use super::{credential_item_error, ImportItemError, ItemResult};
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::mutations::{
    current_account_records, repair_gateway_after_item_restore, restore_credential_item,
};
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::{current_time_ms, sync_accounts_or_rollback};
use crate::local_pool::models::{LocalAccountRecord, LocalGatewayKeyRecord};
use crate::local_pool::state::DesktopState;
use zenith_relay_core::accounts::AccountAuthState;

pub(in crate::local_pool::accounts) async fn persist_imported_account(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    credentials: &StoredCodexCredentials,
    old_credential: Option<&StoredCodexCredentials>,
    account: LocalAccountRecord,
) -> ItemResult<()> {
    let (old_accounts, old_keys) = current_account_records(state).map_err(|_| {
        ImportItemError::new("account_store_failed", "account store is unavailable")
    })?;
    let sync_gateway = !account.effective_models().is_empty();
    credential_store
        .save(credentials)
        .map_err(credential_item_error)?;
    if state
        .store()
        .and_then(|mut store| store.upsert_account(account.clone()))
        .is_err()
    {
        restore_credential_item(
            credential_store,
            credentials.local_account_id(),
            old_credential,
        )?;
        return Err(ImportItemError::new(
            "account_store_failed",
            "failed to save account record",
        ));
    }
    if sync_gateway
        && sync_accounts_or_rollback(state, old_accounts.clone(), old_keys.clone())
            .await
            .is_err()
    {
        restore_credential_item(
            credential_store,
            credentials.local_account_id(),
            old_credential,
        )?;
        repair_gateway_after_item_restore(state, old_accounts, old_keys).await?;
        return Err(ImportItemError::new(
            "gateway_sync_failed",
            "failed to apply account to the local gateway",
        ));
    }
    if credentials.has_oauth()
        && state
            .token_authority()
            .register(
                credentials.local_account_id(),
                credentials.to_token_set().map_err(credential_item_error)?,
                account.account.auth_state,
            )
            .await
            .is_err()
    {
        rollback_after_authority_failure(
            state,
            credential_store,
            credentials.local_account_id(),
            old_credential,
            old_accounts,
            old_keys,
        )
        .await?;
        return Err(ImportItemError::new(
            "token_authority_failed",
            "failed to register account token state",
        ));
    }
    if state
        .sync_account_quota_refresh(credentials.local_account_id(), current_time_ms())
        .is_err()
    {
        rollback_after_authority_failure(
            state,
            credential_store,
            credentials.local_account_id(),
            old_credential,
            old_accounts,
            old_keys,
        )
        .await?;
        return Err(ImportItemError::new(
            "quota_queue_failed",
            "failed to schedule account quota refresh",
        ));
    }
    Ok(())
}

async fn rollback_after_authority_failure(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
    old_credential: Option<&StoredCodexCredentials>,
    old_accounts: Vec<LocalAccountRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
) -> ItemResult<()> {
    let (new_accounts, new_keys) = current_account_records(state)
        .map_err(|_| ImportItemError::recovery("failed to read account state during rollback"))?;
    restore_credential_item(credential_store, account_id, old_credential)?;
    state
        .store()
        .and_then(|mut store| {
            store.replace_accounts_and_keys(old_accounts.clone(), old_keys.clone())
        })
        .map_err(|_| {
            ImportItemError::recovery("failed to restore account records after registration error")
        })?;
    sync_accounts_or_rollback(state, new_accounts, new_keys)
        .await
        .map_err(|_| {
            ImportItemError::recovery("failed to restore gateway after registration error")
        })?;
    restore_authority(state, account_id, old_credential, &old_accounts).await?;
    Ok(())
}

async fn restore_authority(
    state: &DesktopState,
    account_id: &str,
    old_credential: Option<&StoredCodexCredentials>,
    old_accounts: &[LocalAccountRecord],
) -> ItemResult<()> {
    let Some(credentials) = old_credential else {
        state.token_authority().remove(account_id);
        return Ok(());
    };
    if !credentials.has_oauth() {
        state.token_authority().remove(account_id);
        return Ok(());
    }
    let auth_state = old_accounts
        .iter()
        .find(|account| account.account.id == account_id)
        .map_or(AccountAuthState::Unknown, |account| {
            account.account.auth_state
        });
    state
        .token_authority()
        .register(
            account_id,
            credentials.to_token_set().map_err(credential_item_error)?,
            auth_state,
        )
        .await
        .map_err(|_| ImportItemError::recovery("failed to restore previous token authority state"))
}
