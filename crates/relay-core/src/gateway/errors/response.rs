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
    let mut error_response = if rate_limited {
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
    if let Ok(retry_after_header) = HeaderValue::from_str(&seconds.to_string()) {
        error_response
            .headers_mut()
            .insert(RETRY_AFTER, retry_after_header);
    }
    error_response
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
    let mut error_response = (
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
    super::super::response::attach_error_diagnostics(
        &mut error_response,
        origin,
        category,
        request_id,
    );
    error_response.extensions_mut().insert(LocalGatewayError);
    error_response
}

pub(crate) fn prefix_error_value(error_payload: &mut Value, origin: ErrorOrigin) -> bool {
    let request_id = crate::usage::UpstreamErrorDetails::from_value(None, error_payload).request_id;
    let message_field = if error_payload
        .pointer("/response/error/message")
        .and_then(Value::as_str)
        .is_some()
    {
        error_payload.pointer_mut("/response/error/message")
    } else if error_payload
        .pointer("/error/message")
        .and_then(Value::as_str)
        .is_some()
    {
        error_payload.pointer_mut("/error/message")
    } else if error_payload
        .pointer("/error/errors/0/message")
        .and_then(Value::as_str)
        .is_some()
    {
        error_payload.pointer_mut("/error/errors/0/message")
    } else if error_payload
        .pointer("/errors/0/message")
        .and_then(Value::as_str)
        .is_some()
    {
        error_payload.pointer_mut("/errors/0/message")
    } else if error_payload
        .get("message")
        .and_then(Value::as_str)
        .is_some()
    {
        error_payload.get_mut("message")
    } else if error_payload
        .get("detail")
        .and_then(Value::as_str)
        .is_some()
    {
        error_payload.get_mut("detail")
    } else if error_payload
        .get("error_description")
        .and_then(Value::as_str)
        .is_some()
    {
        error_payload.get_mut("error_description")
    } else if error_payload
        .pointer("/response/error")
        .and_then(Value::as_str)
        .is_some()
    {
        error_payload.pointer_mut("/response/error")
    } else if error_payload
        .get("error")
        .and_then(Value::as_str)
        .is_some_and(|error| !looks_like_error_code(error))
    {
        error_payload.get_mut("error")
    } else {
        None
    };
    if let Some(message_value) = message_field {
        let Some(original_message) = message_value.as_str() else {
            return false;
        };
        let prefixed_message = origin.prefix_message(&message_with_request_id(
            original_message,
            request_id.as_deref(),
        ));
        if prefixed_message == original_message {
            return false;
        }
        *message_value = Value::String(prefixed_message);
        return true;
    }
    false
}

/// Some clients show only `message` and ignore structured error metadata.
/// The caller supplies an ID validated by `UpstreamErrorDetails`.
pub(super) fn message_with_request_id(message: &str, request_id: Option<&str>) -> String {
    match request_id {
        Some(request_id) if !message.contains(request_id) => {
            format!("{} Request ID: {request_id}", message.trim_end())
        }
        _ => message.to_string(),
    }
}

fn looks_like_error_code(candidate_code: &str) -> bool {
    let trimmed_code = candidate_code.trim();
    !trimmed_code.is_empty()
        && trimmed_code
            .chars()
            .any(|character| matches!(character, '_' | '-'))
        && trimmed_code
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

pub(crate) fn prefix_error_body(response_body: &[u8], origin: ErrorOrigin) -> Vec<u8> {
    if response_body.is_empty() {
        return Vec::new();
    }
    if let Ok(mut error_payload) = serde_json::from_slice::<Value>(response_body) {
        if let Some(message) = error_payload.as_str() {
            let prefixed_message = origin.prefix_message(message);
            if let Ok(serialized_body) = serde_json::to_vec(&prefixed_message) {
                return serialized_body;
            }
        }
        if prefix_error_value(&mut error_payload, origin) {
            if let Ok(serialized_body) = serde_json::to_vec(&error_payload) {
                return serialized_body;
            }
        }
        return response_body.to_vec();
    }

    let Ok(body_text) = std::str::from_utf8(response_body) else {
        return response_body.to_vec();
    };
    origin.prefix_message(body_text).into_bytes()
}

pub(crate) fn api_error_type(status: StatusCode, code: &str) -> &'static str {
    error_codes::public_type(status.as_u16(), code)
}

pub(crate) fn api_error_code(code: &str) -> &str {
    error_codes::public_code(code)
}
