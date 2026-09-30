use super::{runtime_error, store_error, validation_error, ManagementError};
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use std::sync::Arc;
use zenith_relay_core::{error_codes, protocol::RuntimeStateSnapshot, ToolPolicyUpdate};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/gateway/tool-policy", post(update))
}

async fn update(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ToolPolicyUpdate>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    let policy = input.policy.normalized().map_err(validation_error)?;
    let expected = input
        .expected_policy
        .normalized()
        .map_err(validation_error)?;
    let previous = state.store.routing_policy().map_err(store_error)?;
    if previous.tool_policy.clone().unwrap_or_default() != expected {
        return Err(ManagementError::new(
            StatusCode::CONFLICT,
            error_codes::CONFIGURATION_REVISION_STALE,
            "tool policy changed; reload before saving",
            "configuration",
            false,
        ));
    }
    let mut next = previous.clone();
    next.tool_policy = Some(policy.clone());
    state.store.set_routing_policy(&next).map_err(store_error)?;
    if let Some(runtime) = state.runtime().map_err(runtime_error)? {
        if let Err(error) = runtime.set_tool_policy(policy) {
            state
                .store
                .set_routing_policy(&previous)
                .map_err(store_error)?;
            return Err(runtime_error(error.to_string()));
        }
    }
    state.snapshot().map(Json).map_err(store_error)
}
