use super::super::import_orchestrator::{apply_account_patch, validate_account_record};
use super::{SetAccountProxyInput, UpdateAccountInput};
use crate::local_pool::commands::{
    apply_account_policy_if_running, current_time_ms, fence_runtime_candidates,
    refresh_active_codex_catalog_in_background, refresh_local_gateway_key_scope_if_running,
    sync_account_or_rollback,
};
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError};
use crate::local_pool::models::{LocalAccountRecord, LocalPoolSnapshot};
use crate::local_pool::state::DesktopState;
use tauri::{AppHandle, State};
use zenith_relay_core::{
    pool_catalog_visibility_changed, pool_dispatch_permission_changed, PoolParticipant,
};

type CommandResult<T> = std::result::Result<T, CommandError>;

#[tauri::command]
pub async fn update_local_account(
    input: UpdateAccountInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let account_update = input;
    let _mutation = state.setup_guard().await;
    let account_id = account_update.account_id.clone();
    let mut account = state
        .store()?
        .account(&account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))?;
    let previous_account = account.clone();
    apply_account_patch(&mut account, account_update)?;
    validate_account_record(&account)?;
    let catalog_changed = account_catalog_visibility_changed(&previous_account, &account);
    let model_refresh_account = (!previous_account.account.in_pool && account.account.in_pool)
        .then(|| account.account.id.clone());
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = if account_dispatch_permission_changed(&previous_account, &account) {
        fence_runtime_candidates(runtime.as_deref(), std::slice::from_ref(&account_id), &[])
    } else {
        Vec::new()
    };
    state.store()?.upsert_account(account.clone())?;
    let membership_changed = previous_account.account.in_pool != account.account.in_pool;
    let updated_in_place = if apply_account_policy_if_running(&state, &account).await {
        !membership_changed
            || refresh_local_gateway_key_scope_if_running(&state)
                .await
                .unwrap_or(false)
    } else {
        false
    };
    if !updated_in_place {
        sync_account_or_rollback(&state, previous_account, account.clone()).await?;
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
    previous_account: &LocalAccountRecord,
    updated_account: &LocalAccountRecord,
) -> bool {
    pool_catalog_visibility_changed(
        previous_account.pool_access(),
        updated_account.pool_access(),
    )
}

fn account_dispatch_permission_changed(
    previous_account: &LocalAccountRecord,
    updated_account: &LocalAccountRecord,
) -> bool {
    pool_dispatch_permission_changed(
        previous_account.pool_access(),
        updated_account.pool_access(),
    )
}

#[tauri::command]
pub async fn set_local_account_proxy(
    input: SetAccountProxyInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let proxy_update = input;
    let _mutation = state.setup_guard().await;
    crate::local_pool::commands::proxies::set_account_proxy_inner(
        proxy_update.account_id,
        proxy_update.proxy_url,
        proxy_update.bypass_common_proxy,
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
    let previous_account = account.clone();
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
        sync_account_or_rollback(&state, previous_account, account.clone()).await?;
    }
    state.sync_account_quota_refresh(&account_id, current_time_ms())?;
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    if updated_in_place && catalog_changed {
        refresh_active_codex_catalog_in_background(app);
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::accounts::{credentials::StoredCodexCredentials, records};
    use zenith_relay_core::accounts::AccountAuthMode;

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
}
