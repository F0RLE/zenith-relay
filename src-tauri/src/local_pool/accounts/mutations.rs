use super::import_orchestrator::{
    apply_account_patch, credential_local_error, prune_key_account_scopes, validate_account_record,
    ImportItemError,
};
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::exports::normalize_account_ids;
use crate::local_pool::accounts::proxy::ProxyPool;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::{current_time_ms, sync_accounts_or_rollback};
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::{
    AutomationRecords, LocalAccountRecord, LocalGatewayKeyRecord, LocalPoolSnapshot,
};
use crate::local_pool::profiles::codex;
use crate::local_pool::state::DesktopState;
use serde::Deserialize;
use std::path::Path;
use tauri::State;
use zenith_relay_core::automations::AccountSelector;

type CommandResult<T> = std::result::Result<T, CommandError>;
type ItemResult<T> = std::result::Result<T, ImportItemError>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateAccountInput {
    pub(super) account_id: String,
    #[serde(default)]
    pub(super) label: Option<String>,
    #[serde(default)]
    pub(super) priority: Option<i32>,
    #[serde(default)]
    pub(super) weight: Option<u32>,
    #[serde(default)]
    pub(super) allowed_models: Option<Vec<String>>,
    #[serde(default)]
    pub(super) excluded_models: Option<Vec<String>>,
    #[serde(default)]
    pub(super) in_pool: Option<bool>,
    #[serde(default)]
    pub(super) draining: Option<bool>,
    #[serde(default)]
    pub(super) purchase_cost_micro_usd: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetAccountProxyInput {
    pub(super) account_id: String,
    pub(super) proxy_url: Option<String>,
    #[serde(default)]
    pub(super) bypass_common_proxy: bool,
}

#[tauri::command]
pub async fn update_local_account(
    input: UpdateAccountInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let account_id = input.account_id.clone();
    let mut account = state
        .store()?
        .account(&account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    apply_account_patch(&mut account, input)?;
    validate_account_record(&account)?;
    let (old_accounts, old_keys) = current_account_records(&state)?;
    state.store()?.upsert_account(account)?;
    sync_accounts_or_rollback(&state, old_accounts, old_keys).await?;
    state.sync_account_quota_refresh(&account_id, current_time_ms())?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_account_proxy(
    input: SetAccountProxyInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    crate::local_pool::commands::proxies::set_account_proxy_inner(
        input.account_id,
        input.proxy_url,
        input.bypass_common_proxy,
        &state,
    )
    .await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_account_enabled(
    account_id: String,
    enabled: bool,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let mut account = state
        .store()?
        .account(&account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    if account.account.enabled == enabled {
        return state.snapshot().await.map_err(Into::into);
    }
    account.account.enabled = enabled;
    if enabled {
        validate_account_record(&account)?;
    }
    let (old_accounts, old_keys) = current_account_records(&state)?;
    state.store()?.upsert_account(account)?;
    sync_accounts_or_rollback(&state, old_accounts, old_keys).await?;
    state.sync_account_quota_refresh(&account_id, current_time_ms())?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_account_draining(
    account_id: String,
    draining: bool,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let mut account = state
        .store()?
        .account(&account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    if account.account.draining == draining {
        return state.snapshot().await.map_err(Into::into);
    }
    account.account.draining = draining;
    let (old_accounts, old_keys) = current_account_records(&state)?;
    state.store()?.upsert_account(account)?;
    sync_accounts_or_rollback(&state, old_accounts, old_keys).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn delete_local_account(
    account_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    ensure_accounts_not_in_ownership_operation(&state, std::slice::from_ref(&account_id))?;
    delete_local_account_inner(&account_id, &state).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn delete_local_accounts(
    account_ids: Vec<String>,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let account_ids = normalize_account_ids(account_ids)?;
    let _mutation = state.setup_guard().await;
    ensure_accounts_not_in_ownership_operation(&state, &account_ids)?;
    for account_id in account_ids {
        delete_local_account_inner(&account_id, &state).await?;
    }
    state.snapshot().await.map_err(Into::into)
}

pub(super) fn ensure_accounts_not_in_ownership_operation(
    state: &DesktopState,
    account_ids: &[String],
) -> CommandResult<()> {
    if state
        .store()?
        .ownership_operation()
        .is_some_and(|operation| {
            operation
                .local_account_ids
                .iter()
                .any(|account_id| account_ids.contains(account_id))
        })
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "account ownership recovery must finish before deleting this local record",
        )
        .into());
    }
    Ok(())
}

pub(super) async fn delete_local_account_inner(
    account_id: &str,
    state: &DesktopState,
) -> CommandResult<()> {
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let (old_accounts, old_keys, _) = current_account_state(state)?;
    if !old_accounts
        .iter()
        .any(|account| account.account.id == account_id)
    {
        return Err(LocalPoolError::new(ErrorCode::NotFound, "account not found").into());
    }
    let old_credential = credentials
        .load(account_id)
        .map_err(credential_local_error)?;
    let old_automations = state.store()?.automations().clone();
    let previous_quota_refresh = state.quota_refresh_snapshot()?;
    let previous_wake = state.wake_snapshot()?;
    let bindings = codex::account_bindings(&state.profile_backup_root())?
        .into_iter()
        .filter(|binding| binding.credential_id == account_id)
        .collect::<Vec<_>>();
    if !bindings.is_empty() && old_credential.is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "account profile binding exists without stored credentials",
        )
        .into());
    }
    let restored_bindings =
        restore_bound_account_profiles(state, &bindings, old_credential.as_ref())?;
    if let Err(error) = state.remove_pending_wakes_for_account(account_id) {
        rollback_deleted_account_side_effects(
            state,
            &credentials,
            account_id,
            old_credential.as_ref(),
            previous_quota_refresh,
            previous_wake,
            old_automations,
            &restored_bindings,
            None,
            &error,
        )?;
        return Err(error.into());
    }
    let accounts = old_accounts
        .iter()
        .filter(|account| account.account.id != account_id)
        .cloned()
        .collect::<Vec<_>>();
    let mut keys = old_keys.clone();
    prune_key_account_scopes(&mut keys, &accounts);
    let automations = prune_account_task_selectors(old_automations.clone(), account_id);
    let previous_proxy_pool = release_account_proxy(account_id)?;
    if let Err(error) = credentials
        .delete(account_id)
        .map_err(credential_local_error)
    {
        rollback_deleted_account_side_effects(
            state,
            &credentials,
            account_id,
            old_credential.as_ref(),
            previous_quota_refresh,
            previous_wake,
            old_automations,
            &restored_bindings,
            previous_proxy_pool.as_ref(),
            &error,
        )?;
        return Err(error.into());
    }
    match state.remove_quota_refresh(account_id) {
        Ok(_) => {}
        Err(error) => {
            rollback_deleted_account_side_effects(
                state,
                &credentials,
                account_id,
                old_credential.as_ref(),
                previous_quota_refresh,
                previous_wake,
                old_automations,
                &restored_bindings,
                previous_proxy_pool.as_ref(),
                &error,
            )?;
            return Err(error.into());
        }
    }
    if let Err(error) = state
        .store()?
        .delete_account_state(account_id, accounts, keys, automations)
    {
        rollback_deleted_account_side_effects(
            state,
            &credentials,
            account_id,
            old_credential.as_ref(),
            previous_quota_refresh,
            previous_wake,
            old_automations,
            &restored_bindings,
            previous_proxy_pool.as_ref(),
            &error,
        )?;
        return Err(error.into());
    }
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.remove_candidate(account_id);
    }
    state.token_authority().remove(account_id);
    state.remove_quota_account_lock(account_id)?;
    Ok(())
}

pub(super) fn release_account_proxy(account_id: &str) -> LocalResult<Option<ProxyPool>> {
    let previous = ProxyPool::load()?;
    let mut next = previous.clone();
    next.release(account_id);
    if next == previous {
        return Ok(None);
    }
    next.save()?;
    Ok(Some(previous))
}

pub(super) fn current_account_records(
    state: &DesktopState,
) -> LocalResult<(Vec<LocalAccountRecord>, Vec<LocalGatewayKeyRecord>)> {
    let store = state.store()?;
    Ok((store.accounts().to_vec(), store.keys().to_vec()))
}

pub(super) fn current_account_state(
    state: &DesktopState,
) -> LocalResult<(
    Vec<LocalAccountRecord>,
    Vec<LocalGatewayKeyRecord>,
    AutomationRecords,
)> {
    let store = state.store()?;
    Ok((
        store.accounts().to_vec(),
        store.keys().to_vec(),
        store.automations().clone(),
    ))
}

pub(super) fn prune_account_task_selectors(
    mut automations: AutomationRecords,
    account_id: &str,
) -> AutomationRecords {
    let now_ms = current_time_ms();
    automations.tasks.retain_mut(|task| {
        let AccountSelector::AccountIds(account_ids) = &mut task.account_selector else {
            return true;
        };
        if !account_ids.remove(account_id) {
            return true;
        }
        task.updated_at_ms = now_ms;
        !account_ids.is_empty()
    });
    automations
}

pub(super) fn restore_credential_item(
    credential_store: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
    old_credential: Option<&StoredCodexCredentials>,
) -> ItemResult<()> {
    match old_credential {
        Some(credentials) => credential_store.save(credentials),
        None => credential_store.delete(account_id),
    }
    .map_err(|_| ImportItemError::recovery("failed to restore previous account credentials"))
}

pub(super) fn restore_credential_local(
    credential_store: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
    old_credential: Option<&StoredCodexCredentials>,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    let restored = match old_credential {
        Some(credentials) => credential_store.save(credentials),
        None => Ok(()),
    };
    restored.map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; failed to restore previous account credentials",
                cause.message
            ),
        )
    })?;
    if old_credential.is_none() {
        let _ = account_id;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rollback_deleted_account_side_effects(
    state: &DesktopState,
    credential_store: &CredentialStore<NativeSecretBackend>,
    account_id: &str,
    old_credential: Option<&StoredCodexCredentials>,
    previous_quota_refresh: zenith_relay_core::quota::QuotaRefreshQueue,
    previous_wake: zenith_relay_core::automations::WakeCoordinator,
    old_automations: AutomationRecords,
    restored_bindings: &[codex::ProfileBinding],
    previous_proxy_pool: Option<&ProxyPool>,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    restore_credential_local(credential_store, account_id, old_credential, cause)?;
    state
        .restore_quota_refresh(previous_quota_refresh)
        .map_err(|error| recovery_after_delete(cause, "quota schedule", error))?;
    state
        .restore_wake(previous_wake, old_automations)
        .map_err(|error| recovery_after_delete(cause, "wake state", error))?;
    reattach_account_profiles(state, restored_bindings, old_credential, cause)?;
    if let Some(pool) = previous_proxy_pool {
        pool.save()
            .map_err(|error| recovery_after_delete(cause, "proxy assignment", error))?;
    }
    Ok(())
}

pub(super) fn restore_bound_account_profiles(
    state: &DesktopState,
    bindings: &[codex::ProfileBinding],
    credentials: Option<&StoredCodexCredentials>,
) -> LocalResult<Vec<codex::ProfileBinding>> {
    let mut restored = Vec::with_capacity(bindings.len());
    for binding in bindings {
        match codex::restore_account_profile(
            Path::new(&binding.profile_dir),
            &state.profile_backup_root(),
        ) {
            Ok(Some(binding)) => restored.push(binding),
            Ok(None) => {
                let error = LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "account profile binding disappeared during deletion",
                );
                reattach_account_profiles(state, &restored, credentials, &error)?;
                return Err(error);
            }
            Err(error) => {
                reattach_account_profiles(state, &restored, credentials, &error)?;
                return Err(error);
            }
        }
    }
    Ok(restored)
}

pub(super) fn reattach_account_profiles(
    state: &DesktopState,
    bindings: &[codex::ProfileBinding],
    credentials: Option<&StoredCodexCredentials>,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    if bindings.is_empty() {
        return Ok(());
    }
    let credentials = credentials.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("{}; account profile credentials are missing", cause.message),
        )
    })?;
    let tokens = credentials.to_token_set().map_err(|_| {
        LocalPoolError::new(ErrorCode::RecoveryRequired, "account tokens are invalid")
    })?;
    let provider_account_id = credentials.provider_account_id().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "account provider identity is missing",
        )
    })?;
    for binding in bindings {
        codex::attach_account(
            Path::new(&binding.profile_dir),
            &state.profile_backup_root(),
            &binding.credential_id,
            &tokens,
            provider_account_id,
        )
        .map_err(|error| recovery_after_delete(cause, "profile binding", error))?;
    }
    Ok(())
}

pub(super) fn recovery_after_delete(
    cause: &LocalPoolError,
    state: &str,
    error: LocalPoolError,
) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        format!(
            "{}; failed to restore account {state}: {}",
            cause.message, error.message
        ),
    )
}

pub(super) async fn repair_gateway_after_item_restore(
    state: &DesktopState,
    old_accounts: Vec<LocalAccountRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
) -> ItemResult<()> {
    sync_accounts_or_rollback(state, old_accounts, old_keys)
        .await
        .map_err(|_| {
            ImportItemError::recovery("failed to rebuild gateway after credential rollback")
        })
}
