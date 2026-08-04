use super::super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use super::super::errors::api_error;
use super::super::execution::execute_account_endpoint;
use super::normalization::normalize_account_request;
use super::{
    CODEX_RESPONSES_LITE_HEADER, MAX_ALPHA_SEARCH_RESPONSE_BYTES, MAX_CLIENT_REQUEST_BODY_BYTES,
    MAX_CLIENT_REQUEST_BODY_ERROR,
};
use crate::protocol::ClientWireApi;
use crate::runtime::AuthenticatedKey;
use crate::GatewayRuntime;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderValue, Request, Response, StatusCode};
use serde_json::{Map, Value};
use std::sync::Arc;

pub(in crate::gateway) async fn responses_compact(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };
    if !runtime.allows_client_wire_api(&key, ClientWireApi::Responses) {
        return client_api_forbidden();
    }
    let mut request = match read_json_object(body).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.get("stream").is_some_and(|stream| stream != false) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "streaming is not supported for compact responses",
            "invalid_request",
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
    let Some(resolved_model) = resolve_visible_account_model(&runtime, &key, &requested_model)
    else {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available for this local key",
            "model_not_found",
        );
    };
    let responses_lite = headers
        .get(CODEX_RESPONSES_LITE_HEADER)
        .cloned()
        .or_else(|| {
            runtime
                .codex_model_uses_responses_lite(&resolved_model)
                .then(|| HeaderValue::from_static("true"))
        });
    let response_affinity_key =
        runtime.response_affinity_key(request.get("previous_response_id").and_then(Value::as_str));
    normalize_account_request(&mut request, responses_lite.is_some());
    request.remove("stream");
    execute_account_endpoint(
        runtime,
        key,
        Value::Object(request),
        requested_model,
        resolved_model,
        headers,
        AccountEndpoint::Compact,
        responses_lite,
        response_affinity_key,
        true,
    )
    .await
}

pub(in crate::gateway) async fn alpha_search(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let mut headers = parts.headers;
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };
    if !runtime.allows_client_wire_api(&key, ClientWireApi::Responses) {
        return client_api_forbidden();
    }
    let mut request = match read_json_object(body).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let model_was_provided = request
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| !model.trim().is_empty());
    let requested_model = request
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string)
        .or_else(|| runtime.visible_account_models(&key).into_iter().next());
    let Some(requested_model) = requested_model else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no OAuth account model is available for search",
            "no_eligible_source",
        );
    };
    let Some(resolved_model) = resolve_visible_account_model(&runtime, &key, &requested_model)
    else {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available for this local key",
            "model_not_found",
        );
    };
    if !model_was_provided {
        request.remove("model");
    }
    request.remove("prompt_cache_key");
    request.remove("prompt_cache_retention");
    if let Some(session_id) = request
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .and_then(|value| HeaderValue::from_str(value).ok())
    {
        if !headers.contains_key("x-session-id") {
            headers.insert("x-session-id", session_id.clone());
        }
        if !headers.contains_key("session_id") {
            headers.insert("session_id", session_id);
        }
    }
    execute_account_endpoint(
        runtime,
        key,
        Value::Object(request),
        requested_model,
        resolved_model,
        headers,
        AccountEndpoint::AlphaSearch,
        None,
        None,
        model_was_provided,
    )
    .await
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::gateway) enum AccountEndpoint {
    Compact,
    AlphaSearch,
}

impl AccountEndpoint {
    pub(in crate::gateway) fn response_limit(self) -> usize {
        match self {
            Self::Compact => crate::runtime::MAX_NON_STREAM_BODY_BYTES,
            Self::AlphaSearch => MAX_ALPHA_SEARCH_RESPONSE_BYTES,
        }
    }
}

async fn read_json_object(body: Body) -> Result<Map<String, Value>, Response<Body>> {
    let body = axum::body::to_bytes(body, MAX_CLIENT_REQUEST_BODY_BYTES)
        .await
        .map_err(|_| {
            api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                MAX_CLIENT_REQUEST_BODY_ERROR,
                "request_too_large",
            )
        })?;
    match serde_json::from_slice(&body) {
        Ok(Value::Object(object)) => Ok(object),
        _ => Err(api_error(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
            "invalid_request",
        )),
    }
}

fn resolve_visible_account_model(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    requested_model: &str,
) -> Option<String> {
    runtime.resolve_visible_account_model(key, requested_model)
}

pub(in crate::gateway) fn account_endpoint_url(
    mut responses_url: url::Url,
    endpoint: AccountEndpoint,
) -> Option<url::Url> {
    let mut segments = responses_url.path_segments_mut().ok()?;
    segments.pop_if_empty().pop();
    match endpoint {
        AccountEndpoint::Compact => {
            segments.push("responses").push("compact");
        }
        AccountEndpoint::AlphaSearch => {
            segments.push("alpha").push("search");
        }
    }
    drop(segments);
    Some(responses_url)
}
