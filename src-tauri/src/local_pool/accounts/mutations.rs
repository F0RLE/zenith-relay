use super::authority::{
    ProcessAccountGuard, ProcessAccountLocks, ProcessLockConfig, ProcessLockError,
};
use super::import_orchestrator::{
    apply_account_patch, credential_local_error, validate_account_record,
};
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::exports::normalize_account_ids;
use crate::local_pool::accounts::proxy::ProxyPool;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::{
    apply_account_policy_if_running, current_time_ms, fail_closed, fence_runtime_candidates,
    refresh_active_codex_catalog_in_background, refresh_local_gateway_key_scope_if_running,
    sync_account_or_rollback,
};
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::{
    AutomationRecords, LocalAccountRecord, LocalGatewayKeyRecord, LocalPoolSnapshot,
};
use crate::local_pool::profiles::codex;
use crate::local_pool::state::DesktopState;
use serde::Deserialize;
use std::path::Path;
use tauri::{AppHandle, State};
use zenith_relay_core::automations::AccountSelector;

type CommandResult<T> = std::result::Result<T, CommandError>;

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
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let account_id = input.account_id.clone();
    let mut account = state
        .store()?
        .account(&account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    let previous = account.clone();
    apply_account_patch(&mut account, input)?;
    validate_account_record(&account)?;
    let catalog_changed = account_catalog_visibility_changed(&previous, &account);
    let model_refresh_account =
        (!previous.account.in_pool && account.account.in_pool).then(|| account.account.id.clone());
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = if account_dispatch_permission_changed(&previous, &account) {
        fence_runtime_candidates(runtime.as_deref(), std::slice::from_ref(&account_id), &[])
    } else {
        Vec::new()
    };
    state.store()?.upsert_account(account.clone())?;
    let membership_changed = previous.account.in_pool != account.account.in_pool;
    let updated_in_place = if apply_account_policy_if_running(&state, &account).await {
        !membership_changed
            || refresh_local_gateway_key_scope_if_running(&state)
                .await
                .unwrap_or(false)
    } else {
        false
    };
    if !updated_in_place {
        sync_account_or_rollback(&state, previous, account.clone()).await?;
    }
    state.sync_account_quota_refresh(&account_id, current_time_ms())?;
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    if updated_in_place && catalog_changed {
        refresh_active_codex_catalog_in_background(app.clone());
    }
    if let Some(account_id) = model_refresh_account {
        crate::local_pool::background::refresh_account_models_in_background(app, vec![account_id]);
    }
    Ok(snapshot)
}

fn account_catalog_visibility_changed(
    previous: &LocalAccountRecord,
    current: &LocalAccountRecord,
) -> bool {
    (previous.account.in_pool || current.account.in_pool)
        && (previous.account.in_pool != current.account.in_pool
            || previous.account.enabled != current.account.enabled
            || previous.account.draining != current.account.draining
            || previous.allowed_models != current.allowed_models
            || previous.excluded_models != current.excluded_models)
}

fn account_dispatch_permission_changed(
    previous: &LocalAccountRecord,
    current: &LocalAccountRecord,
) -> bool {
    previous.account.enabled != current.account.enabled
        || previous.account.in_pool != current.account.in_pool
        || previous.account.draining != current.account.draining
        || previous.allowed_models != current.allowed_models
        || previous.excluded_models != current.excluded_models
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
    app: AppHandle,
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
    let previous = account.clone();
    account.account.enabled = enabled;
    if enabled {
        validate_account_record(&account)?;
    }
    let catalog_changed = account.account.in_pool;
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences =
        fence_runtime_candidates(runtime.as_deref(), std::slice::from_ref(&account_id), &[]);
    state.store()?.upsert_account(account.clone())?;
    let updated_in_place = apply_account_policy_if_running(&state, &account).await;
    if !updated_in_place {
        sync_account_or_rollback(&state, previous, account.clone()).await?;
    }
    state.sync_account_quota_refresh(&account_id, current_time_ms())?;
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    if updated_in_place && catalog_changed {
        refresh_active_codex_catalog_in_background(app);
    }
    Ok(snapshot)
}

#[tauri::command]
pub async fn delete_local_account(
    account_id: String,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    ensure_accounts_not_in_ownership_operation(&state, std::slice::from_ref(&account_id))?;
    delete_local_account_inner(&account_id, &state).await?;
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    refresh_active_codex_catalog_in_background(app);
    Ok(snapshot)
}

#[tauri::command]
pub async fn delete_local_accounts(
    account_ids: Vec<String>,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let account_ids = normalize_account_ids(account_ids)?;
    let _mutation = state.setup_guard().await;
    ensure_accounts_not_in_ownership_operation(&state, &account_ids)?;
    let existing_accounts = state.store()?.accounts().to_vec();
    ensure_accounts_exist(&existing_accounts, &account_ids)?;
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = fence_runtime_candidates(runtime.as_deref(), &account_ids, &[]);
    let ids = account_ids.iter().map(String::as_str).collect::<Vec<_>>();
    let _credential_guards = acquire_delete_credential_guards(&state, &ids).await?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let initial_wake = state.wake_snapshot()?;
    let initial_automations = state.store()?.automations().clone();
    let initial_proxy_pool = ProxyPool::load()?;
    let old_keys = state.store()?.keys().to_vec();
    let mut automations = initial_automations.clone();
    for account_id in &account_ids {
        automations = prune_account_task_selectors(automations, account_id);
    }
    let mut prepared = Vec::with_capacity(account_ids.len());
    for account_id in &account_ids {
        match prepare_delete_local_account(account_id, &state, &credentials).await {
            Ok(deleted) => prepared.push(deleted),
            Err(error) => {
                ensure_delete_rollback_or_fail_closed(
                    &state,
                    rollback_batch_delete(
                        &state,
                        &credentials,
                        &prepared,
                        initial_wake,
                        initial_automations,
                        &initial_proxy_pool,
                        &error,
                    ),
                )
                .await?;
                return Err(error.into());
            }
        }
    }
    let accounts = existing_accounts
        .iter()
        .filter(|account| !account_ids.contains(&account.account.id))
        .cloned()
        .collect::<Vec<_>>();
    let delete_result = state.store().and_then(|mut store| {
        store.delete_accounts_state(&account_ids, accounts, old_keys, automations)
    });
    if let Err(error) = delete_result {
        ensure_delete_rollback_or_fail_closed(
            &state,
            rollback_batch_delete(
                &state,
                &credentials,
                &prepared,
                initial_wake,
                initial_automations,
                &initial_proxy_pool,
                &error,
            ),
        )
        .await?;
        return Err(error.into());
    }
    if let Some(runtime) = state.gateway.runtime().await {
        for account_id in &account_ids {
            runtime.remove_candidate(account_id);
        }
    }
    for account_id in &account_ids {
        state.token_authority().remove(account_id);
        let _ = state.remove_quota_account_lock(account_id);
    }
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    refresh_active_codex_catalog_in_background(app);
    Ok(snapshot)
}

pub(super) fn ensure_accounts_exist(
    accounts: &[LocalAccountRecord],
    account_ids: &[String],
) -> LocalResult<()> {
    if let Some(account_id) = account_ids.iter().find(|account_id| {
        !accounts
            .iter()
            .any(|account| account.account.id == **account_id)
    }) {
        return Err(LocalPoolError::new(
            ErrorCode::NotFound,
            format!("account not found: {account_id}"),
        ));
    }
    Ok(())
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
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences =
        fence_runtime_candidates(runtime.as_deref(), &[account_id.to_string()], &[]);
    let _credential_guards = acquire_delete_credential_guards(state, &[account_id]).await?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let (old_accounts, old_keys, old_automations) = current_account_state(state)?;
    let deleted = prepare_delete_local_account(account_id, state, &credentials).await?;
    let accounts = old_accounts
        .iter()
        .filter(|account| account.account.id != account_id)
        .cloned()
        .collect::<Vec<_>>();
    let automations = prune_account_task_selectors(old_automations, account_id);
    let delete_result = state.store().and_then(|mut store| {
        store.delete_account_state(account_id, accounts, old_keys, automations)
    });
    if let Err(error) = delete_result {
        ensure_delete_rollback_or_fail_closed(
            state,
            rollback_prepared_delete(state, &credentials, &deleted, &error),
        )
        .await?;
        return Err(error.into());
    }
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.remove_candidate(account_id);
    }
    state.token_authority().remove(account_id);
    let _ = state.remove_quota_account_lock(account_id);
    Ok(())
}

/// A token refresh owns this process lock until its secret write finishes.
/// Acquire it before any delete side effects and keep it through rollback or
/// authority retirement, so an old refresh cannot recreate a deleted secret.
async fn acquire_delete_credential_guards(
    state: &DesktopState,
    account_ids: &[&str],
) -> LocalResult<Vec<ProcessAccountGuard>> {
    let locks =
        ProcessAccountLocks::with_config(state.transient_root(), ProcessLockConfig::default())
            .map_err(|_| {
                LocalPoolError::invalid_state("account credential locks are unavailable")
            })?;
    let mut ids = account_ids.to_vec();
    ids.sort_unstable();
    let mut guards = Vec::with_capacity(ids.len());
    for id in ids {
        guards.push(locks.acquire(id).await.map_err(|error| {
            let code = if error == ProcessLockError::Timeout {
                ErrorCode::Conflict
            } else {
                ErrorCode::Io
            };
            LocalPoolError::new(
                code,
                "account credential lock is unavailable; retry deletion",
            )
        })?);
    }
    Ok(guards)
}

struct PreparedAccountDelete {
    account_id: String,
    old_credential: Option<StoredCodexCredentials>,
    previous_wake: zenith_relay_core::automations::WakeCoordinator,
    old_automations: AutomationRecords,
    restored_bindings: Vec<codex::ProfileBinding>,
    previous_proxy_pool: Option<ProxyPool>,
}

async fn prepare_delete_local_account(
    account_id: &str,
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
) -> LocalResult<PreparedAccountDelete> {
    let old_accounts = state.store()?.accounts().to_vec();
    if !old_accounts
        .iter()
        .any(|account| account.account.id == account_id)
    {
        return Err(LocalPoolError::new(
            ErrorCode::NotFound,
            "account not found",
        ));
    }
    let old_credential = credentials
        .load(account_id)
        .map_err(credential_local_error)?;
    let old_automations = state.store()?.automations().clone();
    let previous_wake = state.wake_snapshot()?;
    let bindings = codex::account_bindings(&state.profile_backup_root())?
        .into_iter()
        .filter(|binding| binding.credential_id == account_id)
        .collect::<Vec<_>>();
    if !bindings.is_empty() && old_credential.is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "account profile binding exists without stored credentials",
        ));
    }
    state.store()?.invalidate_account_refresh(&[account_id])?;
    state.remove_account_refresh(account_id);
    let restored_bindings =
        match restore_bound_account_profiles(state, &bindings, old_credential.as_ref()) {
            Ok(restored) => restored,
            // A failed profile reattach may leave credentials and the saved route
            // out of sync. A recovery error must not reopen the old runtime.
            Err(error) if error.code == ErrorCode::RecoveryRequired => {
                return Err(fail_closed(state, error.to_string()).await);
            }
            Err(error) => return Err(error),
        };
    if let Err(error) = state.remove_pending_wakes_for_account(account_id) {
        ensure_delete_rollback_or_fail_closed(
            state,
            rollback_deleted_account_side_effects(
                state,
                credentials,
                account_id,
                old_credential.as_ref(),
                previous_wake,
                old_automations,
                &restored_bindings,
                None,
                &error,
            ),
        )
        .await?;
        return Err(error);
    }
    let previous_proxy_pool = match release_account_proxy(account_id) {
        Ok(previous) => previous,
        Err(error) => {
            ensure_delete_rollback_or_fail_closed(
                state,
                rollback_deleted_account_side_effects(
                    state,
                    credentials,
                    account_id,
                    old_credential.as_ref(),
                    previous_wake,
                    old_automations,
                    &restored_bindings,
                    None,
                    &error,
                ),
            )
            .await?;
            return Err(error);
        }
    };
    if let Err(error) = credentials
        .delete(account_id)
        .map_err(credential_local_error)
    {
        ensure_delete_rollback_or_fail_closed(
            state,
            rollback_deleted_account_side_effects(
                state,
                credentials,
                account_id,
                old_credential.as_ref(),
                previous_wake,
                old_automations,
                &restored_bindings,
                previous_proxy_pool.as_ref(),
                &error,
            ),
        )
        .await?;
        return Err(error);
    }
    Ok(PreparedAccountDelete {
        account_id: account_id.to_string(),
        old_credential,
        previous_wake,
        old_automations,
        restored_bindings,
        previous_proxy_pool,
    })
}

/// Keep the dispatch fence held until an incomplete delete rollback has
/// retired the old runtime and stopped its listener. A failed credential or
/// profile restore must never turn into a newly dispatchable old account.
async fn ensure_delete_rollback_or_fail_closed(
    state: &DesktopState,
    rollback: LocalResult<()>,
) -> LocalResult<()> {
    match rollback {
        Ok(()) => Ok(()),
        Err(error) => Err(fail_closed(state, error.to_string()).await),
    }
}

fn rollback_prepared_delete(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
    deleted: &PreparedAccountDelete,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    rollback_deleted_account_side_effects(
        state,
        credentials,
        &deleted.account_id,
        deleted.old_credential.as_ref(),
        deleted.previous_wake.clone(),
        deleted.old_automations.clone(),
        &deleted.restored_bindings,
        deleted.previous_proxy_pool.as_ref(),
        cause,
    )
}

#[allow(clippy::too_many_arguments)]
fn rollback_batch_delete(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
    deleted: &[PreparedAccountDelete],
    previous_wake: zenith_relay_core::automations::WakeCoordinator,
    old_automations: AutomationRecords,
    previous_proxy_pool: &ProxyPool,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    for account in deleted.iter().rev() {
        restore_credential_local(
            credentials,
            &account.account_id,
            account.old_credential.as_ref(),
            cause,
        )?;
        reattach_account_profiles(
            state,
            &account.restored_bindings,
            account.old_credential.as_ref(),
            cause,
        )?;
    }
    state.store()?.notify_refresh_changed();
    state
        .restore_wake(previous_wake, old_automations)
        .map_err(|error| recovery_after_delete(cause, "wake state", error))?;
    previous_proxy_pool
        .save()
        .map_err(|error| recovery_after_delete(cause, "proxy assignment", error))?;
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
    previous_wake: zenith_relay_core::automations::WakeCoordinator,
    old_automations: AutomationRecords,
    restored_bindings: &[codex::ProfileBinding],
    previous_proxy_pool: Option<&ProxyPool>,
    cause: &LocalPoolError,
) -> LocalResult<()> {
    restore_credential_local(credential_store, account_id, old_credential, cause)?;
    state.store()?.notify_refresh_changed();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::accounts::{credentials::StoredCodexCredentials, records};
    use std::sync::Arc;
    use zenith_relay_core::accounts::AccountAuthMode;
    use zenith_relay_core::{GatewayRuntime, LocalGatewayKey, ProviderSource, WireApi};

    fn account_record() -> LocalAccountRecord {
        let credentials = StoredCodexCredentials::new(
            "account",
            "access-private".into(),
            Some("refresh-private".into()),
            None,
            None,
            1,
            0,
            None,
            Some("provider-private".into()),
            None,
            None,
            None,
            false,
        )
        .expect("test credentials");
        records::new_account_record(
            &credentials,
            AccountAuthMode::OAuth,
            vec!["gpt-test".into()],
            0,
            1,
        )
        .expect("test account")
    }

    #[test]
    fn account_catalog_refreshes_for_pool_membership_changes() {
        let mut inside = account_record();
        inside.account.in_pool = true;
        let mut outside = inside.clone();
        outside.account.in_pool = false;

        assert!(account_catalog_visibility_changed(&inside, &outside));
        assert!(account_catalog_visibility_changed(&outside, &inside));
    }

    #[tokio::test]
    async fn incomplete_delete_rollback_closes_the_old_gateway() {
        let root = std::env::temp_dir().join(format!(
            "relay-delete-fail-closed-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let state = DesktopState::open(root.clone()).unwrap();
        let runtime = Arc::new(
            GatewayRuntime::new(
                ProviderSource {
                    id: "synthetic-source".into(),
                    name: "Synthetic".into(),
                    base_url: "http://127.0.0.1:9/v1".into(),
                    api_key: "synthetic-upstream".into(),
                    wire_api: WireApi::Responses,
                    models: vec!["gpt-test".into()],
                },
                LocalGatewayKey {
                    id: "synthetic-key".into(),
                    secret: "synthetic-local".into(),
                },
                Arc::new(|_| {}),
            )
            .unwrap(),
        );
        state.gateway.start(runtime.clone(), 0).await.unwrap();
        state.store().unwrap().set_gateway_enabled(true).unwrap();
        let _dispatch_fences =
            fence_runtime_candidates(Some(&runtime), &[], &["synthetic-source".into()]);
        assert!(!_dispatch_fences.is_empty());

        let deleted = PreparedAccountDelete {
            account_id: "synthetic-account".into(),
            old_credential: None,
            previous_wake: state.wake_snapshot().unwrap(),
            old_automations: state.store().unwrap().automations().clone(),
            restored_bindings: vec![codex::ProfileBinding {
                profile_dir: root.join("profile").to_string_lossy().into_owned(),
                credential_kind: codex::ProfileCredentialKind::OAuthAccount,
                credential_id: "synthetic-account".into(),
                bound_oauth_account_id: None,
                active: true,
            }],
            previous_proxy_pool: None,
        };
        let cause = LocalPoolError::new(ErrorCode::Io, "injected deletion failure");
        let rollback = rollback_prepared_delete(
            &state,
            &CredentialStore::from_backend(NativeSecretBackend),
            &deleted,
            &cause,
        );
        assert_eq!(
            rollback.as_ref().unwrap_err().code,
            ErrorCode::RecoveryRequired
        );
        assert!(state.gateway.runtime().await.is_some());
        let error = ensure_delete_rollback_or_fail_closed(&state, rollback)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::RecoveryRequired);
        assert!(state.gateway.runtime().await.is_none());
        assert!(!state.store().unwrap().gateway().enabled);

        drop(_dispatch_fences);
        drop(runtime);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn delete_waits_for_the_same_credential_lock_as_refresh() {
        use futures_util::poll;
        use std::task::Poll;

        let root = std::env::temp_dir().join(format!(
            "relay-delete-credential-lock-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let state = DesktopState::open(root.clone()).unwrap();
        let locks =
            ProcessAccountLocks::with_config(state.transient_root(), ProcessLockConfig::default())
                .unwrap();
        let held = locks.acquire("synthetic_account").await.unwrap();
        let mut deleting = Box::pin(acquire_delete_credential_guards(
            &state,
            &["synthetic_account"],
        ));
        assert!(matches!(poll!(deleting.as_mut()), Poll::Pending));
        drop(held);
        let guards = tokio::time::timeout(std::time::Duration::from_secs(2), deleting)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(guards.len(), 1);
        drop(guards);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
