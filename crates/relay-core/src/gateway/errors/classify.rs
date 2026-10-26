use super::failure::zenith_gateway_invalid_request_value;
use super::upstream_failure_message;
use crate::error_codes;
use axum::http::StatusCode;
use serde_json::Value;

mod scoped;
mod text;
use text::classify_upstream_error_text;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::gateway) struct UpstreamErrorClassification {
    pub(in crate::gateway) category: &'static str,
    pub(in crate::gateway) message: &'static str,
}

pub(in crate::gateway) fn classify_upstream_error(
    status: StatusCode,
    response_body: Option<&[u8]>,
) -> UpstreamErrorClassification {
    let Some(response_body) = response_body else {
        return classify_upstream_error_text(status, "");
    };
    match serde_json::from_slice::<Value>(response_body) {
        Ok(error_payload) => classify_upstream_error_value(status, &error_payload),
        Err(_) => {
            if status == StatusCode::FORBIDDEN
                && let Some(error_payload) = scoped::error_before_sse(response_body)
            {
                return classify_upstream_error_value(status, &error_payload);
            }
            classify_upstream_error_text(status, &normalized_error_text(response_body))
        }
    }
}

pub(in crate::gateway) fn classify_upstream_error_value(
    status: StatusCode,
    error_payload: &Value,
) -> UpstreamErrorClassification {
    if matches!(status, StatusCode::FORBIDDEN | StatusCode::BAD_GATEWAY)
        && [
            "/code",
            "/type",
            "/error/code",
            "/error/type",
            "/error/upstream_error/code",
            "/error/upstream_error/type",
            "/response/error/code",
            "/response/error/type",
        ]
        .iter()
        .filter_map(|path| error_payload.pointer(path).and_then(Value::as_str))
        .any(|code| code == "subscription_sharing_user_not_eligible")
    {
        return UpstreamErrorClassification {
            category: error_codes::UPSTREAM_SIWC_NOT_ELIGIBLE,
            message: upstream_failure_message(error_codes::UPSTREAM_SIWC_NOT_ELIGIBLE),
        };
    }
    // Normalization and stream guards emit typed local failures into the stream.
    // Keep its exact category instead of falling back to upstream_terminal.
    // Only these representation failures are accepted, never arbitrary codes
    // or origin claims, and an HTTP authorization refusal still owns its status.
    if status == StatusCode::BAD_GATEWAY {
        for path in ["", "/error", "/response/error", "/body/error"] {
            if !path.is_empty()
                && error_payload.pointer(path).is_some_and(|error| {
                    error.get("type").and_then(Value::as_str) == Some(error_codes::STREAM_ERROR)
                        && error.get("code").and_then(Value::as_str)
                            == Some(error_codes::STREAM_EVENT_TOO_LARGE)
                })
            {
                return UpstreamErrorClassification {
                    category: error_codes::STREAM_EVENT_TOO_LARGE,
                    message: "Upstream streaming event exceeded Relay's size limit",
                };
            }
            if let Some(error) = error_payload.pointer(path).filter(|error| {
                let kind = error.get("type").and_then(Value::as_str);
                kind == Some(if path.is_empty() {
                    "error"
                } else {
                    "relay_adapter"
                })
            }) {
                let adapter = match error.get("code").and_then(Value::as_str) {
                    Some(error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID) => {
                        Some(crate::AdapterError::upstream_response_invalid())
                    }
                    Some(error_codes::ADAPTER_UPSTREAM_STREAM_INVALID) => {
                        Some(crate::AdapterError::upstream_stream_invalid())
                    }
                    _ => None,
                };
                if let Some(adapter) = adapter {
                    return UpstreamErrorClassification {
                        category: adapter.code(),
                        message: adapter.message(),
                    };
                }
            }
        }
    }
    if [
        "/code",
        "/error_code",
        "/error/code",
        "/detail/code",
        "/body/code",
        "/body/error/code",
        "/response/code",
        "/response/error/code",
    ]
    .iter()
    .filter_map(|path| error_payload.pointer(path).and_then(Value::as_str))
    .any(|code| code.eq_ignore_ascii_case("turn_rate_limit_exceeded"))
    {
        return UpstreamErrorClassification {
            category: error_codes::UPSTREAM_TURN_RATE_LIMITED,
            message: upstream_failure_message(error_codes::UPSTREAM_TURN_RATE_LIMITED),
        };
    }
    if zenith_gateway_invalid_request_value(error_payload) {
        return UpstreamErrorClassification {
            // The legacy gateway envelope does not prove a route failure or
            // a repairable continuation. Return the rejection without cooling
            // the candidate or replaying the same request on another route.
            category: error_codes::UPSTREAM_INVALID_REQUEST,
            message: upstream_failure_message(error_codes::UPSTREAM_INVALID_REQUEST),
        };
    }
    let classification = classify_upstream_error_text(status, &upstream_error_text(error_payload));
    // Narrow compatibility evidence may refine a generic refusal, never hide
    // an explicit account, workspace, quota or policy discriminator.
    if matches!(
        classification.category,
        error_codes::UPSTREAM_STATUS
            | error_codes::UPSTREAM_FORBIDDEN
            | error_codes::UPSTREAM_ERROR
            | error_codes::UPSTREAM_EDGE_CHALLENGE
    ) && let Some(category) = scoped::forbidden_category(status, error_payload)
    {
        return UpstreamErrorClassification {
            category,
            message: upstream_failure_message(category),
        };
    }
    classification
}

pub(crate) fn is_deactivated_workspace_value(error_payload: &Value) -> bool {
    [
        "/detail/code",
        "/error/code",
        "/body/error/code",
        "/response/error/code",
    ]
    .into_iter()
    .filter_map(|path| error_payload.pointer(path).and_then(Value::as_str))
    .any(|code| code.eq_ignore_ascii_case("deactivated_workspace"))
}

pub(crate) fn is_deactivated_workspace(response_body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(response_body)
        .ok()
        .is_some_and(|error_payload| is_deactivated_workspace_value(&error_payload))
}

pub(super) fn upstream_error_text(error_payload: &Value) -> String {
    const PATHS: &[&str] = &[
        "/code",
        "/type",
        "/message",
        "/msg",
        "/err",
        "/error_msg",
        "/detail",
        "/error_code",
        "/error",
        "/error/code",
        "/error/type",
        "/error/message",
        "/error/detail",
        "/error/status",
        "/detail/code",
        "/detail/type",
        "/detail/message",
        "/body/code",
        "/body/type",
        "/body/message",
        "/body/error",
        "/body/error/code",
        "/body/error/type",
        "/body/error/message",
        "/response/code",
        "/response/type",
        "/response/message",
        "/response/error",
        "/response/error/code",
        "/response/error/type",
        "/response/error/message",
        "/response/incomplete_details/reason",
        "/header/message",
    ];
    let mut text = String::new();
    for error_text in PATHS
        .iter()
        .filter_map(|path| error_payload.pointer(path).and_then(Value::as_str))
    {
        if !text.is_empty() {
            text.push(' ');
        }
        text.extend(
            error_text
                .chars()
                .take(4_096)
                .map(|character| character.to_ascii_lowercase()),
        );
    }
    text
}

pub(super) fn normalized_error_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .take(4_096)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

pub(super) fn text_has_any(text: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| text.contains(marker))
}
