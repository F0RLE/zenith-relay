use super::super::errors::RateLimitBodyHint;
use super::emit_usage;
use crate::error_codes;
use crate::runtime::ExecutorRoute;
use crate::{ErrorOrigin, GatewayRuntime, UsageEvent};
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
) -> Response<Body> {
    event.success = false;
    event.http_status = StatusCode::BAD_GATEWAY.as_u16();
    event.error_category = Some(error_codes::UPSTREAM_BODY.to_string());
    event.latency_ms = started.elapsed().as_millis() as u64;
    let origin = event.error_origin().unwrap_or(ErrorOrigin::Relay);
    let request_id = event.request_id.clone();
    emit_usage(runtime, event);
    super::super::errors::api_error_with_origin_and_category(
        StatusCode::BAD_GATEWAY,
        "upstream response failed",
        error_codes::UPSTREAM_ERROR,
        error_codes::UPSTREAM_BODY,
        origin,
        Some(&request_id),
    )
}

pub(in crate::gateway) fn proxy_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    response_body: Body,
) -> Response<Body> {
    let mut proxied_response = Response::builder()
        .status(status)
        .body(response_body)
        .unwrap();
    copy_safe_upstream_headers(proxied_response.headers_mut(), upstream_headers, true);
    proxied_response
}

pub(in crate::gateway) fn proxy_error_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    error_body: &[u8],
    origin: ErrorOrigin,
    category: &str,
    request_id: Option<&str>,
) -> Response<Body> {
    let origin = origin.for_category(category);
    let prefixed_body = super::super::errors::prefix_error_body(error_body, origin);
    let mut proxied_response = proxy_response(status, upstream_headers, Body::from(prefixed_body));
    attach_error_diagnostics(&mut proxied_response, origin, category, request_id);
    proxied_response
}

pub(in crate::gateway) fn attach_error_diagnostics(
    outgoing_response: &mut Response<Body>,
    origin: ErrorOrigin,
    category: &str,
    request_id: Option<&str>,
) {
    let origin = origin.for_category(category);
    outgoing_response.headers_mut().insert(
        "x-zenith-relay-error-origin",
        HeaderValue::from_static(origin.as_str()),
    );
    if let Ok(category_header) = HeaderValue::from_str(category) {
        outgoing_response
            .headers_mut()
            .insert("x-zenith-relay-error-category", category_header);
    }
    if let Some(request_id) = request_id.and_then(safe_request_id) {
        if let Ok(request_id_header) = HeaderValue::from_str(request_id) {
            outgoing_response
                .headers_mut()
                .insert("x-zenith-relay-request-id", request_id_header);
        }
    }
}

pub(in crate::gateway) fn attach_stream_diagnostics(
    outgoing_response: &mut Response<Body>,
    origin: ErrorOrigin,
    request_id: &str,
) {
    outgoing_response.headers_mut().insert(
        "x-zenith-relay-upstream-origin",
        HeaderValue::from_static(origin.as_str()),
    );
    if let Some(request_id) = safe_request_id(request_id) {
        if let Ok(request_id_header) = HeaderValue::from_str(request_id) {
            outgoing_response
                .headers_mut()
                .insert("x-zenith-relay-request-id", request_id_header);
        }
    }
}

fn safe_request_id(request_id: &str) -> Option<&str> {
    let trimmed_request_id = request_id.trim();
    (!trimmed_request_id.is_empty()
        && trimmed_request_id.len() <= 128
        && trimmed_request_id.is_ascii()
        && !trimmed_request_id.chars().any(char::is_control))
    .then_some(trimmed_request_id)
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
    stream_body: Body,
) -> Response<Body> {
    let mut proxied_response = Response::builder()
        .status(status)
        .body(stream_body)
        .unwrap();
    proxied_response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    copy_safe_upstream_headers(proxied_response.headers_mut(), upstream_headers, false);
    proxied_response
}

pub(in crate::gateway) fn proxy_json_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    json_body: Body,
) -> Response<Body> {
    let mut proxied_response = Response::builder().status(status).body(json_body).unwrap();
    proxied_response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    copy_safe_upstream_headers(proxied_response.headers_mut(), upstream_headers, false);
    proxied_response
}

/// Copies only response metadata that a native client can safely use for
/// retries and diagnostics. Credentials, cookies, transport headers, and
/// provider server details are never reflected to the local client.
fn copy_safe_upstream_headers(
    target: &mut HeaderMap,
    upstream_headers: &reqwest::header::HeaderMap,
    include_content_type: bool,
) {
    for (header_name, header_value) in upstream_headers {
        let header_name = header_name.as_str();
        let allowed = (include_content_type && header_name == CONTENT_TYPE.as_str())
            || matches!(
                header_name,
                "cache-control"
                    | "retry-after"
                    | "request-id"
                    | "x-request-id"
                    | "x-should-retry"
                    | "openai-processing-ms"
            )
            || header_name.starts_with("anthropic-ratelimit-")
            || header_name.starts_with("x-ratelimit-");
        if allowed {
            target.insert(
                axum::http::HeaderName::from_bytes(header_name.as_bytes())
                    .expect("upstream header name is valid"),
                HeaderValue::from_bytes(header_value.as_bytes())
                    .expect("upstream header value is valid"),
            );
        }
    }
}
