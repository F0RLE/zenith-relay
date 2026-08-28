use self::request::{
    alpha_search, chat_completions, gemini, messages, models, responses, responses_compact,
};
use crate::GatewayRuntime;
use axum::routing::{get, post};
use axum::Router;
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
