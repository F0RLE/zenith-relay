use super::{runtime_error, store_error, ManagementError};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::{PresetRoutingPolicy, RuntimeStateSnapshot};
use zenith_relay_core::DefaultServiceTier;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/routing/settings", post(set_routing_policy))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutingPolicyInput {
    pool_routing: Option<zenith_relay_core::PoolRoutingPolicy>,
    expected_pool_routing: Option<zenith_relay_core::PoolRoutingPolicy>,
    max_retry_candidates: u8,
    // Accept old clients' fields without allowing them to edit rotation behavior or
    // overwrite the retained storage-compatibility values.
    #[serde(default, rename = "cooldownAfterFailures")]
    _legacy_cooldown_after_failures: Option<serde::de::IgnoredAny>,
    #[serde(default, rename = "keepLastCandidateAvailable")]
    _legacy_keep_last_candidate_available: Option<serde::de::IgnoredAny>,
    #[serde(default, rename = "routingStrategy")]
    _legacy_routing_strategy: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    default_service_tier: Option<DefaultServiceTier>,
    #[serde(default)]
    image_base_model: Option<Option<String>>,
    #[serde(default)]
    basis_points_enabled: Option<bool>,
    #[serde(default, rename = "subscriptionPlanOrder")]
    _legacy_subscription_plan_order: Option<serde::de::IgnoredAny>,
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
    let default_service_tier = input
        .default_service_tier
        .unwrap_or(previous.default_service_tier);
    let image_base_model = input
        .image_base_model
        .unwrap_or(previous.image_base_model.clone());
    let basis_points_enabled = input
        .basis_points_enabled
        .unwrap_or(previous.basis_points_enabled);
    let policy = PresetRoutingPolicy {
        tool_policy: previous.tool_policy.clone(),
        // An unrelated scalar edit must not persist a newly reconciled,
        // malformed inventory entry or silently migrate a legacy policy.
        pool_routing: input.pool_routing.or_else(|| previous.pool_routing.clone()),
        basis_points_enabled,
        max_retry_candidates: input.max_retry_candidates,
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
        ) {
            state
                .store
                .set_routing_policy(&previous)
                .map_err(store_error)?;
            return Err(runtime_error(error.to_string()));
        }
        runtime.set_basis_points_enabled(policy.basis_points_enabled);
        runtime.set_default_service_tier(policy.default_service_tier);
    }
    state.snapshot().map(Json).map_err(store_error)
}
