use super::model_policy::{configured_pool_model_ids, model_policy_error};
use super::CommandResult;
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError},
    state::DesktopState,
};
use tauri::State;
use zenith_relay_core::{
    protocol::complete_model_display_order, ApiModelPriceOverride, DefaultServiceTier,
};

pub(super) use zenith_relay_core::protocol::{
    SetModelEnabledInput, SetModelOrderInput as SetModelDisplayOrderInput, SetModelPriceInput,
    SetModelServiceTierInput,
};

pub(super) async fn set_local_model_enabled(
    input: SetModelEnabledInput,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let _mutation = state.setup_guard().await;
    let canonical = super::canonical_pool_model(&state, &input.model_id)?;
    let mut gateway = state.store()?.gateway().clone();
    let previous = gateway.hidden_models.clone();
    gateway
        .hidden_models
        .retain(|model| !model.eq_ignore_ascii_case(&canonical));
    if !input.enabled {
        gateway.hidden_models.push(canonical);
    }
    if gateway.hidden_models == previous {
        return Ok(());
    }
    let hidden = gateway.hidden_models.clone();
    state.store()?.replace_gateway(gateway)?;
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.set_hidden_models(hidden);
    }
    Ok(())
}

pub(super) async fn set_local_model_price(
    input: SetModelPriceInput,
    state: State<'_, DesktopState>,
) -> CommandResult<()> {
    let price = ApiModelPriceOverride::from_optional_fields(
        input.input_micro_usd_per_million,
        input.cached_input_micro_usd_per_million,
        input.cache_write_5m_micro_usd_per_million,
        input.cache_write_1h_micro_usd_per_million,
        input.output_micro_usd_per_million,
    )
    .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    let _mutation = state.setup_guard().await;
    let canonical = super::canonical_pool_model(&state, &input.model_id)?;
    let old_gateway = state.store()?.gateway().clone();
    let mut gateway = old_gateway.clone();
    let key = zenith_relay_core::model_id_key(&canonical);
    if let Some(price) = price {
        gateway.model_price_overrides.insert(key, price);
    } else {
        gateway.model_price_overrides.remove(&key);
    }
    if gateway != old_gateway {
        let mut store = state.store()?;
        store.replace_gateway(gateway)?;
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
        && !state
            .model_metadata_catalog()
            .service_tiers_for(&canonical)
            .contains(&input.service_tier)
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "requested service tier is not available under the Relay model-family policy",
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
            configured_pool_model_ids(store.sources(), store.accounts()),
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
