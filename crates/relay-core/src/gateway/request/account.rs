use super::super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use super::super::errors::api_error;
use super::super::execution::{execute_account_endpoint, AccountExecution};
use super::super::request_body::read_json_object;
use super::normalization::{
    normalize_compact_account_request, responses_lite_parallel_tool_calls_valid,
};
use super::{CODEX_RESPONSES_LITE_HEADER, MAX_ALPHA_SEARCH_RESPONSE_BYTES};
use crate::error_codes;
use crate::protocol::ClientWireApi;
use crate::runtime::AuthenticatedKey;
use crate::GatewayRuntime;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderValue, Request, Response, StatusCode};
use serde_json::{Map, Value};
use std::sync::Arc;

pub(in crate::gateway) async fn responses_compact(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    let (headers, key, mut request) = match read_account_request(&runtime, request).await {
        Ok(request) => request,
        Err(response) => return *response,
    };
    if request.get("stream").is_some_and(|stream| stream != false) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "streaming is not supported for compact responses",
            error_codes::INVALID_REQUEST,
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
            error_codes::INVALID_REQUEST,
        );
    };
    let resolved_model = runtime
        .resolve_visible_account_model(&key, &requested_model)
        .or_else(|| {
            runtime
                .route_recovery_enabled()
                .then(|| runtime.resolve_configured_account_model(&key, &requested_model))
                .flatten()
        });
    let Some(resolved_model) = resolved_model else {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available in this managed pool",
            error_codes::MODEL_NOT_FOUND,
        );
    };
    let responses_lite = headers.get(CODEX_RESPONSES_LITE_HEADER).cloned();
    if responses_lite.is_some() && !responses_lite_parallel_tool_calls_valid(&request) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "responses Lite requires parallel_tool_calls to be a boolean",
            error_codes::INVALID_REQUEST,
        );
    }
    // The endpoint is ChatGPT-specific. Keep eligibility with the request and
    // read the mutable retry setting in the execution loop.
    let wait_for_candidate_availability = true;
    normalize_compact_account_request(&mut request, responses_lite.is_some());
    execute_account_endpoint(AccountExecution {
        runtime,
        key,
        request: Value::Object(request),
        requested_model,
        resolved_model,
        client_headers: headers,
        endpoint: AccountEndpoint::Compact,
        responses_lite,
        rewrite_model: true,
        wait_for_candidate_availability,
        allow_automatic_responses_lite: true,
        request_origin: None,
    })
    .await
}

pub(in crate::gateway) async fn alpha_search(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    let (mut headers, key, mut request) = match read_account_request(&runtime, request).await {
        Ok(request) => request,
        Err(response) => return *response,
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
            error_codes::NO_ELIGIBLE_SOURCE,
        );
    };
    let resolved_model = runtime
        .resolve_visible_account_model(&key, &requested_model)
        .or_else(|| {
            runtime
                .route_recovery_enabled()
                .then(|| runtime.resolve_configured_account_model(&key, &requested_model))
                .flatten()
        });
    let Some(resolved_model) = resolved_model else {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available in this managed pool",
            error_codes::MODEL_NOT_FOUND,
        );
    };
    if !model_was_provided {
        request.remove("model");
    }
    // The endpoint is ChatGPT-specific. Keep eligibility with the request and
    // read the mutable retry setting in the execution loop.
    let wait_for_candidate_availability = true;
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
    execute_account_endpoint(AccountExecution {
        runtime,
        key,
        request: Value::Object(request),
        requested_model,
        resolved_model,
        client_headers: headers,
        endpoint: AccountEndpoint::AlphaSearch,
        responses_lite: None,
        rewrite_model: model_was_provided,
        wait_for_candidate_availability,
        allow_automatic_responses_lite: true,
        request_origin: None,
    })
    .await
}

async fn read_account_request(
    runtime: &GatewayRuntime,
    request: Request<Body>,
) -> Result<(HeaderMap, AuthenticatedKey, Map<String, Value>), Box<Response<Body>>> {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    if !valid_local_host(&headers) {
        return Err(Box::new(invalid_host()));
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return Err(Box::new(unauthorized()));
    };
    if !runtime.allows_client_wire_api(&key, ClientWireApi::Responses) {
        return Err(Box::new(client_api_forbidden()));
    }
    let request = read_json_object(&headers, body).await?;
    Ok((headers, key, request))
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::gateway) enum AccountEndpoint {
    Compact,
    AlphaSearch,
    /// Native Responses endpoint used by scheduler-owned account wake probes.
    /// The account route already points at `/backend-api/codex/responses`, so
    /// this variant intentionally leaves the URL unchanged.
    Wake,
}

impl AccountEndpoint {
    pub(in crate::gateway) fn response_limit(self) -> usize {
        match self {
            Self::Compact | Self::Wake => crate::runtime::MAX_NON_STREAM_BODY_BYTES,
            Self::AlphaSearch => MAX_ALPHA_SEARCH_RESPONSE_BYTES,
        }
    }
}

pub(in crate::gateway) fn account_endpoint_url(
    mut responses_url: url::Url,
    endpoint: AccountEndpoint,
) -> Option<url::Url> {
    if endpoint == AccountEndpoint::Wake {
        return Some(responses_url);
    }
    let mut segments = responses_url.path_segments_mut().ok()?;
    segments.pop_if_empty().pop();
    match endpoint {
        AccountEndpoint::Compact => {
            segments.push("responses").push("compact");
        }
        AccountEndpoint::AlphaSearch => {
            segments.push("alpha").push("search");
        }
        AccountEndpoint::Wake => unreachable!("wake endpoint returned before path mutation"),
    }
    drop(segments);
    Some(responses_url)
}
