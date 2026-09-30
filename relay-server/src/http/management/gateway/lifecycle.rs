use super::super::{runtime_error, store_error, ManagementError};
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use std::sync::Arc;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::RuntimeStateSnapshot;

pub async fn start_gateway(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    state
        .store
        .routing_policy()
        .map_err(store_error)?
        .pool_routing
        .unwrap_or_default()
        .validate_activation()
        .map_err(|message| {
            ManagementError::validation(error_codes::POOL_ROUTING_CONFLICT, message)
        })?;
    let previous_enabled = state.store.gateway_enabled().map_err(store_error)?;
    state.store.set_gateway_enabled(true).map_err(store_error)?;
    state
        .rebuild_runtime_or_rollback(|| state.store.set_gateway_enabled(previous_enabled))
        .await
        .map_err(runtime_error)?;
    Ok(Json(state.snapshot().map_err(store_error)?))
}

pub async fn stop_gateway(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    // Retire pending reservations before the durable switch. A request that
    // entered the public API before this action must not start a new send
    // after the operator stopped the gateway.
    state.replace_runtime(None).map_err(runtime_error)?;
    if let Err(error) = state.store.set_gateway_enabled(false) {
        build.rebuild(&state).await.map_err(|restore| {
            runtime_error(format!(
                "{error}; failed to restore gateway runtime: {restore}"
            ))
        })?;
        return Err(store_error(error));
    }
    Ok(Json(state.snapshot().map_err(store_error)?))
}
