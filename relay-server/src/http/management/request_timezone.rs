use super::{runtime_error, store_error, validation_error, ManagementError};
use crate::state::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;
use zenith_relay_core::{error_codes, protocol::RuntimeStateSnapshot, RequestTimezoneUpdate};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/gateway/request-timezone", post(update))
        .route("/gateway/request-timezone/catalog", get(catalog))
}

async fn catalog() -> Json<Vec<String>> {
    Json(zenith_relay_core::request_timezone::request_timezone_catalog())
}

async fn update(
    State(state): State<Arc<AppState>>,
    Json(input): Json<RequestTimezoneUpdate>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    input.policy.validate().map_err(validation_error)?;
    input.expected_policy.validate().map_err(validation_error)?;
    let previous = state.store.request_timezone().map_err(store_error)?;
    if previous != input.expected_policy {
        return Err(ManagementError::new(
            StatusCode::CONFLICT,
            error_codes::CONFIGURATION_REVISION_STALE,
            "request timezone changed; reload before saving",
            "configuration",
            false,
        ));
    }
    state
        .store
        .set_request_timezone(&input.policy)
        .map_err(store_error)?;
    if let Some(runtime) = state.runtime().map_err(runtime_error)? {
        if let Err(error) = runtime.set_request_timezone(input.policy) {
            state
                .store
                .set_request_timezone(&previous)
                .map_err(store_error)?;
            return Err(runtime_error(error.to_string()));
        }
    }
    state.snapshot().map(Json).map_err(store_error)
}
