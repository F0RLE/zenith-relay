use super::super::{credential_item_error, ImportItemError, ItemResult};
use super::ImportedAccountCommit;
use crate::local_pool::accounts::{
    authority::ProcessAccountLocks,
    credentials::{CredentialStore, StoredCodexCredentials},
    NativeSecretBackend,
};
use crate::local_pool::commands::restart_or_rollback;
use crate::local_pool::models::LocalAccountRecord;
use crate::local_pool::state::DesktopState;
use zenith_relay_core::accounts::{AccountAuthState, TokenSet};

pub(super) async fn rollback_after_authority_failure(
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

pub(super) fn restore_import_credentials_if_current(
    credential_store: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
    previous_credentials: &Option<StoredCodexCredentials>,
    attempted_credentials: &StoredCodexCredentials,
) -> ItemResult<bool> {
    let current = credential_store
        .load(account_id)
        .map_err(credential_item_error)?;
    if !StoredCodexCredentials::snapshots_match(current.as_ref(), Some(attempted_credentials)) {
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
pub(super) fn restore_import_durable_state_if_current(
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
    if !StoredCodexCredentials::snapshots_match(
        current_credentials.as_ref(),
        Some(&commit.attempted_credentials),
    ) || !current_account
        .as_ref()
        .is_some_and(|current| current.matches_rollback_snapshot(&commit.attempted_account))
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
    if !StoredCodexCredentials::snapshots_match(current.as_ref(), previous_credentials.as_ref()) {
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

pub(super) async fn reconcile_import_authority(
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
    let credential_matches_attempt = StoredCodexCredentials::snapshots_match(
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

fn import_token_state_matches(
    current: &LocalAccountRecord,
    attempted: &LocalAccountRecord,
) -> bool {
    current.account.source_id == attempted.account.source_id
        && current.account.token_generation == attempted.account.token_generation
        && current.account.token_updated_at_ms == attempted.account.token_updated_at_ms
        && current.account.auth_state == attempted.account.auth_state
}
