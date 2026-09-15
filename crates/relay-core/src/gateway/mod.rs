use self::execution::{execute_account_endpoint, AccountExecution};
use self::request::AccountEndpoint;
use self::request::{
    alpha_search, chat_completions, gemini, messages, models, responses, responses_compact,
};
use crate::GatewayRuntime;
use axum::body::Body;
use axum::http::{HeaderMap, Response, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;
use std::sync::Arc;

pub(crate) use crate::unix_time_ms as now_ms;

mod auth;
mod errors;
mod execution;
mod images;
mod messages;
mod request;
mod response;
mod streaming;
mod turn_state;
mod websocket;

/// A scheduler-owned, non-streaming probe sent through one exact OAuth
/// account.  The request never enters the public listener; it reuses the
/// running [`GatewayRuntime`] so token refresh, cooldowns, leases, usage, and
/// diagnostics stay identical to ordinary account traffic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountWakeRequest {
    pub local_key_id: String,
    pub account_id: String,
    pub model_id: String,
    pub output_token_cap: u16,
}

/// Executes a scheduler-owned wake request against one account.  The returned
/// response is deliberately the same sanitized native Responses envelope used
/// by the account execution path; callers should inspect only status and
/// bounded usage fields and must not persist or log its body.
pub async fn execute_account_wake(
    runtime: Arc<GatewayRuntime>,
    request: AccountWakeRequest,
) -> Response<Body> {
    let local_key_id = request.local_key_id.trim();
    let account_id = request.account_id.trim();
    let model_id = request.model_id.trim();
    if local_key_id.is_empty()
        || account_id.is_empty()
        || !crate::is_valid_model_id(model_id)
        || !(1..=256).contains(&request.output_token_cap)
    {
        return errors::api_error(
            StatusCode::BAD_REQUEST,
            "invalid account wake request",
            "invalid_request",
        );
    }
    let Some(key) = runtime.internal_account_key(local_key_id, account_id) else {
        return errors::api_error(
            StatusCode::NOT_FOUND,
            "account is not available in the managed pool",
            "no_eligible_source",
        );
    };
    let Some(resolved_model) = runtime.resolve_configured_account_model(&key, model_id) else {
        return errors::api_error(
            StatusCode::NOT_FOUND,
            "model is not available for this account",
            "model_not_found",
        );
    };
    let request = json!({
        "model": model_id,
        "input": "Reply briefly.",
        "stream": false,
        "store": false,
        "max_output_tokens": request.output_token_cap,
    });
    execute_account_endpoint(AccountExecution {
        runtime,
        key,
        request,
        requested_model: model_id.to_string(),
        resolved_model,
        client_headers: HeaderMap::new(),
        endpoint: AccountEndpoint::Wake,
        responses_lite: None,
        response_affinity_key: None,
        rewrite_model: true,
        // A wake is a bounded background operation.  It must not hold a task
        // open indefinitely when this one account is cooling down; the next
        // scheduled permit will retry after the scheduler's cooldown.
        wait_for_candidate_availability: false,
        allow_automatic_responses_lite: false,
        request_origin: Some("wake"),
    })
    .await
}

pub fn router(runtime: Arc<GatewayRuntime>) -> Router {
    Router::new()
        .route("/v1/models", get(models))
        .route("/v1/responses", get(websocket::responses).post(responses))
        .route("/v1/responses/compact", post(responses_compact))
        .route("/v1/chat/completions/v1/responses", post(responses))
        .route(
            "/v1/chat/completions/v1/responses/compact",
            post(responses_compact),
        )
        .route("/v1/alpha/search", post(alpha_search))
        .route("/backend-api/codex/alpha/search", post(alpha_search))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/messages", post(messages))
        .route("/v1beta/models/{*model_action}", post(gemini))
        .route("/v1/models/{*model_action}", post(gemini))
        .route("/v1/images/generations", post(images::generations))
        .route("/v1/images/edits", post(images::edits))
        .with_state(runtime)
}

#[cfg(test)]
mod test_support {
    use crate::runtime::DefaultServiceTier;
    use crate::{ToolUseDiagnostics, UsageEvent};

    pub(super) fn test_usage_event() -> UsageEvent {
        UsageEvent {
            request_id: "request".into(),
            attempt: 1,
            local_key_id: "key".into(),
            source_id: "source".into(),
            candidate_id: Some("source".into()),
            account_id: None,
            account_token_generation: None,
            client_context_id: None,
            routing: None,
            requested_model: Some("model".into()),
            resolved_model: Some("model".into()),
            requested_reasoning_effort: None,
            effective_reasoning_effort: None,
            wire_api: crate::WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: true,
            http_status: 200,
            error_category: None,
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 0,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            cache_write_ttl: None,
            reasoning_tokens: None,
            output_tokens: None,
            total_tokens: None,
            quota_snapshot: None,
        }
    }
}
