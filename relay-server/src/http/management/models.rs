use super::{runtime_error, store_error, ManagementError};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use zenith_relay_core::error_codes;
use zenith_relay_core::{
    is_valid_model_id, normalize_model_reasoning_allowed_levels, protocol::RuntimeStateSnapshot,
    reasoning_policy_key, ApiModelPriceOverride, DefaultServiceTier, WireApi,
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
        && !runtime.as_ref().is_some_and(|runtime| {
            runtime.model_supports_service_tier(&canonical, input.service_tier)
        })
    {
        return Err(ManagementError::validation(
            error_codes::MODEL_SERVICE_TIER_UNSUPPORTED,
            "requested service tier requires confirmed upstream support for this active model route",
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
    let snapshot = state.snapshot().map_err(store_error)?;
    let current = pool_model_inventory(&state, &snapshot)?;
    let previous = state.store.model_display_order().map_err(store_error)?;
    let order = complete_model_display_order(&current, input.model_ids, &previous)?;
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
    let policy_key = reasoning_policy_key(&canonical);
    let mut normalized_allowed_levels =
        normalize_model_reasoning_allowed_levels(BTreeMap::from([(
            policy_key.clone(),
            input.allowed_levels,
        )]))
        .map_err(|message| {
            ManagementError::validation(error_codes::REASONING_LEVELS_INVALID, message)
        })?;
    let allowed_levels = normalized_allowed_levels
        .remove(&policy_key)
        .unwrap_or_default();
    let runtime = state.runtime().map_err(runtime_error)?;

    let previous = state
        .store
        .model_reasoning_allowed_levels()
        .map_err(store_error)?;
    let mut configured = previous.clone();
    configured.remove(&canonical);
    // Keep an explicit empty override so the user can disable every
    // provider-reported mode without losing that choice on the next refresh.
    configured.insert(policy_key, allowed_levels);
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
    let requested = requested.trim();
    if !is_valid_model_id(requested) {
        return Err(ManagementError::validation(
            error_codes::MODEL_ID_INVALID,
            "model id is invalid",
        ));
    }
    if let Some(model) = snapshot
        .gateway
        .models
        .iter()
        .find(|model| model.id.eq_ignore_ascii_case(requested))
        .map(|model| model.id.clone())
    {
        return Ok(model);
    }
    pool_model_inventory(state, snapshot)?
        .get(&requested.to_ascii_lowercase())
        .cloned()
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::MODEL_NOT_FOUND, "pool model not found")
        })
}

/// Build the editable model inventory from configured pool members. The live
/// snapshot intentionally omits routes that are unavailable, but management
/// actions must remain usable while a source or account is cooling down,
/// refreshing, or temporarily missing credentials.
fn pool_model_inventory(
    state: &AppState,
    snapshot: &RuntimeStateSnapshot,
) -> Result<BTreeMap<String, String>, ManagementError> {
    let sources = state.store.sources().map_err(store_error)?;
    let accounts = state.store.accounts().map_err(store_error)?;
    let mut current = snapshot
        .gateway
        .models
        .iter()
        .map(|model| (model.id.to_ascii_lowercase(), model.id.clone()))
        .collect::<BTreeMap<_, _>>();
    for source in sources.iter().filter(|source| source.in_pool) {
        for wire_api in WireApi::ALL {
            let Ok(models) = source.models_for_wire_api(wire_api) else {
                continue;
            };
            for model in models {
                current.entry(model.to_ascii_lowercase()).or_insert(model);
            }
        }
    }
    for account in accounts.iter().filter(|account| account.in_pool) {
        for model in account.effective_models() {
            current
                .entry(model.to_ascii_lowercase())
                .or_insert_with(|| model.clone());
        }
    }
    Ok(current)
}

/// Merge a partial order from the management UI with the saved order and the
/// current configured inventory. Unknown or duplicate IDs supplied by the UI
/// are rejected; stale saved IDs are dropped and newly discovered models are
/// appended deterministically.
fn complete_model_display_order(
    current: &BTreeMap<String, String>,
    requested_ids: Vec<String>,
    saved_order: &[String],
) -> Result<Vec<String>, ManagementError> {
    let mut included = BTreeSet::new();
    let mut order = Vec::with_capacity(current.len());
    let mut include = |model: &str, reject: bool| -> Result<(), ManagementError> {
        let key = model.trim().to_ascii_lowercase();
        let Some(canonical) = current.get(&key) else {
            if reject {
                return Err(ManagementError::not_found(
                    error_codes::MODEL_NOT_FOUND,
                    "pool model not found",
                ));
            }
            return Ok(());
        };
        if !included.insert(key) {
            if reject {
                return Err(ManagementError::validation(
                    error_codes::MODEL_ORDER_INVALID,
                    "model order contains duplicates",
                ));
            }
            return Ok(());
        }
        order.push(canonical.clone());
        Ok(())
    };

    for model in requested_ids {
        include(&model, true)?;
    }
    for model in saved_order {
        include(model, false)?;
    }
    for model in current.values() {
        include(model, false)?;
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory(ids: &[&str]) -> BTreeMap<String, String> {
        ids.iter()
            .map(|id| (id.to_ascii_lowercase(), (*id).to_string()))
            .collect()
    }

    #[test]
    fn partial_model_order_keeps_saved_entries_and_appends_new_models() {
        let current = inventory(&["gpt-a", "gpt-b", "gpt-hidden", "gpt-new"]);
        let order = complete_model_display_order(
            &current,
            vec!["gpt-b".into(), "gpt-a".into()],
            &["gpt-hidden".into(), "gpt-a".into(), "stale-model".into()],
        )
        .unwrap();
        assert_eq!(order, ["gpt-b", "gpt-a", "gpt-hidden", "gpt-new"]);
    }

    #[test]
    fn model_order_rejects_unknown_and_duplicate_requested_ids() {
        let current = inventory(&["gpt-a", "gpt-b"]);
        assert!(complete_model_display_order(&current, vec!["missing".into()], &[]).is_err());
        assert!(
            complete_model_display_order(&current, vec!["gpt-a".into(), "GPT-A".into()], &[])
                .is_err()
        );
    }
}
