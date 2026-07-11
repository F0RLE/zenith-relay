mod management_api;
mod middleware;
mod public_api;

use crate::state::AppState;
use axum::{
    middleware::from_fn_with_state,
    routing::{get, patch, post},
    Router,
};
use std::sync::Arc;

pub fn router(state: Arc<AppState>) -> Router {
    let auth = middleware::ManagementAuth::new(&state.config.management_token);
    let management = Router::new()
        .route("/capabilities", get(management_api::capabilities))
        .route("/state", get(management_api::state_snapshot))
        .route(
            "/sources",
            get(management_api::list_sources).post(management_api::create_source),
        )
        .route(
            "/sources/{id}",
            patch(management_api::update_source).delete(management_api::delete_source),
        )
        .route("/accounts", get(management_api::list_accounts))
        .route(
            "/accounts/import/preview",
            post(management_api::preview_account_import),
        )
        .route(
            "/accounts/import/confirm",
            post(management_api::confirm_account_import),
        )
        .route(
            "/accounts/import/batch/preview",
            post(management_api::preview_account_batch_import),
        )
        .route(
            "/accounts/import/batch/confirm",
            post(management_api::confirm_account_batch_import),
        )
        .route(
            "/accounts/{id}",
            patch(management_api::update_account).delete(management_api::delete_account),
        )
        .route(
            "/keys",
            get(management_api::list_keys).post(management_api::create_key),
        )
        .route(
            "/keys/{id}",
            patch(management_api::update_key).delete(management_api::delete_key),
        )
        .route("/keys/{id}/rotate", post(management_api::rotate_key))
        .route("/quota", get(management_api::quota))
        .route("/models", get(management_api::models))
        .route("/usage", get(management_api::usage))
        .route("/gateway/start", post(management_api::start_gateway))
        .route("/gateway/stop", post(management_api::stop_gateway))
        .route(
            "/wake-tasks",
            get(management_api::list_wake_tasks).post(management_api::create_wake_task),
        )
        .route(
            "/wake-tasks/{id}",
            patch(management_api::update_wake_task).delete(management_api::delete_wake_task),
        )
        .route(
            "/wake-tasks/{id}/test",
            post(management_api::test_wake_task),
        )
        .route("/wake-history", get(management_api::wake_history))
        .route_layer(from_fn_with_state(auth, middleware::require_management));

    Router::new()
        .route("/health", get(management_api::health))
        .merge(management)
        .route("/v1/models", get(public_api::proxy))
        .route("/v1/responses", post(public_api::proxy))
        .route("/v1/chat/completions", post(public_api::proxy))
        .fallback(public_api::not_found)
        .with_state(state)
}
