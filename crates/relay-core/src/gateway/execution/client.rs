use super::super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use super::super::errors::api_error;
use super::super::now_ms;
use super::super::request::{
    candidate_protocols, chat_request_is_text_or_image_only, chat_request_uses_tools,
    client_context_fingerprint, codex_background_request_kind, forwarded_codex_headers,
    forwarded_messages_headers, is_managed_codex_client, request_id, response_tool_call_ids,
    tool_call_output_ids, ServiceTierPolicy, CODEX_RESPONSES_LITE_HEADER,
    MAX_CLIENT_REQUEST_BODY_BYTES, MAX_CLIENT_REQUEST_BODY_ERROR,
};
use super::request::{execute_request, RequestExecution};
use crate::protocol::ClientWireApi;
use crate::{GatewayRuntime, WireApi};
use axum::body::Body;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
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
    let key = runtime
        .authenticate(headers.get(AUTHORIZATION))
        .or_else(|| {
            (wire_api == WireApi::Messages)
                .then(|| headers.get("x-api-key"))
                .flatten()
                .and_then(|value| value.to_str().ok())
                .and_then(|secret| runtime.authenticate_secret(secret))
        })
        .or_else(|| {
            (wire_api == WireApi::Gemini)
                .then(|| headers.get("x-goog-api-key"))
                .flatten()
                .and_then(|value| value.to_str().ok())
                .and_then(|secret| runtime.authenticate_secret(secret))
        });
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
    let Ok(body) = axum::body::to_bytes(body, MAX_CLIENT_REQUEST_BODY_BYTES).await else {
        return api_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            MAX_CLIENT_REQUEST_BODY_ERROR,
            "request_too_large",
        );
    };

    let request: Value = match serde_json::from_slice(&body) {
        Ok(Value::Object(request)) => Value::Object(request),
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "request body must be a JSON object",
                "invalid_request",
            )
        }
    };
    let managed_codex_client = is_managed_codex_client(&headers);
    let service_tier_policy = if managed_codex_client {
        ServiceTierPolicy::pool_owned(&request)
    } else {
        ServiceTierPolicy::client_owned(&request)
    };
    if wire_api == WireApi::ChatCompletions && chat_request_uses_tools(&request) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "tool use is not supported through Chat Completions; use Responses or Messages",
            "tool_use_not_supported",
        );
    }
    if wire_api == WireApi::ChatCompletions && !chat_request_is_text_or_image_only(&request) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Chat Completions supports text and image content only",
            "chat_feature_not_supported",
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
                "invalid_request",
            );
        }
    }
    let Some(requested_model) = path_model.or(body_model) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "model must be a non-empty string",
            "invalid_request",
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
                "invalid_request",
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
    let response_affinity_key = (wire_api == WireApi::Responses)
        .then(|| {
            runtime
                .response_affinity_key(request.get("previous_response_id").and_then(Value::as_str))
        })
        .flatten();
    let tool_output_ids = if wire_api == WireApi::Responses {
        tool_call_output_ids(&request)
    } else {
        Vec::new()
    };
    let tool_call_ids = if wire_api == WireApi::Responses {
        response_tool_call_ids(&request)
    } else {
        Vec::new()
    };
    let has_unpaired_tool_output = tool_output_ids
        .iter()
        .any(|output_id| !tool_call_ids.iter().any(|call_id| call_id == output_id));
    let response_binding_known = response_affinity_key
        .as_deref()
        .is_some_and(|key| runtime.has_response_affinity_binding(key, now_ms()));
    let tool_affinity_key = tool_output_ids.iter().find_map(|call_id| {
        let affinity_key = runtime.tool_call_affinity_key(&key.id, call_id)?;
        runtime
            .has_response_affinity_binding(&affinity_key, now_ms())
            .then_some(affinity_key)
    });
    let has_previous_response_id = response_affinity_key.is_some();
    // A model switch can make an old tool call lose its route affinity. Keep
    // the current conversation alive and let normal pool selection continue it.
    let response_affinity_key = if response_binding_known {
        response_affinity_key
    } else {
        tool_affinity_key.or(response_affinity_key)
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
            "model_not_found",
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
        response_affinity_key,
        requires_affinity_owner: has_previous_response_id || has_unpaired_tool_output,
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
