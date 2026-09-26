use super::{runtime_error, store_error, ManagementError};
use crate::state::{AppState, ServerAccountRecord, SourceRecord};
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use zenith_relay_core::error_codes;
use zenith_relay_core::{
    protocol::{
        canonical_pool_model_id, complete_model_display_order, update_model_reasoning_policy,
        ModelPolicyError, RuntimeStateSnapshot,
    },
    ApiModelPriceOverride, DefaultServiceTier,
};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/models", get(models))
        .route("/models/rules", post(set_model_enabled))
        .route("/models/prices", post(set_model_price))
        .route("/models/reasoning", post(set_model_reasoning))
        .route("/models/service-tier", post(set_model_service_tier))
        .route("/models/order", post(set_model_order))
}

#[derive(Serialize)]
pub struct ModelList {
    data: Vec<ModelItem>,
}

#[derive(Serialize)]
struct ModelItem {
    id: String,
    object: &'static str,
    owned_by: &'static str,
}

pub async fn models(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ModelList>, ManagementError> {
    let models = state
        .snapshot()
        .map_err(store_error)?
        .gateway
        .visible_model_ids
        .into_iter()
        .map(|id| ModelItem {
            id,
            object: "model",
            owned_by: "user",
        })
        .collect();
    Ok(Json(ModelList { data: models }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelEnabledInput {
    model_id: String,
    enabled: bool,
}

pub async fn set_model_enabled(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SetModelEnabledInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let snapshot = state.snapshot().map_err(store_error)?;
    let canonical = canonical_model_id(&state, &snapshot, &input.model_id)?;
    let old_hidden = state.store.hidden_models().map_err(store_error)?;
    let mut hidden = old_hidden.clone();
    hidden.retain(|model| !model.eq_ignore_ascii_case(&canonical));
    if !input.enabled {
        hidden.push(canonical);
    }
    if hidden == old_hidden {
        return Ok(Json(snapshot));
    }
    state.store.set_hidden_models(hidden).map_err(store_error)?;
    state
        .rebuild_runtime_or_rollback(|| state.store.set_hidden_models(old_hidden))
        .await
        .map_err(runtime_error)?;
    state.snapshot().map(Json).map_err(store_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelPriceInput {
    model_id: String,
    input_micro_usd_per_million: Option<u64>,
    cached_input_micro_usd_per_million: Option<u64>,
    cache_write_5m_micro_usd_per_million: Option<u64>,
    cache_write_1h_micro_usd_per_million: Option<u64>,
    output_micro_usd_per_million: Option<u64>,
}

pub async fn set_model_price(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SetModelPriceInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let price = ApiModelPriceOverride::from_optional_fields(
        input.input_micro_usd_per_million,
        input.cached_input_micro_usd_per_million,
        input.cache_write_5m_micro_usd_per_million,
        input.cache_write_1h_micro_usd_per_million,
        input.output_micro_usd_per_million,
    )
    .map_err(|message| ManagementError::validation(error_codes::MODEL_PRICE_INVALID, message))?;
    let snapshot = state.snapshot().map_err(store_error)?;
    let canonical = canonical_model_id(&state, &snapshot, &input.model_id)?.to_ascii_lowercase();
    let previous_overrides = state.store.model_price_overrides().map_err(store_error)?;
    let mut overrides = previous_overrides.clone();
    if let Some(price) = price {
        overrides.insert(canonical, price);
    } else {
        overrides.remove(&canonical);
    }
    if overrides != previous_overrides {
        state
            .store
            .set_model_price_overrides(overrides)
            .map_err(store_error)?;
    }
    state.snapshot().map(Json).map_err(store_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelReasoningInput {
    model_id: String,
    #[serde(default)]
    allowed_levels: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelServiceTierInput {
    model_id: String,
    service_tier: DefaultServiceTier,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelOrderInput {
    model_ids: Vec<String>,
}

pub async fn set_model_service_tier(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SetModelServiceTierInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let snapshot = state.snapshot().map_err(store_error)?;
    let canonical = canonical_model_id(&state, &snapshot, &input.model_id)?;
    let runtime = state.runtime().map_err(runtime_error)?;
    if input.service_tier != DefaultServiceTier::Standard
        && !snapshot.gateway.models.iter().any(|model| {
            model.id.eq_ignore_ascii_case(&canonical)
                && model.speed_tiers.contains(&input.service_tier)
        })
    {
        return Err(ManagementError::validation(
            error_codes::MODEL_SERVICE_TIER_UNSUPPORTED,
            "requested service tier is not available under the Relay model-family policy",
        ));
    }
    let previous = state
        .store
        .model_service_tier_overrides()
        .map_err(store_error)?;
    let mut next = previous.clone();
    let key = canonical.to_ascii_lowercase();
    next.insert(key, input.service_tier);
    if next == previous {
        return Ok(Json(snapshot));
    }
    state
        .store
        .set_model_service_tier_overrides(next.clone())
        .map_err(store_error)?;
    if let Some(runtime) = runtime {
        if let Err(error) = runtime.set_model_service_tier_overrides(next) {
            state
                .store
                .set_model_service_tier_overrides(previous)
                .map_err(store_error)?;
            return Err(runtime_error(error.to_string()));
        }
    }
    state.snapshot().map(Json).map_err(store_error)
}

pub async fn set_model_order(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SetModelOrderInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    let snapshot = state.snapshot().map_err(store_error)?;
    let sources = state.store.sources().map_err(store_error)?;
    let accounts = state.store.accounts().map_err(store_error)?;
    let previous = state.store.model_display_order().map_err(store_error)?;
    let order = complete_model_display_order(
        snapshot
            .gateway
            .models
            .iter()
            .map(|model| &model.id)
            .chain(configured_pool_model_ids(&sources, &accounts)),
        &input.model_ids,
        &previous,
    )
    .map_err(model_policy_error)?;
    if previous == order {
        return Ok(Json(snapshot));
    }
    state
        .store
        .set_model_display_order(order.clone())
        .map_err(store_error)?;
    if let Some(runtime) = state.runtime().map_err(runtime_error)? {
        runtime.set_model_display_order(order);
    }
    state.snapshot().map(Json).map_err(store_error)
}

pub async fn set_model_reasoning(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SetModelReasoningInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let snapshot = state.snapshot().map_err(store_error)?;
    let canonical = canonical_model_id(&state, &snapshot, &input.model_id)?.to_ascii_lowercase();
    let runtime = state.runtime().map_err(runtime_error)?;

    let previous = state
        .store
        .model_reasoning_allowed_levels()
        .map_err(store_error)?;
    let mut configured = previous.clone();
    update_model_reasoning_policy(&mut configured, &canonical, input.allowed_levels).map_err(
        |message| ManagementError::validation(error_codes::REASONING_LEVELS_INVALID, message),
    )?;
    if configured == previous {
        return Ok(Json(snapshot));
    }
    state
        .store
        .set_model_reasoning_allowed_levels(configured.clone())
        .map_err(store_error)?;
    if let Some(runtime) = runtime {
        if let Err(error) = runtime.set_model_reasoning_allowed_levels(configured) {
            state
                .store
                .set_model_reasoning_allowed_levels(previous)
                .map_err(|rollback| {
                    ManagementError::internal(
                        error_codes::MODEL_REASONING_RECOVERY_FAILED,
                        format!("{error}; failed to restore model reasoning levels: {rollback}"),
                    )
                })?;
            return Err(runtime_error(error.to_string()));
        }
    }
    state.snapshot().map(Json).map_err(store_error)
}

fn canonical_model_id(
    state: &AppState,
    snapshot: &RuntimeStateSnapshot,
    requested: &str,
) -> Result<String, ManagementError> {
    match canonical_pool_model_id(
        snapshot.gateway.models.iter().map(|model| &model.id),
        requested,
    ) {
        Ok(model) => return Ok(model.to_string()),
        Err(ModelPolicyError::NotFound) => {}
        Err(error) => return Err(model_policy_error(error)),
    }
    let sources = state.store.sources().map_err(store_error)?;
    let accounts = state.store.accounts().map_err(store_error)?;
    canonical_pool_model_id(configured_pool_model_ids(&sources, &accounts), requested)
        .map(str::to_owned)
        .map_err(model_policy_error)
}

/// Read the editable model inventory from configured pool members so actions
/// remain usable even with old or incomplete runtime projections.
fn configured_pool_model_ids<'a>(
    sources: &'a [SourceRecord],
    accounts: &'a [ServerAccountRecord],
) -> impl Iterator<Item = &'a String> {
    let source_models = sources
        .iter()
        .filter(|source| source.in_pool)
        .flat_map(|source| {
            source.models.iter().chain(
                source
                    .protocol_bindings
                    .iter()
                    .flat_map(|binding| &binding.model_ids),
            )
        });
    let account_models = accounts
        .iter()
        .filter(|account| account.in_pool)
        .flat_map(ServerAccountRecord::effective_models);
    source_models.chain(account_models)
}

fn model_policy_error(error: ModelPolicyError) -> ManagementError {
    match error {
        ModelPolicyError::NotFound => ManagementError::not_found(error.code(), error.to_string()),
        ModelPolicyError::InvalidId | ModelPolicyError::DuplicateOrderEntry => {
            ManagementError::validation(error.code(), error.to_string())
        }
    }
}
