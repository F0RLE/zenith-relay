use super::super::errors::RateLimitBodyHint;
use super::emit_usage;
use crate::error_codes;
use crate::runtime::ExecutorRoute;
use crate::{Error, ErrorOrigin, GatewayRuntime, UsageEvent};
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use std::sync::Arc;
use std::time::Instant;
pub(in crate::gateway) type CompletionCallback =
    Arc<dyn Fn(&mut UsageEvent, Option<&str>, RateLimitBodyHint) + Send + Sync>;

pub(in crate::gateway) fn upstream_body_error_response(
    runtime: &GatewayRuntime,
    mut event: UsageEvent,
    started: Instant,
    error: Error,
) -> Response<Body> {
    event.success = false;
    event.http_status = StatusCode::BAD_GATEWAY.as_u16();
    let too_large = matches!(error, Error::UpstreamBodyTooLarge);
    let category = if too_large {
        error_codes::UPSTREAM_BODY_TOO_LARGE
    } else {
        error_codes::UPSTREAM_BODY
    };
    event.error_category = Some(category.to_string());
    event.latency_ms = started.elapsed().as_millis() as u64;
    let origin = event.error_origin().unwrap_or(ErrorOrigin::Relay);
    let request_id = event.request_id.clone();
    emit_usage(runtime, event);
    super::super::errors::api_error_with_origin_and_category(
        StatusCode::BAD_GATEWAY,
        if too_large {
            "upstream response is too large"
        } else {
            "upstream response failed"
        },
        error_codes::UPSTREAM_ERROR,
        category,
        origin,
        Some(&request_id),
    )
}

pub(in crate::gateway) fn proxy_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    copy_safe_upstream_headers(response.headers_mut(), upstream_headers, true);
    response
}

pub(in crate::gateway) fn proxy_error_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
    origin: ErrorOrigin,
    category: &str,
    request_id: Option<&str>,
) -> Response<Body> {
    let mut response = proxy_response(status, upstream_headers, body);
    attach_error_diagnostics(&mut response, origin, category, request_id);
    response
}

pub(in crate::gateway) fn attach_error_diagnostics(
    response: &mut Response<Body>,
    origin: ErrorOrigin,
    category: &str,
    request_id: Option<&str>,
) {
    let origin = origin.for_category(category);
    response.headers_mut().insert(
        "x-zenith-relay-error-origin",
        HeaderValue::from_static(origin.as_str()),
    );
    if let Ok(value) = HeaderValue::from_str(category) {
        response
            .headers_mut()
            .insert("x-zenith-relay-error-category", value);
    }
    if let Some(request_id) = request_id.and_then(safe_request_id) {
        if let Ok(value) = HeaderValue::from_str(request_id) {
            response
                .headers_mut()
                .insert("x-zenith-relay-request-id", value);
        }
    }
}

pub(in crate::gateway) fn attach_stream_diagnostics(
    response: &mut Response<Body>,
    origin: ErrorOrigin,
    request_id: &str,
) {
    response.headers_mut().insert(
        "x-zenith-relay-upstream-origin",
        HeaderValue::from_static(origin.as_str()),
    );
    if let Some(request_id) = safe_request_id(request_id) {
        if let Ok(value) = HeaderValue::from_str(request_id) {
            response
                .headers_mut()
                .insert("x-zenith-relay-request-id", value);
        }
    }
}

fn safe_request_id(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 128
        && value.is_ascii()
        && !value.chars().any(char::is_control))
    .then_some(value)
}

pub(in crate::gateway) fn route_error_origin(route: &ExecutorRoute) -> ErrorOrigin {
    if route.account_id.is_some() {
        ErrorOrigin::Account
    } else {
        ErrorOrigin::Provider
    }
}

pub(in crate::gateway) fn proxy_sse_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    copy_safe_upstream_headers(response.headers_mut(), upstream_headers, false);
    response
}

pub(in crate::gateway) fn proxy_json_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    copy_safe_upstream_headers(response.headers_mut(), upstream_headers, false);
    response
}

/// Copies only response metadata that a native client can safely use for
/// retries and diagnostics. Credentials, cookies, transport headers, and
/// provider server details are never reflected to the local client.
fn copy_safe_upstream_headers(
    target: &mut HeaderMap,
    upstream: &reqwest::header::HeaderMap,
    include_content_type: bool,
) {
    for (name, value) in upstream {
        let name = name.as_str();
        let allowed = (include_content_type && name == CONTENT_TYPE.as_str())
            || matches!(
                name,
                "cache-control"
                    | "retry-after"
                    | "request-id"
                    | "x-request-id"
                    | "x-should-retry"
                    | "openai-processing-ms"
            )
            || name.starts_with("anthropic-ratelimit-")
            || name.starts_with("x-ratelimit-");
        if allowed {
            target.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes())
                    .expect("upstream header name is valid"),
                HeaderValue::from_bytes(value.as_bytes()).expect("upstream header value is valid"),
            );
        }
    }
}
