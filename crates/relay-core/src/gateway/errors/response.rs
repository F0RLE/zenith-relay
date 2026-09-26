use super::*;
use crate::error_codes;
use crate::ErrorOrigin;

pub(crate) fn cooldown_error(
    retry_at_ms: u64,
    failure: Option<&AttemptFailure>,
    all_sources_rate_limited: bool,
) -> Response<Body> {
    let seconds = retry_at_ms
        .saturating_sub(now_ms())
        .saturating_add(999)
        .checked_div(1_000)
        .unwrap_or_default()
        .max(1);
    let rate_limited = all_sources_rate_limited;
    let mut response = if rate_limited {
        failure
            .filter(|failure| failure.category == error_codes::UPSTREAM_QUOTA_EXHAUSTED)
            .map_or_else(
                || {
                    api_error(
                        StatusCode::TOO_MANY_REQUESTS,
                        "all eligible sources are rate limited",
                        error_codes::ALL_SOURCES_COOLING_DOWN,
                    )
                },
                |failure| api_error(failure.status, failure.message, failure.category),
            )
    } else {
        api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "all eligible sources are temporarily unavailable",
            error_codes::ALL_SOURCES_TEMPORARILY_UNAVAILABLE,
        )
    };
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

pub(crate) fn api_error(status: StatusCode, message: &str, code: &str) -> Response<Body> {
    api_error_with_origin(status, message, code, ErrorOrigin::Relay, None)
}

pub(crate) fn api_error_with_origin(
    status: StatusCode,
    message: &str,
    code: &str,
    origin: ErrorOrigin,
    request_id: Option<&str>,
) -> Response<Body> {
    api_error_with_origin_and_category(status, message, code, code, origin, request_id)
}

pub(crate) fn api_error_with_origin_and_category(
    status: StatusCode,
    message: &str,
    code: &str,
    category: &str,
    origin: ErrorOrigin,
    request_id: Option<&str>,
) -> Response<Body> {
    api_error_with_parameter(status, message, code, category, origin, request_id, None)
}

pub(crate) fn api_error_with_parameter(
    status: StatusCode,
    message: &str,
    code: &str,
    category: &str,
    origin: ErrorOrigin,
    request_id: Option<&str>,
    parameter: Option<&str>,
) -> Response<Body> {
    let origin = origin.for_category(category);
    let code = api_error_code(code);
    let error_type = api_error_type(status, code);
    let mut response = (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": error_type,
                "code": code,
                "param": parameter,
                "zenith_relay": {
                    "origin": origin.as_str(),
                    "category": category,
                    "request_id": request_id,
                },
            }
        })),
    )
        .into_response();
    super::super::response::attach_error_diagnostics(&mut response, origin, category, request_id);
    response.extensions_mut().insert(LocalGatewayError);
    response
}

pub(crate) fn api_error_type(status: StatusCode, code: &str) -> &'static str {
    error_codes::public_type(status.as_u16(), code)
}

pub(crate) fn api_error_code(code: &str) -> &str {
    error_codes::public_code(code)
}
