use crate::local_pool::{
    accounts::{
        credentials::{
            credential_local_error as credential_error, CredentialStore, StoredCodexCredentials,
        },
        NativeSecretBackend,
    },
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    models::LocalAccountRecord,
    state::DesktopState,
};
use zenith_relay_core::accounts::{AccountAuthState, TokenSet};

pub(in crate::local_pool::commands::oauth) fn rollback_completion_before_authority(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
    local_account_id: &str,
    previous_credentials: Option<&StoredCodexCredentials>,
    previous_account: Option<&LocalAccountRecord>,
    attempted_credentials: &StoredCodexCredentials,
    attempted_account: &LocalAccountRecord,
) -> LocalResult<bool> {
    let current_credentials = credentials
        .load(local_account_id)
        .map_err(credential_error)?;
    let current_account = state.store()?.account(local_account_id).cloned();
    let record_requires_restore = current_account
        .as_ref()
        .is_some_and(|stored_account| stored_account.matches_rollback_snapshot(attempted_account));
    let record_already_previous = match (previous_account, current_account.as_ref()) {
        (Some(previous_account_snapshot), Some(stored_account)) => {
            stored_account.matches_rollback_snapshot(previous_account_snapshot)
        }
        (None, None) => true,
        _ => false,
    };
    if !completion_rollback_owns_state(
        current_credentials.as_ref(),
        attempted_credentials,
        record_requires_restore,
        record_already_previous,
    ) {
        return Ok(false);
    }

    match previous_credentials {
        Some(previous_credentials_snapshot) => credentials
            .save(previous_credentials_snapshot)
            .map_err(credential_error)?,
        None => credentials
            .delete(local_account_id)
            .map_err(credential_error)?,
    }
    if record_requires_restore {
        let restore_record = (|| -> LocalResult<()> {
            let mut store = state.store()?;
            match previous_account {
                Some(previous_account_snapshot) => {
                    let mut restored = previous_account_snapshot.clone();
                    // The watchdog is informational and can update while the OAuth
                    // command runs. It is unrelated to the failed token write.
                    if let Some(account_snapshot) = current_account {
                        restored.client_auth_status = account_snapshot.client_auth_status;
                        restored.last_client_login_redirect_at_ms =
                            account_snapshot.last_client_login_redirect_at_ms;
                    }
                    store.upsert_account(restored)?;
                }
                None => {
                    let accounts = store
                        .accounts()
                        .iter()
                        .filter(|account| account.account.id != local_account_id)
                        .cloned()
                        .collect();
                    let keys = store.keys().to_vec();
                    let automations = store.automations().clone();
                    store.delete_account_state(local_account_id, accounts, keys, automations)?;
                }
            }
            Ok(())
        })();
        if let Err(error) = restore_record {
            // The credential rollback precedes the record write. If that write
            // fails, put the attempted secret back only while this transaction
            // still owns the restored credential snapshot; otherwise a newer
            // login would be overwritten and the account would be split across
            // two token generations.
            let compensated = restore_attempted_completion_credentials_if_current(
                credentials,
                local_account_id,
                previous_credentials,
                attempted_credentials,
            )?;
            return if compensated {
                Err(error)
            } else {
                Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "OAuth completion could not compensate a failed account rollback",
                ))
            };
        }
    }
    Ok(true)
}

pub(in crate::local_pool::commands::oauth) fn restore_attempted_completion_credentials_if_current(
    credentials: &CredentialStore<NativeSecretBackend>,
    local_account_id: &str,
    previous_credentials: Option<&StoredCodexCredentials>,
    attempted_credentials: &StoredCodexCredentials,
) -> LocalResult<bool> {
    let stored_credentials = credentials
        .load(local_account_id)
        .map_err(credential_error)?;
    if !StoredCodexCredentials::snapshots_match(stored_credentials.as_ref(), previous_credentials) {
        return Ok(false);
    }
    credentials
        .save(attempted_credentials)
        .map_err(credential_error)?;
    Ok(true)
}

pub(in crate::local_pool::commands::oauth) fn current_accounts(
    state: &DesktopState,
) -> LocalResult<Vec<LocalAccountRecord>> {
    Ok(state.store()?.accounts().to_vec())
}

pub(in crate::local_pool::commands::oauth) fn next_completion_generation(
    account: Option<&LocalAccountRecord>,
    credentials: Option<&StoredCodexCredentials>,
) -> u64 {
    account
        .map(|account_record| account_record.account.token_generation)
        .into_iter()
        .chain(credentials.map(StoredCodexCredentials::generation))
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

pub(in crate::local_pool::commands::oauth) fn completion_rollback_owns_state(
    current_credentials: Option<&StoredCodexCredentials>,
    attempted_credentials: &StoredCodexCredentials,
    record_requires_restore: bool,
    record_already_previous: bool,
) -> bool {
    current_credentials.is_some_and(|stored_credentials| {
        stored_credentials.matches_snapshot(attempted_credentials)
    }) && (record_requires_restore || record_already_previous)
}

pub(in crate::local_pool::commands::oauth) fn reconcile_completion_authority(
    state: &DesktopState,
    account_id: &str,
    attempted_account: &LocalAccountRecord,
    authoritative_tokens: &TokenSet,
    authoritative_auth_state: AccountAuthState,
) -> LocalResult<bool> {
    let mut store = state.store()?;
    let mut account_record = store
        .account(account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    if !account_record.matches_rollback_snapshot(attempted_account) {
        return Ok(false);
    }
    account_record.account.token_generation = authoritative_tokens.generation();
    account_record.account.token_updated_at_ms = Some(authoritative_tokens.issued_at_ms());
    account_record.account.auth_state = authoritative_auth_state;
    store.upsert_account(account_record)?;
    Ok(true)
}
