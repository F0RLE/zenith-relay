use super::super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use super::super::continuation::{
    prepare_response_continuation, RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
    RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
};
use super::super::errors::api_error;
use super::super::now_ms;
use super::super::request::{
    candidate_protocols, chat_request_is_text_or_image_only, client_context_fingerprint,
    codex_background_request_kind, forwarded_codex_headers, forwarded_messages_headers,
    is_managed_codex_client, request_id, ServiceTierPolicy, CODEX_RESPONSES_LITE_HEADER,
};
use super::request::{execute_request, RequestExecution};
use crate::error_codes;
use crate::protocol::ClientWireApi;
use crate::{GatewayRuntime, WireApi};
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, Response, StatusCode};
use serde_json::Value;
use std::sync::Arc;

pub(in crate::gateway) async fn execute_client_request(
    runtime: Arc<GatewayRuntime>,
    request: Request<Body>,
    wire_api: WireApi,
) -> Response<Body> {
    execute_client_request_inner(runtime, request, wire_api, None, false).await
}

pub(in crate::gateway) async fn execute_gemini_client_request(
    runtime: Arc<GatewayRuntime>,
    request: Request<Body>,
    model: String,
    force_stream: bool,
) -> Response<Body> {
    execute_client_request_inner(runtime, request, WireApi::Gemini, Some(model), force_stream).await
}

async fn execute_client_request_inner(
    runtime: Arc<GatewayRuntime>,
    request: Request<Body>,
    wire_api: WireApi,
    path_model: Option<String>,
    force_stream: bool,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let key = super::super::auth::authenticate_client(&runtime, &headers, wire_api);
    let Some(key) = key else {
        return unauthorized();
    };
    let client_wire_api = match wire_api {
        WireApi::Responses => ClientWireApi::Responses,
        WireApi::ChatCompletions => ClientWireApi::ChatCompletions,
        WireApi::Messages => ClientWireApi::Messages,
        WireApi::Gemini => ClientWireApi::Gemini,
    };
    if !runtime.allows_client_wire_api(&key, client_wire_api) {
        return client_api_forbidden();
    }
    let mut request = match super::super::request_body::read_json_object(&headers, body).await {
        Ok(object) => Value::Object(object),
        Err(response) => return *response,
    };
    let managed_codex_client = is_managed_codex_client(&headers);
    let service_tier_policy = if managed_codex_client {
        ServiceTierPolicy::pool_owned(&request)
    } else {
        ServiceTierPolicy::client_owned(&request)
    };
    if wire_api == WireApi::ChatCompletions && !chat_request_is_text_or_image_only(&request) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Chat Completions supports text and image content only",
            error_codes::CHAT_FEATURE_NOT_SUPPORTED,
        );
    }
    let body_model = request
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string);
    if let (Some(path_model), Some(body_model)) = (path_model.as_deref(), body_model.as_deref()) {
        if !path_model.eq_ignore_ascii_case(body_model) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "model in the path must match model in the request body",
                error_codes::INVALID_REQUEST,
            );
        }
    }
    let Some(requested_model) = path_model.or(body_model) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "model must be a non-empty string",
            error_codes::INVALID_REQUEST,
        );
    };
    let background_kind = (wire_api == WireApi::Responses)
        .then(|| codex_background_request_kind(&headers, &request))
        .flatten();
    let request_id = request_id();
    let stream = match request.get("stream") {
        Some(Value::Bool(stream)) => *stream,
        Some(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "stream must be a boolean",
                error_codes::INVALID_REQUEST,
            )
        }
        None => false,
    } || force_stream;
    if let Some(kind) = background_kind {
        runtime.mark_request_origin(&request_id, kind);
        if !runtime.codex_background_tasks_enabled() {
            runtime.blocked_codex_background_event(
                &request_id,
                &key.id,
                &requested_model,
                wire_api,
                kind,
            );
            return blocked_background_response(wire_api, stream, &request_id, kind);
        }
    }
    let continuation = if wire_api == WireApi::Responses {
        match prepare_response_continuation(&runtime, &key.id, &mut request, now_ms(), None) {
            Ok(continuation) => Some(continuation),
            Err(()) => {
                return api_error(
                    StatusCode::CONFLICT,
                    RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
                    RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
                )
            }
        }
    } else {
        None
    };

    let resolved_model = runtime
        .resolve_visible_model(
            &key,
            &requested_model,
            candidate_protocols(wire_api),
            now_ms(),
        )
        .or_else(|| {
            (managed_codex_client && runtime.chatgpt_retry_until_available())
                .then(|| {
                    runtime.resolve_configured_model(
                        &key,
                        &requested_model,
                        candidate_protocols(wire_api),
                    )
                })
                .flatten()
        });
    let Some(resolved_model) = resolved_model else {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available in this managed pool",
            error_codes::MODEL_NOT_FOUND,
        );
    };
    let responses_lite = (wire_api == WireApi::Responses)
        .then(|| headers.get(CODEX_RESPONSES_LITE_HEADER).cloned())
        .flatten();
    let client_context_id = client_context_fingerprint(&headers);
    let forwarded_headers = match wire_api {
        WireApi::Messages => forwarded_messages_headers(&headers),
        WireApi::Responses | WireApi::ChatCompletions => {
            forwarded_codex_headers(&headers, &request_id)
        }
        WireApi::Gemini => super::super::request::forwarded_bridge_gemini_headers(&headers),
    };
    execute_request(RequestExecution {
        runtime: runtime.clone(),
        key,
        request,
        service_tier_policy,
        requested_model,
        resolved_model,
        stream,
        request_id,
        forwarded_headers,
        client_context_id,
        response_affinity_key: continuation
            .as_ref()
            .and_then(|continuation| continuation.response_affinity_key.clone()),
        requires_affinity_owner: continuation
            .is_some_and(|continuation| continuation.requires_affinity_owner),
        // Keep client eligibility with the request. The mutable setting is
        // read at every wait decision so a running request observes a toggle
        // change without granting this behavior to non-ChatGPT clients.
        wait_for_candidate_availability: managed_codex_client,
        wire_api,
        responses_lite,
        allow_previous_response_reset: true,
        attempt_offset: 0,
    })
    .await
}

fn blocked_background_response(
    wire_api: WireApi,
    stream: bool,
    request_id: &str,
    kind: &str,
) -> Response<Body> {
    let response_id = format!("resp_relay_blocked_{request_id}");
    let body = if stream {
        format!(
            "event: response.completed\ndata: {}\n\n",
            serde_json::json!({
                "type": "response.completed",
                "response": {"id": response_id, "object": "response", "status": "completed", "output": [], "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}, "metadata": {"zenith_relay": {"blocked": true, "request_type": kind}}}
            })
        )
    } else {
        match wire_api {
            WireApi::Responses => serde_json::json!({
                "id": response_id,
                "object": "response",
                "status": "completed",
                "output": [],
                "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0},
                "metadata": {"zenith_relay": {"blocked": true, "request_type": kind}}
            })
            .to_string(),
            WireApi::ChatCompletions => serde_json::json!({"id": response_id, "object": "chat.completion", "choices": [], "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}}).to_string(),
            WireApi::Messages => serde_json::json!({"id": response_id, "type": "message", "role": "assistant", "content": [], "stop_reason": "end_turn", "usage": {"input_tokens": 0, "output_tokens": 0}}).to_string(),
            WireApi::Gemini => serde_json::json!({"candidates": [], "usageMetadata": {"promptTokenCount": 0, "candidatesTokenCount": 0, "totalTokenCount": 0}}).to_string(),
        }
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(
            CONTENT_TYPE,
            if stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .body(Body::from(body))
        .expect("blocked response builder is valid")
}
