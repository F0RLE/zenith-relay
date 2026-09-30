use super::errors::api_error;
use crate::error_codes;
use axum::body::Body;
use axum::http::header::{HOST, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use std::net::IpAddr;

pub(super) fn valid_local_host(headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<axum::http::uri::Authority>().ok())
    else {
        return false;
    };
    host.host().eq_ignore_ascii_case("localhost")
        || host
            .host()
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

pub(super) fn invalid_host() -> Response<Body> {
    api_error(
        StatusCode::MISDIRECTED_REQUEST,
        "Host must target the local gateway",
        error_codes::INVALID_HOST,
    )
}

pub(super) fn unauthorized() -> Response<Body> {
    let mut response = api_error(
        StatusCode::UNAUTHORIZED,
        "managed gateway credential is missing or invalid",
        error_codes::INVALID_API_KEY,
    );
    response
        .headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

pub(super) fn client_api_forbidden() -> Response<Body> {
    api_error(
        StatusCode::FORBIDDEN,
        "the managed gateway credential does not allow the requested API",
        error_codes::CLIENT_API_NOT_ALLOWED,
    )
}
pub(super) fn authenticate_client(
    runtime: &crate::GatewayRuntime,
    headers: &axum::http::HeaderMap,
    protocol: crate::WireApi,
) -> Option<crate::runtime::AuthenticatedKey> {
    runtime
        .authenticate(headers.get(axum::http::header::AUTHORIZATION))
        .or_else(|| {
            let header = match protocol {
                crate::WireApi::Messages => "x-api-key",
                crate::WireApi::Gemini => "x-goog-api-key",
                _ => return None,
            };
            headers
                .get(header)
                .and_then(|value| value.to_str().ok())
                .and_then(|secret| runtime.authenticate_secret(secret))
        })
}
