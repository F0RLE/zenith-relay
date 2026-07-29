use super::errors::api_error;
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
        "invalid_host",
    )
}

pub(super) fn unauthorized() -> Response<Body> {
    let mut response = api_error(
        StatusCode::UNAUTHORIZED,
        "local API key is missing or invalid",
        "invalid_api_key",
    );
    response
        .headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

pub(super) fn client_api_forbidden() -> Response<Body> {
    api_error(
        StatusCode::FORBIDDEN,
        "this client key does not allow the requested API",
        "client_api_not_allowed",
    )
}
