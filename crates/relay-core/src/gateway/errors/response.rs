use super::*;
use crate::error_codes;
use crate::ErrorOrigin;

pub(crate) fn cooldown_error(
    retry_at_ms: u64,
    failure: Option<&AttemptFailure>,
    all_sources_rate_limited: bool,
    origin: ErrorOrigin,
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
                |failure| {
                    api_error_with_origin_and_category(
                        failure.status,
                        failure.message,
                        failure.category,
                        failure.category,
                        origin,
                        None,
                    )
                },
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
    let message = origin.prefix_message(message);
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

pub(crate) fn prefix_error_value(value: &mut Value, origin: ErrorOrigin) -> bool {
    let message = if value
        .pointer("/response/error/message")
        .and_then(Value::as_str)
        .is_some()
    {
        value.pointer_mut("/response/error/message")
    } else if value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .is_some()
    {
        value.pointer_mut("/error/message")
    } else if value
        .pointer("/error/errors/0/message")
        .and_then(Value::as_str)
        .is_some()
    {
        value.pointer_mut("/error/errors/0/message")
    } else if value
        .pointer("/errors/0/message")
        .and_then(Value::as_str)
        .is_some()
    {
        value.pointer_mut("/errors/0/message")
    } else if value.get("message").and_then(Value::as_str).is_some() {
        value.get_mut("message")
    } else if value.get("detail").and_then(Value::as_str).is_some() {
        value.get_mut("detail")
    } else if value
        .get("error_description")
        .and_then(Value::as_str)
        .is_some()
    {
        value.get_mut("error_description")
    } else if value
        .pointer("/response/error")
        .and_then(Value::as_str)
        .is_some()
    {
        value.pointer_mut("/response/error")
    } else if value
        .get("error")
        .and_then(Value::as_str)
        .is_some_and(|error| !looks_like_error_code(error))
    {
        value.get_mut("error")
    } else {
        None
    };
    if let Some(message) = message {
        let Some(text) = message.as_str() else {
            return false;
        };
        let prefixed = origin.prefix_message(text);
        if prefixed == text {
            return false;
        }
        *message = Value::String(prefixed);
        return true;
    }
    false
}

fn looks_like_error_code(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value
            .chars()
            .any(|character| matches!(character, '_' | '-'))
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

pub(crate) fn prefix_error_body(body: &[u8], origin: ErrorOrigin) -> Vec<u8> {
    if body.is_empty() {
        return Vec::new();
    }
    if let Ok(mut value) = serde_json::from_slice::<Value>(body) {
        if let Some(message) = value.as_str() {
            let prefixed = origin.prefix_message(message);
            if let Ok(body) = serde_json::to_vec(&prefixed) {
                return body;
            }
        }
        if prefix_error_value(&mut value, origin) {
            if let Ok(body) = serde_json::to_vec(&value) {
                return body;
            }
        }
        return body.to_vec();
    }

    let Ok(message) = std::str::from_utf8(body) else {
        return body.to_vec();
    };
    origin.prefix_message(message).into_bytes()
}

pub(crate) fn api_error_type(status: StatusCode, code: &str) -> &'static str {
    error_codes::public_type(status.as_u16(), code)
}

pub(crate) fn api_error_code(code: &str) -> &str {
    error_codes::public_code(code)
}
