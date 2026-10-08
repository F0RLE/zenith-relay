use super::{fence_runtime_candidates, restart_or_rollback, runtime_account_policy};
use crate::{
    local_pool::{
        accounts::{credentials::CredentialStore, NativeSecretBackend},
        error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
        models::LocalPoolSnapshot,
        profiles::codex,
        state::DesktopState,
        store::secret_store,
    },
    platform::default_codex_home,
};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};
use zenith_relay_core::{
    protocol::{
        ConfigurationPreset, ConfigurationPresetApplyInput, ConfigurationPresetApplyResult,
        ConfigurationPresetPreview,
    },
    DefaultServiceTier,
};

mod model_policy;
mod model_settings;
mod reasoning;

pub(super) use model_policy::{canonical_pool_model, local_pool_member_ids};
use reasoning::SetModelReasoningInput;
mod gateway_keys;
mod membership;
mod presets;
pub(super) use membership::apply_local_pool_membership;

pub(crate) use gateway_keys::retire_user_gateway_keys;
pub(in crate::local_pool) use gateway_keys::SYSTEM_GATEWAY_KEY_ID;
pub(super) use gateway_keys::{
    ensure_local_gateway_key_secret, ensure_system_gateway_key, new_local_gateway_api_key,
};

type CommandResult<T> = std::result::Result<T, CommandError>;

#[tauri::command]
pub async fn set_local_model_reasoning(
    input: SetModelReasoningInput,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    reasoning::set_local_model_reasoning(input, state).await
}

#[tauri::command]
pub fn export_local_configuration_preset(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<Option<String>> {
    presets::export_local_configuration_preset(app, state)
}

#[tauri::command]
pub fn preview_local_configuration_preset(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<Option<ConfigurationPresetPreview>> {
    presets::preview_local_configuration_preset(app, state)
}

#[tauri::command]
pub async fn apply_local_configuration_preset(
    input: ConfigurationPresetApplyInput,
    state: State<'_, DesktopState>,
) -> CommandResult<ConfigurationPresetApplyResult> {
    presets::apply_local_configuration_preset(input, state).await
}

pub(super) fn write_configuration_preset(
    preset: &ConfigurationPreset,
    app: &AppHandle,
) -> CommandResult<Option<String>> {
    presets::write_configuration_preset(preset, app)
}

pub(crate) fn has_usable_pool_candidate(state: &DesktopState) -> LocalResult<bool> {
    let store = state.store()?;
    for source in store.sources() {
        if source.in_pool
            && source.enabled
            && !source.draining
            && source.supports_any_wire_api().unwrap_or(false)
            && secret_store::load(&source.secret_ref)?.is_some()
        {
            return Ok(true);
        }
    }
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    for account in store.accounts() {
        if account.account.in_pool
            && account.account.enabled
            && !account.account.draining
            && credentials
                .load(&account.account.id)
                .map_err(|error| {
                    LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error.to_string())
                })?
                .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRoutingInput {
    pool_routing: Option<zenith_relay_core::PoolRoutingPolicy>,
    expected_pool_routing: Option<zenith_relay_core::PoolRoutingPolicy>,
    #[serde(default)]
    basis_points_enabled: Option<bool>,
    max_retry_candidates: u8,
    #[serde(default)]
    default_service_tier: DefaultServiceTier,
}

pub(crate) use zenith_relay_core::protocol::PoolMembershipInput;

use model_settings::{
    SetModelDisplayOrderInput, SetModelEnabledInput, SetModelPriceInput, SetModelServiceTierInput,
};

#[tauri::command]
pub async fn set_local_model_enabled(
    input: SetModelEnabledInput,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    model_settings::set_local_model_enabled(input, state).await
}

#[tauri::command]
pub async fn set_local_model_price(
    input: SetModelPriceInput,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    model_settings::set_local_model_price(input, state).await
}

#[tauri::command]
pub async fn set_local_model_service_tier(
    input: SetModelServiceTierInput,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    model_settings::set_local_model_service_tier(input, state).await
}

#[tauri::command]
pub async fn set_local_model_display_order(
    input: SetModelDisplayOrderInput,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    model_settings::set_local_model_display_order(input, state).await
}

#[tauri::command]
pub async fn set_local_pool_membership(
    input: PoolMembershipInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let (snapshot, updated_in_place, model_refresh_account_ids) = {
        let _mutation = state.setup_guard().await;
        apply_local_pool_membership(input, &state).await?
    };
    if updated_in_place {
        let catalog_app = app.clone();
        tauri::async_runtime::spawn(async move {
            let state = catalog_app.state::<DesktopState>();
            let refresh_result = super::profiles::refresh_active_client_catalogs(&state).await;
            super::record_catalog_refresh_result(&state, &refresh_result);
            let _ = catalog_app.emit("zenith-state-changed", ());
        });
    }
    crate::local_pool::background::refresh_account_models_in_background(
        app,
        model_refresh_account_ids,
    );
    Ok(snapshot)
}

#[tauri::command]
pub async fn update_local_routing(
    input: UpdateRoutingInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    update_local_routing_at(input, &state, &default_codex_home()).await
}

async fn update_local_routing_at(
    input: UpdateRoutingInput,
    state: &DesktopState,
    codex_home: &std::path::Path,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let (old_gateway, current_pool) = {
        let store = state.store()?;
        // The UI reads the policy reconciled with inventory, including before
        // the first save. Compare that same policy, not the stale stored list.
        (
            store.gateway().clone(),
            store
                .gateway()
                .pool_routing_for(store.sources(), store.accounts()),
        )
    };
    let mut gateway = old_gateway.clone();
    gateway.max_retry_candidates = input.max_retry_candidates;
    gateway.pool_routing = Some(current_pool.clone());
    if let Some(policy) = input.pool_routing {
        policy
            .validate_update(&current_pool, input.expected_pool_routing.as_ref())
            .map_err(|message| LocalPoolError::new(ErrorCode::Conflict, message))?;
        gateway.pool_routing = Some(policy);
    }
    if let Some(basis_points_enabled) = input.basis_points_enabled {
        gateway.basis_points_enabled = basis_points_enabled;
    }
    gateway.default_service_tier = input.default_service_tier;
    if gateway == old_gateway {
        codex::sync_default_service_tier(codex_home, gateway.default_service_tier)?;
        return state.snapshot().await.map_err(Into::into);
    }
    let default_service_tier = gateway.default_service_tier;
    state.store()?.replace_gateway(gateway.clone())?;
    let runtime = state.gateway.runtime().await;
    if let Some(runtime) = &runtime {
        if let Err(error) = runtime.set_pool_routing_policy(
            gateway
                .pool_routing
                .clone()
                .unwrap_or_else(|| current_pool.clone()),
            gateway.max_retry_candidates,
        ) {
            state.store()?.replace_gateway(old_gateway)?;
            return Err(LocalPoolError::invalid_state(error).into());
        }
        runtime.set_basis_points_enabled(gateway.basis_points_enabled);
        runtime.set_default_service_tier(default_service_tier);
    }
    if let Err(error) = codex::sync_default_service_tier(codex_home, default_service_tier) {
        state.store()?.replace_gateway(old_gateway.clone())?;
        if let Some(runtime) = runtime {
            runtime
                .set_pool_routing_policy(current_pool, old_gateway.max_retry_candidates)
                .map_err(LocalPoolError::invalid_state)?;
            runtime.set_basis_points_enabled(old_gateway.basis_points_enabled);
            runtime.set_default_service_tier(old_gateway.default_service_tier);
        }
        return Err(error.into());
    }
    state.snapshot().await.map_err(Into::into)
}

#[cfg(test)]
mod tests;
