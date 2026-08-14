use super::{runtime_error, store_error, ManagementError};
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};
use zenith_relay_core::{
    is_valid_model_id, normalize_model_reasoning_allowed_levels,
    protocol::{model_has_native_account_route, RuntimeStateSnapshot},
    ApiModelPriceOverride,
};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/models", get(models))
        .route("/models/rules", post(set_model_enabled))
        .route("/models/prices", post(set_model_price))
        .route("/models/reasoning", post(set_model_reasoning))
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
    let canonical = canonical_model_id(&snapshot, &input.model_id)?;
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
    .map_err(|message| ManagementError::validation("model_price_invalid", message))?;
    let snapshot = state.snapshot().map_err(store_error)?;
    let canonical = canonical_model_id(&snapshot, &input.model_id)?.to_ascii_lowercase();
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
        state
            .store
            .reset_quota_economics_learning()
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

pub async fn set_model_reasoning(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SetModelReasoningInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let snapshot = state.snapshot().map_err(store_error)?;
    let canonical = canonical_model_id(&snapshot, &input.model_id)?.to_ascii_lowercase();
    let mut normalized_allowed_levels =
        normalize_model_reasoning_allowed_levels(BTreeMap::from([(
            canonical.clone(),
            input.allowed_levels,
        )]))
        .map_err(|message| ManagementError::validation("reasoning_levels_invalid", message))?;
    let allowed_levels = normalized_allowed_levels
        .remove(&canonical)
        .unwrap_or_default();
    let runtime = state.runtime().map_err(runtime_error)?;
    if !allowed_levels.is_empty() && model_has_native_account_route(&snapshot.accounts, &canonical)
    {
        return Err(ManagementError::new(
            StatusCode::CONFLICT,
            "native_reasoning_managed",
            "native ChatGPT model reasoning settings cannot be configured here",
            "configuration",
            false,
        ));
    }

    let previous = state
        .store
        .model_reasoning_allowed_levels()
        .map_err(store_error)?;
    let mut configured = previous.clone();
    if allowed_levels.is_empty() {
        configured.remove(&canonical);
    } else {
        configured.insert(canonical, allowed_levels);
    }
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
                        "model_reasoning_recovery_failed",
                        format!("{error}; failed to restore model reasoning levels: {rollback}"),
                    )
                })?;
            return Err(runtime_error(error.to_string()));
        }
    }
    state.snapshot().map(Json).map_err(store_error)
}

fn canonical_model_id(
    snapshot: &RuntimeStateSnapshot,
    requested: &str,
) -> Result<String, ManagementError> {
    let requested = requested.trim();
    if !is_valid_model_id(requested) {
        return Err(ManagementError::validation(
            "model_id_invalid",
            "model id is invalid",
        ));
    }
    snapshot
        .gateway
        .models
        .iter()
        .find(|model| model.id.eq_ignore_ascii_case(requested))
        .map(|model| model.id.clone())
        .ok_or_else(|| ManagementError::not_found("model_not_found", "pool model not found"))
}
