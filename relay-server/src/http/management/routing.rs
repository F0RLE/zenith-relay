use super::{store_error, ManagementError};
use crate::state::AppState;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;
use zenith_relay_core::protocol::RuntimeStateSnapshot;
use zenith_relay_core::{DefaultServiceTier, RoutingStrategy};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/routing/settings", post(set_routing_policy))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutingPolicyInput {
    max_retry_candidates: u8,
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
    let default_service_tier = input.default_service_tier.unwrap_or(previous.2);
    let image_base_model = input.image_base_model.unwrap_or(previous.3.clone());
    let subscription_plan_order = input
        .subscription_plan_order
        .unwrap_or_else(|| previous.4.clone());
    state
        .store
        .set_routing_policy(
            input.max_retry_candidates,
            input.routing_strategy,
            default_service_tier,
            image_base_model,
            subscription_plan_order,
        )
        .map_err(store_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        state
            .store
            .set_routing_policy(previous.0, previous.1, previous.2, previous.3, previous.4)
            .map_err(store_error)?;
        if let Err(restore) = state.rebuild_runtime().await {
            return Err(store_error(format!(
                "{error}; failed to restore previous runtime: {restore}"
            )));
        }
        return Err(store_error(error));
    }
    state.snapshot().map(Json).map_err(store_error)
}
