use super::{store_error, ManagementError};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;
use zenith_relay_core::protocol::{PresetRoutingPolicy, RuntimeStateSnapshot};
use zenith_relay_core::{DefaultServiceTier, RoutingStrategy};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/routing/settings", post(set_routing_policy))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutingPolicyInput {
    max_retry_candidates: u8,
    #[serde(default)]
    cooldown_after_failures: Option<u8>,
    #[serde(default)]
    keep_last_candidate_available: Option<bool>,
    #[serde(default)]
    routing_strategy: RoutingStrategy,
    #[serde(default)]
    default_service_tier: Option<DefaultServiceTier>,
    #[serde(default)]
    image_base_model: Option<Option<String>>,
    subscription_plan_order: Option<Vec<String>>,
}

pub async fn set_routing_policy(
    State(state): State<Arc<AppState>>,
    Json(input): Json<RoutingPolicyInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    if !(1..=8).contains(&input.max_retry_candidates) {
        return Err(ManagementError::validation(
            "max_retry_candidates_invalid",
            "max retry candidates must be between 1 and 8",
        ));
    }
    let previous = state.store.routing_policy().map_err(store_error)?;
    let cooldown_after_failures = input.cooldown_after_failures.unwrap_or(previous.5);
    if !(1..=8).contains(&cooldown_after_failures) {
        return Err(ManagementError::validation(
            "cooldown_after_failures_invalid",
            "cooldown after failures must be between 1 and 8",
        ));
    }
    let keep_last_candidate_available = input.keep_last_candidate_available.unwrap_or(previous.6);
    let default_service_tier = input.default_service_tier.unwrap_or(previous.2);
    let image_base_model = input.image_base_model.unwrap_or(previous.3.clone());
    let subscription_plan_order = input
        .subscription_plan_order
        .unwrap_or_else(|| previous.4.clone());
    let policy = PresetRoutingPolicy {
        max_retry_candidates: input.max_retry_candidates,
        cooldown_after_failures,
        keep_last_candidate_available,
        routing_strategy: input.routing_strategy,
        subscription_plan_order,
        default_service_tier,
        image_base_model,
    };
    let previous_policy = PresetRoutingPolicy {
        max_retry_candidates: previous.0,
        cooldown_after_failures: previous.5,
        keep_last_candidate_available: previous.6,
        routing_strategy: previous.1,
        subscription_plan_order: previous.4.clone(),
        default_service_tier: previous.2,
        image_base_model: previous.3.clone(),
    };
    state
        .store
        .set_routing_policy(&policy)
        .map_err(store_error)?;
    state
        .rebuild_runtime_or_rollback(|| state.store.set_routing_policy(&previous_policy))
        .await
        .map_err(store_error)?;
    state.snapshot().map(Json).map_err(store_error)
}
