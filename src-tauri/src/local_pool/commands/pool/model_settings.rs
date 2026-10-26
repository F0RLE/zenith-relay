use super::model_policy::{configured_pool_model_ids, model_policy_error};
use super::CommandResult;
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError},
    state::DesktopState,
};
use tauri::{AppHandle, State};
use zenith_relay_core::{protocol::complete_model_display_order, DefaultServiceTier};

pub(super) use zenith_relay_core::protocol::{
    SetModelContextInput, SetModelEnabledInput, SetModelOrderInput as SetModelDisplayOrderInput,
    SetModelServiceTierInput,
};

pub(super) async fn set_local_model_enabled(
    input: SetModelEnabledInput,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let _mutation = state.setup_guard().await;
    let canonical = super::canonical_pool_model(&state, &input.model_id)?;
    let mut gateway = state.store()?.gateway().clone();
    let previous_hidden_models = gateway.hidden_models.clone();
    gateway
        .hidden_models
        .retain(|model| !model.eq_ignore_ascii_case(&canonical));
    if !input.enabled {
        gateway.hidden_models.push(canonical);
    }
    if gateway.hidden_models == previous_hidden_models {
        return Ok(());
    }
    let hidden = gateway.hidden_models.clone();
    state.store()?.replace_gateway(gateway)?;
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.set_hidden_models(hidden);
    }
    Ok(())
}

pub(super) async fn set_local_model_service_tier(
    input: SetModelServiceTierInput,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let _mutation = state.setup_guard().await;
    let canonical = super::canonical_pool_model(&state, &input.model_id)?;
    let runtime = state.gateway.runtime().await;
    if input.service_tier != DefaultServiceTier::Standard
        && !super::super::state::build_local_runtime_state(&state)
            .await?
            .gateway
            .models
            .iter()
            .any(|model| {
                model.id.eq_ignore_ascii_case(&canonical)
                    && model.speed_tiers.contains(&input.service_tier)
            })
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "requested service tier is not supported by the model and configured routes",
        )
        .into());
    }
    let old_gateway = state.store()?.gateway().clone();
    let mut gateway = old_gateway.clone();
    let key = zenith_relay_core::model_id_key(&canonical);
    gateway
        .model_service_tier_overrides
        .insert(key, input.service_tier);
    if gateway == old_gateway {
        return Ok(());
    }
    state.store()?.replace_gateway(gateway.clone())?;
    if let Some(runtime) = runtime {
        if let Err(error) =
            runtime.set_model_service_tier_overrides(gateway.model_service_tier_overrides)
        {
            state.store()?.replace_gateway(old_gateway)?;
            return Err(LocalPoolError::invalid_state(error).into());
        }
    }
    Ok(())
}

pub(super) async fn set_local_model_context(
    input: SetModelContextInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    input
        .context_policy
        .validate()
        .map_err(LocalPoolError::invalid_state)?;
    let _mutation = state.setup_guard().await;
    let canonical = super::canonical_pool_model(&state, &input.model_id)?;
    let key = zenith_relay_core::model_id_key(&canonical);
    let old_gateway = state.store()?.gateway().clone();
    let mut gateway = old_gateway.clone();
    gateway
        .model_context_policies
        .insert(key, input.context_policy);
    if gateway != old_gateway {
        state.store()?.replace_gateway(gateway.clone())?;
        if let Some(runtime) = state.gateway.runtime().await {
            if let Err(error) =
                runtime.set_model_context_policies(gateway.model_context_policies.clone())
            {
                state.store()?.replace_gateway(old_gateway)?;
                return Err(LocalPoolError::invalid_state(error).into());
            }
        }
    }
    drop(_mutation);

    // Catalog-backed clients read context fields from their managed model list.
    // Refresh that projection after the runtime accepts the new policy.
    super::super::runtime::refresh_active_codex_catalog_in_background(app);
    Ok(())
}

pub(super) async fn set_local_model_display_order(
    input: SetModelDisplayOrderInput,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let _mutation = state.setup_guard().await;
    // Order only needs configured model ids. Loading every source key and
    // account token here made a drag wait on the secret vault.
    let (order, old_gateway) = {
        let store = state.store()?;
        let order = complete_model_display_order(
            configured_pool_model_ids(
                store.sources(),
                store.accounts(),
                store.gateway().codex_text_models_enabled,
            ),
            &input.model_ids,
            &store.gateway().model_display_order,
        )
        .map_err(model_policy_error)?;
        (order, store.gateway().clone())
    };
    if order == old_gateway.model_display_order {
        return Ok(());
    }
    let mut gateway = old_gateway;
    gateway.model_display_order = order;
    state.store()?.replace_gateway(gateway.clone())?;
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.set_model_display_order(gateway.model_display_order);
    }
    Ok(())
}
