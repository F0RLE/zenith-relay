use super::{runtime_error, store_error, ManagementError};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use zenith_relay_core::protocol::RuntimeStateSnapshot;
use zenith_relay_core::ApiModelPriceOverride;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/models", get(models))
        .route("/models/rules", post(set_model_enabled))
        .route("/models/prices", post(set_model_price))
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
    let requested = input.model_id.trim();
    if requested.is_empty() || requested.len() > 256 || requested.chars().any(char::is_control) {
        return Err(ManagementError::validation(
            "model_id_invalid",
            "model id is invalid",
        ));
    }
    let snapshot = state.snapshot().map_err(store_error)?;
    let canonical = snapshot
        .gateway
        .models
        .iter()
        .find(|model| model.id.eq_ignore_ascii_case(requested))
        .map(|model| model.id.clone())
        .ok_or_else(|| ManagementError::not_found("model_not_found", "pool model not found"))?;
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
    if let Err(error) = state.rebuild_runtime().await {
        state
            .store
            .set_hidden_models(old_hidden)
            .map_err(|rollback| {
                ManagementError::internal(
                    "model_rule_recovery_failed",
                    format!("{error}; failed to restore model rules: {rollback}"),
                )
            })?;
        return Err(runtime_error(error));
    }
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
    let requested = input.model_id.trim();
    if requested.is_empty() || requested.len() > 256 || requested.chars().any(char::is_control) {
        return Err(ManagementError::validation(
            "model_id_invalid",
            "model id is invalid",
        ));
    }
    let snapshot = state.snapshot().map_err(store_error)?;
    let canonical = snapshot
        .gateway
        .models
        .iter()
        .find(|model| model.id.eq_ignore_ascii_case(requested))
        .map(|model| model.id.to_ascii_lowercase())
        .ok_or_else(|| ManagementError::not_found("model_not_found", "pool model not found"))?;
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
