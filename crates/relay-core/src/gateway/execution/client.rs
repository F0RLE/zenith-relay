use super::super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use super::super::errors::api_error;
use super::super::now_ms;
use super::super::request::{
    candidate_protocols, chat_request_is_text_or_image_only, chat_request_uses_tools,
    forwarded_codex_headers, forwarded_messages_headers, request_id, CODEX_RESPONSES_LITE_HEADER,
    MAX_CLIENT_REQUEST_BODY_BYTES, MAX_CLIENT_REQUEST_BODY_ERROR,
};
use super::request::execute_request;
use crate::protocol::ClientWireApi;
use crate::{GatewayRuntime, WireApi};
use axum::body::Body;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderValue, Request, Response, StatusCode};
use serde_json::Value;
use std::sync::Arc;

pub(in crate::gateway) async fn execute_client_request(
    runtime: Arc<GatewayRuntime>,
    request: Request<Body>,
    wire_api: WireApi,
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
        });
    let Some(key) = key else {
        return unauthorized();
    };
    let client_wire_api = match wire_api {
        WireApi::Responses => ClientWireApi::Responses,
        WireApi::ChatCompletions => ClientWireApi::ChatCompletions,
        WireApi::Messages => ClientWireApi::Messages,
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
    let Some(requested_model) = request
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string)
    else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "model must be a non-empty string",
            "invalid_request",
        );
    };
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
    };
    let Some(resolved_model) = runtime.resolve_visible_model(
        &key,
        &requested_model,
        candidate_protocols(wire_api),
        now_ms(),
    ) else {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available in this managed pool",
            "model_not_found",
        );
    };
    let responses_lite = (wire_api == WireApi::Responses)
        .then(|| {
            headers
                .get(CODEX_RESPONSES_LITE_HEADER)
                .cloned()
                .or_else(|| {
                    runtime
                        .codex_model_uses_responses_lite(&resolved_model)
                        .then(|| HeaderValue::from_static("true"))
                })
        })
        .flatten();
    let response_affinity_key = (wire_api == WireApi::Responses)
        .then(|| {
            runtime
                .response_affinity_key(request.get("previous_response_id").and_then(Value::as_str))
        })
        .flatten();
    let request_id = request_id();
    let forwarded_headers = match wire_api {
        WireApi::Messages => forwarded_messages_headers(&headers),
        WireApi::Responses | WireApi::ChatCompletions => {
            forwarded_codex_headers(&headers, &request_id)
        }
    };
    execute_request(
        runtime,
        key,
        request,
        requested_model,
        resolved_model,
        stream,
        request_id,
        forwarded_headers,
        response_affinity_key,
        wire_api,
        responses_lite,
        true,
        0,
    )
    .await
}
