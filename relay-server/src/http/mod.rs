mod management_api;
mod middleware;
mod public_api;

use crate::state::AppState;
use axum::{
    extract::DefaultBodyLimit,
    middleware::from_fn_with_state,
    routing::{get, patch, post},
    Router,
};
use std::sync::Arc;

const MAX_MANAGEMENT_BODY_BYTES: usize = 8 * 1024 * 1024;

pub fn router(state: Arc<AppState>) -> Router {
    let auth = middleware::ManagementAuth::new(&state.config.management_token);
    let management = Router::new()
        .route("/capabilities", get(management_api::capabilities))
        .route("/state", get(management_api::state_snapshot))
        .route(
            "/configuration/preset",
            get(management_api::configuration_preset),
        )
        .route(
            "/configuration/preset/preview",
            post(management_api::preview_configuration_preset),
        )
        .route(
            "/configuration/preset/apply",
            post(management_api::apply_configuration_preset),
        )
        .route("/routing/runtime", get(management_api::runtime_order))
        .route(
            "/sources",
            get(management_api::list_sources).post(management_api::create_source),
        )
        .route(
            "/sources/{id}",
            patch(management_api::update_source).delete(management_api::delete_source),
        )
        .route("/sources/{id}/test", post(management_api::test_source))
        .route("/accounts", get(management_api::list_accounts))
        .route("/pool/members", post(management_api::set_pool_membership))
        .route(
            "/pool/quota/refresh",
            post(management_api::refresh_all_account_quotas),
        )
        .route("/accounts/export", post(management_api::export_accounts))
        .route(
            "/accounts/{id}/identity/reveal",
            post(management_api::reveal_account_identity),
        )
        .route("/proxies/common", post(management_api::set_common_proxy))
        .route(
            "/proxies/policy",
            post(management_api::set_account_proxy_required),
        )
        .route(
            "/accounts/proxies/assign",
            post(management_api::assign_account_proxies),
        )
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
            "/accounts/{id}/proxy",
            post(management_api::set_account_proxy),
        )
        .route(
            "/accounts/{id}/refresh",
            post(management_api::refresh_account),
        )
        .route(
            "/keys",
            get(management_api::list_keys).post(management_api::create_key),
        )
        .route(
            "/profile/credential",
            get(management_api::profile_credential),
        )
        .route(
            "/keys/{id}",
            patch(management_api::update_key).delete(management_api::delete_key),
        )
        .route("/keys/{id}/rotate", post(management_api::rotate_key))
        .route("/quota", get(management_api::quota))
        .route("/quota/settings", post(management_api::set_quota_policy))
        .route(
            "/routing/settings",
            post(management_api::set_routing_policy),
        )
        .route("/models", get(management_api::models))
        .route("/models/rules", post(management_api::set_model_enabled))
        .route("/models/prices", post(management_api::set_model_price))
        .route(
            "/usage",
            get(management_api::usage).delete(management_api::clear_usage),
        )
        .route("/diagnostics", post(management_api::diagnose_gateway))
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
        .route_layer(from_fn_with_state(auth, middleware::require_management))
        .layer(DefaultBodyLimit::max(MAX_MANAGEMENT_BODY_BYTES));

    Router::new()
        .route("/health", get(management_api::health))
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
        .route("/v1/images/generations", post(public_api::proxy))
        .route("/v1/images/edits", post(public_api::proxy))
        .fallback(public_api::not_found)
        .with_state(state)
}
