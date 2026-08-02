mod management;
mod middleware;
mod public_api;

use crate::state::AppState;
use axum::{
    extract::DefaultBodyLimit,
    middleware::from_fn_with_state,
    routing::{get, post},
    Router,
};
use std::sync::Arc;

const MAX_MANAGEMENT_BODY_BYTES: usize = 8 * 1024 * 1024;

pub fn router(state: Arc<AppState>) -> Router {
    let auth = middleware::ManagementAuth::new(&state.config.management_token);
    let management = management::routes()
        .route_layer(from_fn_with_state(auth, middleware::require_management))
        .layer(DefaultBodyLimit::max(MAX_MANAGEMENT_BODY_BYTES));

    Router::new()
        .route("/health", get(management::health))
        .merge(management)
        .route("/v1/models", get(public_api::proxy))
        .route(
            "/v1/responses",
            get(public_api::proxy).post(public_api::proxy),
        )
        .route("/v1/responses/compact", post(public_api::proxy))
        .route("/v1/chat/completions/v1/responses", post(public_api::proxy))
        .route(
            "/v1/chat/completions/v1/responses/compact",
            post(public_api::proxy),
        )
        .route("/v1/alpha/search", post(public_api::proxy))
        .route("/backend-api/codex/alpha/search", post(public_api::proxy))
        .route("/v1/chat/completions", post(public_api::proxy))
        .route("/v1/messages", post(public_api::proxy))
        .route("/v1/images/generations", post(public_api::proxy))
        .route("/v1/images/edits", post(public_api::proxy))
        .fallback(public_api::not_found)
        .with_state(state)
}
