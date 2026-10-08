use super::super::{credential_item_error, ImportItemError, ItemResult};
use super::ImportedAccountCommit;
use crate::local_pool::accounts::{
    authority::{ProcessAccountLocks, ProcessLockConfig},
    credentials::{CredentialStore, StoredCodexCredentials},
    NativeSecretBackend,
};
use crate::local_pool::commands::{current_time_ms, fence_runtime_candidates, restart_or_rollback};
use crate::local_pool::error::{ErrorCode, LocalPoolError};
use crate::local_pool::models::LocalAccountRecord;
use crate::local_pool::state::DesktopState;
use zenith_relay_core::error_codes;

pub(in crate::local_pool::accounts) async fn persist_imported_account(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    credentials: &StoredCodexCredentials,
    old_credential: Option<&StoredCodexCredentials>,
    account: LocalAccountRecord,
) -> ItemResult<()> {
    let account_id = credentials.local_account_id().to_string();
    let account_hash = crate::diagnostics::hash_identifier(&account_id);
    account_import_event("persist_started", &account_hash);
    let locks =
        ProcessAccountLocks::with_config(state.transient_root(), ProcessLockConfig::default())
            .map_err(|_| ImportItemError::recovery("account credential lock is unavailable"))?;
    // The import can spend time probing models and quota before it reaches this
    // point. Do not commit its old credential snapshot over a login or refresh
    // that finished while those probes were running.
    let commit_guard = locks.acquire(&account_id).await.map_err(|_| {
        ImportItemError::new(
            error_codes::ACCOUNT_CHANGED,
            "account credentials changed while importing; retry the import",
        )
    })?;
    let previous_credentials = credential_store
        .load(&account_id)
        .map_err(credential_item_error)?;
    if !StoredCodexCredentials::snapshots_match(previous_credentials.as_ref(), old_credential) {
        return Err(ImportItemError::new(
            error_codes::ACCOUNT_CHANGED,
            "account credentials changed while importing; retry the import",
        ));
    }
    let previous_account = state
        .store()
        .map_err(|_| {
            ImportItemError::new(
                error_codes::ACCOUNT_STORE_FAILED,
                "account store is unavailable",
            )
        })?
        .account(&account_id)
        .cloned();
    let runtime_sync_required = account.account.in_pool
        || previous_account
            .as_ref()
            .is_some_and(|previous_account| previous_account.account.in_pool);
    let mut attempted_account = account;
    // A CDP login observation is presentation-only. It may legitimately arrive
    // while the import is committed and must not be erased by the account row.
    if let Some(previous_account_snapshot) = previous_account.as_ref() {
        attempted_account.client_auth_status = previous_account_snapshot.client_auth_status.clone();
        attempted_account.last_client_login_redirect_at_ms =
            previous_account_snapshot.last_client_login_redirect_at_ms;
    }
    // Re-import may replace an existing login and executor. Close pending
    // final dispatches before the first credential write, not only when the
    // listener is eventually restarted. A new account has no old candidate.
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences =
        fence_runtime_candidates(runtime.as_deref(), std::slice::from_ref(&account_id), &[]);
    state
        .store()
        .and_then(|mut store| store.invalidate_account_refresh(&[&account_id]))
        .map_err(|_| {
            ImportItemError::new(
                error_codes::ACCOUNT_STORE_FAILED,
                "failed to retire previous account observations",
            )
        })?;
    credential_store
        .save(credentials)
        .map_err(credential_item_error)?;
    account_import_event("credential_saved", &account_hash);
    if state
        .store()
        .and_then(|mut store| store.upsert_account(attempted_account.clone()))
        .is_err()
    {
        let restored = super::rollback::restore_import_credentials_if_current(
            credential_store,
            &account_id,
            &previous_credentials,
            credentials,
        )?;
        return Err(if restored {
            ImportItemError::new(
                error_codes::ACCOUNT_STORE_FAILED,
                "failed to save account record",
            )
        } else {
            ImportItemError::recovery(
                "failed to save account record and the credential state changed during recovery",
            )
        });
    }
    account_import_event("account_saved", &account_hash);
    let commit = ImportedAccountCommit {
        account_id,
        previous_credentials,
        previous_account,
        attempted_credentials: credentials.clone(),
        attempted_account,
        runtime_sync_required,
    };
    // Runtime construction can wait on TokenAuthority while its automatic
    // adapter waits for this process lock. Release it before restarting the
    // gateway, just like the OAuth and explicit-refresh transactions do.
    drop(commit_guard);

    if commit.runtime_sync_required {
        account_import_event("runtime_sync_started", &account_hash);
        if sync_imported_account_or_rollback(state, credential_store, &locks, &commit)
            .await
            .is_err()
        {
            return Err(ImportItemError::new(
                error_codes::GATEWAY_SYNC_FAILED,
                "failed to apply account to the local gateway",
            ));
        }
        account_import_event("runtime_sync_completed", &account_hash);
    } else {
        // Accounts kept outside the local pool do not affect any gateway key
        // scope. Avoid tearing down a healthy listener just to persist an
        // inventory record; the runtime will be rebuilt when the user later
        // adds this account to the pool.
        account_import_event("runtime_sync_skipped", &account_hash);
    }

    if credentials.has_oauth() {
        let attempted_tokens = credentials.to_token_set().map_err(credential_item_error)?;
        let registered = match state
            .token_authority()
            .register_if_newer(
                &commit.account_id,
                attempted_tokens.clone(),
                commit.attempted_account.account.auth_state,
            )
            .await
        {
            Ok(registered) => registered,
            Err(_) => {
                super::rollback::rollback_after_authority_failure(
                    state,
                    credential_store,
                    &locks,
                    &commit,
                )
                .await?;
                return Err(ImportItemError::new(
                    error_codes::TOKEN_AUTHORITY_FAILED,
                    "failed to register account token state",
                ));
            }
        };
        crate::diagnostics::breadcrumb(
            "account-import",
            "authority_registered",
            &[
                ("account", account_hash.clone()),
                ("new", registered.to_string()),
            ],
        );
        if !registered {
            super::rollback::reconcile_import_authority(
                state,
                credential_store,
                &locks,
                &commit,
                &attempted_tokens,
            )
            .await?;
            // A newer OAuth or automatic refresh owns the authority slot. Its
            // persisted credential and account state now win, so rebuild from
            // that current state rather than rolling it back to this import's
            // snapshot.
            if commit.runtime_sync_required {
                restart_or_rollback(state, || Ok(())).await.map_err(|_| {
                    ImportItemError::recovery(
                        "failed to apply newer account credentials to the gateway",
                    )
                })?;
            }
        }
    }
    if state
        .sync_account_quota_refresh(&commit.account_id, current_time_ms())
        .is_err()
    {
        super::rollback::rollback_after_authority_failure(state, credential_store, &locks, &commit)
            .await?;
        return Err(ImportItemError::new(
            error_codes::QUOTA_QUEUE_FAILED,
            "failed to schedule account quota refresh",
        ));
    }
    account_import_event("quota_refresh_scheduled", &account_hash);
    Ok(())
}

async fn sync_imported_account_or_rollback(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    locks: &ProcessAccountLocks,
    commit: &ImportedAccountCommit,
) -> ItemResult<()> {
    let rollback_store = credential_store.clone();
    let rollback_locks = locks.clone();
    let rollback_commit = commit.clone();
    restart_or_rollback(state, move || {
        // The retrying transaction may have been superseded by a new OAuth
        // completion or refresh. A non-blocking lock makes that newer state
        // authoritative instead of restoring a whole, stale account snapshot.
        let Some(_guard) = rollback_locks
            .try_acquire(&rollback_commit.account_id)
            .map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "account credential lock is unavailable",
                )
            })?
        else {
            return Ok(());
        };
        super::rollback::restore_import_durable_state_if_current(
            &rollback_store,
            state,
            &rollback_commit,
        )
        .map_err(|error| LocalPoolError::new(ErrorCode::RecoveryRequired, error.message))?;
        Ok(())
    })
    .await
    .map_err(|_| ImportItemError::recovery("failed to rebuild gateway after account rollback"))
}

fn account_import_event(action: &str, account_hash: &str) {
    crate::diagnostics::breadcrumb(
        "account-import",
        action,
        &[("account", account_hash.to_string())],
    );
}
