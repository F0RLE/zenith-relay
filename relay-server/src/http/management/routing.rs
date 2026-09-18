use super::{runtime_error, store_error, ManagementError};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::{PresetRoutingPolicy, RuntimeStateSnapshot};
use zenith_relay_core::{DefaultServiceTier, RoutingStrategy};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/routing/settings", post(set_routing_policy))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutingPolicyInput {
    pool_routing: Option<zenith_relay_core::PoolRoutingPolicy>,
    expected_pool_routing: Option<zenith_relay_core::PoolRoutingPolicy>,
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
    let _configuration = state.configuration_lock.lock().await;
    if !(1..=8).contains(&input.max_retry_candidates) {
        return Err(ManagementError::validation(
            error_codes::MAX_RETRY_CANDIDATES_INVALID,
            "max retry candidates must be between 1 and 8",
        ));
    }
    let previous = state.store.routing_policy().map_err(store_error)?;
    let current_pool = state
        .snapshot()
        .map_err(store_error)?
        .gateway
        .pool_routing
        .unwrap_or_default();
    if let Some(policy) = &input.pool_routing {
        policy
            .validate_update(&current_pool, input.expected_pool_routing.as_ref())
            .map_err(|message| {
                ManagementError::validation(error_codes::POOL_ROUTING_CONFLICT, message)
            })?;
    }
    let cooldown_after_failures = input
        .cooldown_after_failures
        .unwrap_or(previous.cooldown_after_failures);
    if !(1..=8).contains(&cooldown_after_failures) {
        return Err(ManagementError::validation(
            error_codes::COOLDOWN_AFTER_FAILURES_INVALID,
            "cooldown after failures must be between 1 and 8",
        ));
    }
    let keep_last_candidate_available = input
        .keep_last_candidate_available
        .unwrap_or(previous.keep_last_candidate_available);
    let default_service_tier = input
        .default_service_tier
        .unwrap_or(previous.default_service_tier);
    let image_base_model = input
        .image_base_model
        .unwrap_or(previous.image_base_model.clone());
    let subscription_plan_order = input
        .subscription_plan_order
        .unwrap_or_else(|| previous.subscription_plan_order.clone());
    let policy = PresetRoutingPolicy {
        pool_routing: Some(input.pool_routing.unwrap_or_else(|| current_pool.clone())),
        max_retry_candidates: input.max_retry_candidates,
        cooldown_after_failures,
        keep_last_candidate_available,
        routing_strategy: input.routing_strategy,
        subscription_plan_order,
        default_service_tier,
        image_base_model,
    };
    state
        .store
        .set_routing_policy(&policy)
        .map_err(store_error)?;
    if policy.image_base_model != previous.image_base_model {
        state
            .rebuild_runtime_or_rollback(|| state.store.set_routing_policy(&previous))
            .await
            .map_err(runtime_error)?;
    } else if let Some(runtime) = state.runtime().map_err(runtime_error)? {
        if let Err(error) = runtime.set_pool_routing_policy(
            policy.pool_routing.clone().unwrap_or(current_pool),
            policy.max_retry_candidates,
            policy.cooldown_after_failures,
            policy.keep_last_candidate_available,
        ) {
            state
                .store
                .set_routing_policy(&previous)
                .map_err(store_error)?;
            return Err(runtime_error(error.to_string()));
        }
        runtime.set_default_service_tier(policy.default_service_tier);
    }
    state.snapshot().map(Json).map_err(store_error)
}
