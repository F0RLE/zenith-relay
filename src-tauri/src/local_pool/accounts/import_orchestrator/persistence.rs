use super::{credential_item_error, ImportItemError, ItemResult};
use crate::local_pool::accounts::{
    authority::{ProcessAccountLocks, ProcessLockConfig},
    credentials::{CredentialStore, StoredCodexCredentials},
    NativeSecretBackend,
};
use crate::local_pool::commands::{current_time_ms, restart_or_rollback};
use crate::local_pool::error::{ErrorCode, LocalPoolError};
use crate::local_pool::models::LocalAccountRecord;
use crate::local_pool::state::DesktopState;
use zenith_relay_core::accounts::{AccountAuthState, TokenSet};

#[derive(Clone)]
struct ImportedAccountCommit {
    account_id: String,
    previous_credentials: Option<StoredCodexCredentials>,
    previous_account: Option<LocalAccountRecord>,
    attempted_credentials: StoredCodexCredentials,
    attempted_account: LocalAccountRecord,
    runtime_sync_required: bool,
}

pub(in crate::local_pool::accounts) async fn persist_imported_account(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    credentials: &StoredCodexCredentials,
    old_credential: Option<&StoredCodexCredentials>,
    account: LocalAccountRecord,
) -> ItemResult<()> {
    let account_id = credentials.local_account_id().to_string();
    crate::diagnostics::breadcrumb(
        "account-import",
        "persist_started",
        &[("account", crate::diagnostics::hash_identifier(&account_id))],
    );
    let locks =
        ProcessAccountLocks::with_config(state.transient_root(), ProcessLockConfig::default())
            .map_err(|_| ImportItemError::recovery("account credential lock is unavailable"))?;
    // The import can spend time probing models and quota before it reaches this
    // point. Do not commit its old credential snapshot over a login or refresh
    // that finished while those probes were running.
    let commit_guard = locks.acquire(&account_id).await.map_err(|_| {
        ImportItemError::new(
            "account_changed",
            "account credentials changed while importing; retry the import",
        )
    })?;
    let previous_credentials = credential_store
        .load(&account_id)
        .map_err(credential_item_error)?;
    if !credential_snapshots_match(previous_credentials.as_ref(), old_credential) {
        return Err(ImportItemError::new(
            "account_changed",
            "account credentials changed while importing; retry the import",
        ));
    }
    let previous_account = state
        .store()
        .map_err(|_| ImportItemError::new("account_store_failed", "account store is unavailable"))?
        .account(&account_id)
        .cloned();
    let runtime_sync_required = account.account.in_pool
        || previous_account
            .as_ref()
            .is_some_and(|previous| previous.account.in_pool);
    let mut attempted_account = account;
    // A CDP login observation is presentation-only. It may legitimately arrive
    // while the import is committed and must not be erased by the account row.
    if let Some(current) = previous_account.as_ref() {
        attempted_account.client_auth_status = current.client_auth_status.clone();
        attempted_account.last_client_login_redirect_at_ms =
            current.last_client_login_redirect_at_ms;
    }
    credential_store
        .save(credentials)
        .map_err(credential_item_error)?;
    crate::diagnostics::breadcrumb(
        "account-import",
        "credential_saved",
        &[("account", crate::diagnostics::hash_identifier(&account_id))],
    );
    if state
        .store()
        .and_then(|mut store| store.upsert_account(attempted_account.clone()))
        .is_err()
    {
        let restored = restore_import_credentials_if_current(
            credential_store,
            &account_id,
            &previous_credentials,
            credentials,
        )?;
        return Err(if restored {
            ImportItemError::new("account_store_failed", "failed to save account record")
        } else {
            ImportItemError::recovery(
                "failed to save account record and the credential state changed during recovery",
            )
        });
    }
    crate::diagnostics::breadcrumb(
        "account-import",
        "account_saved",
        &[("account", crate::diagnostics::hash_identifier(&account_id))],
    );
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
        crate::diagnostics::breadcrumb(
            "account-import",
            "runtime_sync_started",
            &[(
                "account",
                crate::diagnostics::hash_identifier(&commit.account_id),
            )],
        );
        if sync_imported_account_or_rollback(state, credential_store, &locks, &commit)
            .await
            .is_err()
        {
            return Err(ImportItemError::new(
                "gateway_sync_failed",
                "failed to apply account to the local gateway",
            ));
        }
        crate::diagnostics::breadcrumb(
            "account-import",
            "runtime_sync_completed",
            &[(
                "account",
                crate::diagnostics::hash_identifier(&commit.account_id),
            )],
        );
    } else {
        // Accounts kept outside the local pool do not affect any gateway key
        // scope. Avoid tearing down a healthy listener just to persist an
        // inventory record; the runtime will be rebuilt when the user later
        // adds this account to the pool.
        crate::diagnostics::breadcrumb(
            "account-import",
            "runtime_sync_skipped",
            &[(
                "account",
                crate::diagnostics::hash_identifier(&commit.account_id),
            )],
        );
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
                rollback_after_authority_failure(state, credential_store, &locks, &commit).await?;
                return Err(ImportItemError::new(
                    "token_authority_failed",
                    "failed to register account token state",
                ));
            }
        };
        crate::diagnostics::breadcrumb(
            "account-import",
            "authority_registered",
            &[
                (
                    "account",
                    crate::diagnostics::hash_identifier(&commit.account_id),
                ),
                ("new", registered.to_string()),
            ],
        );
        if !registered {
            reconcile_import_authority(state, credential_store, &locks, &commit, &attempted_tokens)
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
        rollback_after_authority_failure(state, credential_store, &locks, &commit).await?;
        return Err(ImportItemError::new(
            "quota_queue_failed",
            "failed to schedule account quota refresh",
        ));
    }
    crate::diagnostics::breadcrumb(
        "account-import",
        "quota_refresh_scheduled",
        &[(
            "account",
            crate::diagnostics::hash_identifier(&commit.account_id),
        )],
    );
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
        restore_import_durable_state_if_current(&rollback_store, state, &rollback_commit)
            .map_err(|error| LocalPoolError::new(ErrorCode::RecoveryRequired, error.message))?;
        Ok(())
    })
    .await
    .map_err(|_| ImportItemError::recovery("failed to rebuild gateway after account rollback"))
}

async fn rollback_after_authority_failure(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    locks: &ProcessAccountLocks,
    commit: &ImportedAccountCommit,
) -> ItemResult<()> {
    let guard = locks.acquire(&commit.account_id).await.map_err(|_| {
        ImportItemError::recovery(
            "account credentials changed before the failed import could be restored",
        )
    })?;
    let restored = restore_import_durable_state_if_current(credential_store, state, commit)?;
    drop(guard);
    if restored {
        restore_import_authority_if_current(state, commit).await?;
    }
    // The successful first restart may already be serving the attempted
    // account. Rebuild the current durable state, but never use a bulk
    // snapshot rollback here: another account can legitimately change while
    // this import waits for TokenAuthority.
    if commit.runtime_sync_required {
        restart_or_rollback(state, || Ok(())).await.map_err(|_| {
            ImportItemError::recovery("failed to restore gateway after account registration error")
        })?;
    }
    Ok(())
}

fn restore_import_credentials_if_current(
    credential_store: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
    previous_credentials: &Option<StoredCodexCredentials>,
    attempted_credentials: &StoredCodexCredentials,
) -> ItemResult<bool> {
    let current = credential_store
        .load(account_id)
        .map_err(credential_item_error)?;
    if !credential_snapshots_match(current.as_ref(), Some(attempted_credentials)) {
        return Ok(false);
    }
    match previous_credentials {
        Some(credentials) => credential_store.save(credentials),
        None => credential_store.delete(account_id),
    }
    .map_err(|_| ImportItemError::recovery("failed to restore previous account credentials"))?;
    Ok(true)
}

/// Restores only the one imported account, and only if both its persisted
/// credential and record still belong to this transaction. This intentionally
/// does not restore the previous full account/key list: doing that can erase a
/// newer login, quota observation, or unrelated account edit.
fn restore_import_durable_state_if_current(
    credential_store: &CredentialStore<NativeSecretBackend>,
    state: &DesktopState,
    commit: &ImportedAccountCommit,
) -> ItemResult<bool> {
    let current_credentials = credential_store
        .load(&commit.account_id)
        .map_err(credential_item_error)?;
    let mut store = state
        .store()
        .map_err(|_| ImportItemError::recovery("failed to read account state during rollback"))?;
    let current_account = store.account(&commit.account_id).cloned();
    if !credential_snapshots_match(
        current_credentials.as_ref(),
        Some(&commit.attempted_credentials),
    ) || !current_account
        .as_ref()
        .is_some_and(|current| import_record_matches(current, &commit.attempted_account))
    {
        return Ok(false);
    }

    match &commit.previous_credentials {
        Some(credentials) => credential_store.save(credentials),
        None => credential_store.delete(&commit.account_id),
    }
    .map_err(|_| ImportItemError::recovery("failed to restore previous account credentials"))?;

    let restore_record = match &commit.previous_account {
        Some(previous) => {
            let mut restored = previous.clone();
            // Retain a watchdog observation that arrived during the failed
            // import. It is unrelated to credential ownership.
            if let Some(current) = current_account {
                restored.client_auth_status = current.client_auth_status;
                restored.last_client_login_redirect_at_ms =
                    current.last_client_login_redirect_at_ms;
            }
            store
                .upsert_account(restored)
                .map_err(|_| ImportItemError::recovery("failed to restore account record"))
        }
        None => {
            let accounts = store
                .accounts()
                .iter()
                .filter(|account| account.account.id != commit.account_id)
                .cloned()
                .collect();
            let keys = store.keys().to_vec();
            let automations = store.automations().clone();
            store
                .delete_account_state(&commit.account_id, accounts, keys, automations)
                .map_err(|_| ImportItemError::recovery("failed to remove imported account record"))
        }
    };
    if let Err(error) = restore_record {
        // The secret write above is intentionally conditional, but the local
        // record write can still fail independently. Put the attempted
        // credentials back only if this transaction still owns the restored
        // snapshot; otherwise surface recovery instead of overwriting a newer
        // login.
        let restored_attempt = restore_attempted_import_credentials_if_current(
            credential_store,
            &commit.account_id,
            &commit.previous_credentials,
            &commit.attempted_credentials,
        )?;
        return if restored_attempt {
            Err(error)
        } else {
            Err(ImportItemError::recovery(
                "failed to restore account record and credential rollback could not be compensated",
            ))
        };
    }
    Ok(true)
}

fn restore_attempted_import_credentials_if_current(
    credential_store: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
    previous_credentials: &Option<StoredCodexCredentials>,
    attempted_credentials: &StoredCodexCredentials,
) -> ItemResult<bool> {
    let current = credential_store
        .load(account_id)
        .map_err(credential_item_error)?;
    if !credential_snapshots_match(current.as_ref(), previous_credentials.as_ref()) {
        return Ok(false);
    }
    credential_store.save(attempted_credentials).map_err(|_| {
        ImportItemError::recovery("failed to compensate account credential rollback")
    })?;
    Ok(true)
}

async fn restore_import_authority_if_current(
    state: &DesktopState,
    commit: &ImportedAccountCommit,
) -> ItemResult<()> {
    if !commit.attempted_credentials.has_oauth() {
        return Ok(());
    }
    let attempted_tokens = commit
        .attempted_credentials
        .to_token_set()
        .map_err(credential_item_error)?;
    let authority = state.token_authority();
    match commit
        .previous_credentials
        .as_ref()
        .filter(|credentials| credentials.has_oauth())
    {
        Some(previous) => {
            let previous_tokens = previous.to_token_set().map_err(credential_item_error)?;
            authority
                .replace_if_current(
                    &commit.account_id,
                    &attempted_tokens,
                    commit.attempted_account.account.auth_state,
                    previous_tokens,
                    commit
                        .previous_account
                        .as_ref()
                        .map_or(AccountAuthState::Unknown, |account| {
                            account.account.auth_state
                        }),
                )
                .await
                .map_err(|_| {
                    ImportItemError::recovery(
                        "failed to restore previous account token authority state",
                    )
                })?;
        }
        None => {
            authority
                .remove_if_current(
                    &commit.account_id,
                    &attempted_tokens,
                    commit.attempted_account.account.auth_state,
                )
                .await
                .map_err(|_| {
                    ImportItemError::recovery(
                        "failed to remove failed account token authority state",
                    )
                })?;
        }
    }
    Ok(())
}

async fn reconcile_import_authority(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    locks: &ProcessAccountLocks,
    commit: &ImportedAccountCommit,
    attempted_tokens: &TokenSet,
) -> ItemResult<()> {
    let authority = state.token_authority();
    let authoritative_tokens = authority.tokens(&commit.account_id).await.ok_or_else(|| {
        ImportItemError::recovery("newer account token state disappeared during import")
    })?;
    let authoritative_auth_state =
        authority
            .auth_state(&commit.account_id)
            .await
            .ok_or_else(|| {
                ImportItemError::recovery(
                    "newer account authentication state disappeared during import",
                )
            })?;
    if authoritative_tokens == *attempted_tokens
        && authoritative_auth_state == commit.attempted_account.account.auth_state
    {
        return Ok(());
    }
    // The import may have just put its older secret back after an in-memory
    // refresh already won the authority slot. Serialize the comparison and
    // replacement with refresh/OAuth persistence, but never await the
    // authority while holding this process lock: its persistence adapter takes
    // the same lock after its account mutex.
    let _guard = locks.acquire(&commit.account_id).await.map_err(|_| {
        ImportItemError::recovery(
            "newer account credentials could not acquire the persistence lock",
        )
    })?;
    let current_credentials = credential_store
        .load(&commit.account_id)
        .map_err(credential_item_error)?;
    let credential_matches_attempt = credential_snapshots_match(
        current_credentials.as_ref(),
        Some(&commit.attempted_credentials),
    );
    let credential_matches_authority = current_credentials
        .as_ref()
        .and_then(|credentials| credentials.to_token_set().ok())
        .is_some_and(|tokens| tokens == authoritative_tokens);
    if credential_matches_attempt {
        let current = current_credentials.as_ref().ok_or_else(|| {
            ImportItemError::recovery(
                "attempted account credential disappeared during reconciliation",
            )
        })?;
        let updated = current
            .with_token_set(&authoritative_tokens)
            .map_err(credential_item_error)?;
        credential_store
            .save(&updated)
            .map_err(credential_item_error)?;
    } else if !credential_matches_authority {
        // A separate credential transaction completed while the authority was
        // observed. Its durable secret is now the source of truth; do not
        // splice this import's metadata onto a different login.
        return Ok(());
    }
    let mut store = state
        .store()
        .map_err(|_| ImportItemError::recovery("failed to reconcile newer account state"))?;
    let mut account = store.account(&commit.account_id).cloned().ok_or_else(|| {
        ImportItemError::recovery("imported account disappeared during reconciliation")
    })?;
    if !import_token_state_matches(&account, &commit.attempted_account) {
        return Ok(());
    }
    account.account.token_generation = authoritative_tokens.generation();
    account.account.token_updated_at_ms = Some(authoritative_tokens.issued_at_ms());
    account.account.auth_state = authoritative_auth_state;
    store
        .upsert_account(account)
        .map_err(|_| ImportItemError::recovery("failed to persist newer account token state"))
}

fn credential_snapshots_match(
    current: Option<&StoredCodexCredentials>,
    expected: Option<&StoredCodexCredentials>,
) -> bool {
    match (current, expected) {
        (Some(current), Some(expected)) => current.matches_snapshot(expected),
        (None, None) => true,
        _ => false,
    }
}

fn import_record_matches(current: &LocalAccountRecord, attempted: &LocalAccountRecord) -> bool {
    let mut comparable = current.clone();
    // CDP observations are deliberately allowed to progress while credentials
    // are being changed. All operational state must still match exactly before
    // a rollback can claim ownership.
    comparable.client_auth_status = attempted.client_auth_status.clone();
    comparable.last_client_login_redirect_at_ms = attempted.last_client_login_redirect_at_ms;
    comparable == *attempted
}

fn import_token_state_matches(
    current: &LocalAccountRecord,
    attempted: &LocalAccountRecord,
) -> bool {
    current.account.source_id == attempted.account.source_id
        && current.account.token_generation == attempted.account.token_generation
        && current.account.token_updated_at_ms == attempted.account.token_updated_at_ms
        && current.account.auth_state == attempted.account.auth_state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::accounts::records;
    use crate::local_pool::commands::runtime_from_store;
    use crate::local_pool::store::secret_store;
    use zenith_relay_core::accounts::AccountAuthMode;

    fn credentials(
        account_id: &str,
        suffix: &str,
        issued_at_ms: u64,
        generation: u64,
    ) -> StoredCodexCredentials {
        StoredCodexCredentials::new(
            account_id,
            format!("access-{suffix}"),
            Some(format!("refresh-{suffix}")),
            Some(format!("id-{suffix}")),
            Some(issued_at_ms.saturating_add(60_000)),
            issued_at_ms,
            generation,
            Some("private@example.test".into()),
            Some("provider-private".into()),
            None,
            None,
            Some("plus".into()),
            false,
        )
        .expect("synthetic credentials")
    }

    fn account(credentials: &StoredCodexCredentials) -> LocalAccountRecord {
        records::new_account_record(
            credentials,
            AccountAuthMode::OAuth,
            vec!["gpt-test".into()],
            0,
            credentials.issued_at_ms(),
        )
        .expect("synthetic account")
    }

    async fn cleanup_import_test_state(
        state: DesktopState,
        credential_store: CredentialStore<NativeSecretBackend>,
        account_id: &str,
        root: std::path::PathBuf,
    ) {
        state.gateway.stop().await;
        credential_store
            .delete(account_id)
            .expect("cleanup account");
        let key_refs = {
            let store = state.store().expect("store");
            store
                .keys()
                .iter()
                .map(|key| key.secret_ref.clone())
                .collect::<Vec<_>>()
        };
        for secret_ref in key_refs {
            secret_store::delete(&secret_ref).expect("cleanup key");
        }
        drop(state);
        std::fs::remove_dir_all(root).expect("cleanup state");
    }

    #[tokio::test]
    async fn importing_an_account_outside_the_pool_keeps_the_running_gateway() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-import-no-pool-runtime-{}",
            uuid::Uuid::new_v4()
        ));
        let state = DesktopState::open(root.clone()).expect("state");
        let runtime = runtime_from_store(&state).await.expect("runtime");
        let address = state
            .gateway
            .start(runtime.clone(), 0)
            .await
            .expect("gateway");
        let credential_store = CredentialStore::from_backend(NativeSecretBackend);
        let account_id = format!("account_{}", uuid::Uuid::new_v4().simple());
        let imported = credentials(&account_id, "outside-pool", 10, 1);
        let record = account(&imported);

        persist_imported_account(&state, &credential_store, &imported, None, record)
            .await
            .expect("import");

        assert_eq!(state.gateway.address().await, Some(address));
        let running = state.gateway.runtime().await.expect("running runtime");
        assert!(std::sync::Arc::ptr_eq(&running, &runtime));
        drop(running);
        drop(runtime);

        cleanup_import_test_state(state, credential_store, &account_id, root).await;
    }

    #[tokio::test]
    async fn importing_a_pool_account_restarts_the_gateway_without_losing_it() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-import-pool-runtime-{}",
            uuid::Uuid::new_v4()
        ));
        let state = DesktopState::open(root.clone()).expect("state");
        let runtime = runtime_from_store(&state).await.expect("runtime");
        let address = state.gateway.start(runtime, 0).await.expect("gateway");
        let mut gateway = state.store().expect("store").gateway().clone();
        gateway.port = address.port();
        state
            .store()
            .expect("store")
            .replace_gateway(gateway)
            .expect("gateway settings");
        let credential_store = CredentialStore::from_backend(NativeSecretBackend);
        let account_id = format!("account_{}", uuid::Uuid::new_v4().simple());
        let imported = credentials(&account_id, "pool", 10, 1);
        let mut record = account(&imported);
        record.account.in_pool = true;

        persist_imported_account(&state, &credential_store, &imported, None, record)
            .await
            .expect("import");

        assert_eq!(
            state.gateway.address().await.map(|value| value.port()),
            Some(address.port())
        );
        let running = state.gateway.runtime().await.expect("running runtime");
        assert!(running
            .candidate_runtime_order()
            .iter()
            .any(|candidate| candidate.candidate_id.starts_with("account_")));
        drop(running);
        cleanup_import_test_state(state, credential_store, &account_id, root).await;
    }

    #[test]
    fn import_rollback_restores_only_its_own_durable_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-import-rollback-{}",
            uuid::Uuid::new_v4()
        ));
        let state = DesktopState::open(root.clone()).expect("state");
        let credential_store = CredentialStore::from_backend(NativeSecretBackend);
        let account_id = format!("account_{}", uuid::Uuid::new_v4().simple());
        let previous_credentials = credentials(&account_id, "previous", 10, 1);
        let attempted_credentials = credentials(&account_id, "attempted", 20, 2);
        let previous_account = account(&previous_credentials);
        let attempted_account = account(&attempted_credentials);
        let unrelated_credentials = credentials("account_unrelated", "other", 30, 1);
        let unrelated_account = account(&unrelated_credentials);

        credential_store
            .save(&attempted_credentials)
            .expect("attempted secret");
        state
            .store()
            .expect("store")
            .upsert_account(attempted_account.clone())
            .expect("attempted account");
        state
            .store()
            .expect("store")
            .upsert_account(unrelated_account.clone())
            .expect("unrelated account");

        let commit = ImportedAccountCommit {
            account_id: account_id.clone(),
            previous_credentials: Some(previous_credentials.clone()),
            previous_account: Some(previous_account.clone()),
            attempted_credentials: attempted_credentials.clone(),
            attempted_account,
            runtime_sync_required: false,
        };

        assert!(
            restore_import_durable_state_if_current(&credential_store, &state, &commit)
                .expect("rollback")
        );
        assert!(credential_store
            .require(&account_id)
            .expect("restored secret")
            .matches_snapshot(&previous_credentials));
        let store = state.store().expect("store");
        assert_eq!(store.account(&account_id), Some(&previous_account));
        assert_eq!(store.account("account_unrelated"), Some(&unrelated_account));
        drop(store);

        credential_store
            .delete(&account_id)
            .expect("cleanup account secret");
        drop(state);
        std::fs::remove_dir_all(root).expect("cleanup state");
    }

    #[test]
    fn import_rollback_never_replaces_a_newer_credential_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-import-stale-rollback-{}",
            uuid::Uuid::new_v4()
        ));
        let state = DesktopState::open(root.clone()).expect("state");
        let credential_store = CredentialStore::from_backend(NativeSecretBackend);
        let account_id = format!("account_{}", uuid::Uuid::new_v4().simple());
        let previous_credentials = credentials(&account_id, "previous", 10, 1);
        let attempted_credentials = credentials(&account_id, "attempted", 20, 2);
        let newer_credentials = credentials(&account_id, "newer", 30, 3);
        let previous_account = account(&previous_credentials);
        let attempted_account = account(&attempted_credentials);
        let newer_account = account(&newer_credentials);

        credential_store
            .save(&newer_credentials)
            .expect("newer secret");
        state
            .store()
            .expect("store")
            .upsert_account(newer_account.clone())
            .expect("newer account");
        let commit = ImportedAccountCommit {
            account_id: account_id.clone(),
            previous_credentials: Some(previous_credentials),
            previous_account: Some(previous_account),
            attempted_credentials,
            attempted_account,
            runtime_sync_required: false,
        };

        assert!(
            !restore_import_durable_state_if_current(&credential_store, &state, &commit)
                .expect("stale rollback")
        );
        assert!(credential_store
            .require(&account_id)
            .expect("newer secret")
            .matches_snapshot(&newer_credentials));
        assert_eq!(
            state.store().expect("store").account(&account_id),
            Some(&newer_account)
        );

        credential_store
            .delete(&account_id)
            .expect("cleanup account secret");
        drop(state);
        std::fs::remove_dir_all(root).expect("cleanup state");
    }

    #[tokio::test]
    async fn stale_import_reconciliation_persists_the_authoritative_tokens() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-import-authority-reconcile-{}",
            uuid::Uuid::new_v4()
        ));
        let state = DesktopState::open(root.clone()).expect("state");
        let credential_store = CredentialStore::from_backend(NativeSecretBackend);
        let account_id = format!("account_{}", uuid::Uuid::new_v4().simple());
        let previous_credentials = credentials(&account_id, "previous", 10, 1);
        let attempted_credentials = credentials(&account_id, "attempted", 20, 2);
        let authoritative_credentials = credentials(&account_id, "authoritative", 30, 3);
        let previous_account = account(&previous_credentials);
        let attempted_account = account(&attempted_credentials);
        credential_store
            .save(&attempted_credentials)
            .expect("attempted secret");
        state
            .store()
            .expect("store")
            .upsert_account(attempted_account.clone())
            .expect("attempted account");

        let authority = state.token_authority();
        authority
            .register(
                &account_id,
                authoritative_credentials
                    .to_token_set()
                    .expect("authoritative tokens"),
                AccountAuthState::Active,
            )
            .await
            .expect("authority");
        let attempted_tokens = attempted_credentials
            .to_token_set()
            .expect("attempted tokens");
        assert!(!authority
            .register_if_newer(
                &account_id,
                attempted_tokens.clone(),
                AccountAuthState::Active
            )
            .await
            .expect("stale registration"));

        let commit = ImportedAccountCommit {
            account_id: account_id.clone(),
            previous_credentials: Some(previous_credentials),
            previous_account: Some(previous_account),
            attempted_credentials: attempted_credentials.clone(),
            attempted_account,
            runtime_sync_required: false,
        };
        let locks =
            ProcessAccountLocks::with_config(state.transient_root(), ProcessLockConfig::default())
                .expect("locks");
        reconcile_import_authority(
            &state,
            &credential_store,
            &locks,
            &commit,
            &attempted_tokens,
        )
        .await
        .expect("reconcile");

        assert!(credential_store
            .require(&account_id)
            .expect("authoritative secret")
            .matches_snapshot(&authoritative_credentials));
        let account = state
            .store()
            .expect("store")
            .account(&account_id)
            .cloned()
            .expect("account");
        assert_eq!(account.account.token_generation, 3);
        assert_eq!(account.account.token_updated_at_ms, Some(30));
        assert_eq!(account.account.auth_state, AccountAuthState::Active);

        credential_store
            .delete(&account_id)
            .expect("cleanup account secret");
        drop(state);
        std::fs::remove_dir_all(root).expect("cleanup state");
    }
}
