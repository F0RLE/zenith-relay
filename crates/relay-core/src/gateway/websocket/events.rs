use super::super::errors::{
    previous_response_not_found_value, rate_limit_body_hint_value, upstream_event_failure_category,
    upstream_status_from_value, RateLimitBodyHint,
};
use super::super::now_ms;
use axum::http::header::{HeaderName, HeaderValue, RETRY_AFTER};
use axum::http::{HeaderMap, StatusCode};
use serde_json::Value;

#[derive(Default)]
pub(super) struct EventTerminal {
    pub(super) outcome: Option<EventTerminalOutcome>,
    pub(super) status: Option<StatusCode>,
    pub(super) error_category: Option<&'static str>,
    pub(super) headers: HeaderMap,
    pub(super) body_hint: RateLimitBodyHint,
    pub(super) previous_response_not_found: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EventTerminalOutcome {
    Success,
    Incomplete,
    Failure,
}

pub(super) fn event_terminal(value: &Value) -> EventTerminal {
    let outcome = match value.get("type").and_then(Value::as_str) {
        Some("response.completed" | "response.done") => Some(EventTerminalOutcome::Success),
        Some("response.incomplete") => Some(EventTerminalOutcome::Incomplete),
        Some("response.failed" | "response.cancelled" | "response.canceled" | "error") => {
            Some(EventTerminalOutcome::Failure)
        }
        _ => None,
    };
    let status = upstream_status_from_value(value);
    EventTerminal {
        outcome,
        status,
        error_category: upstream_event_failure_category(
            value.get("type").and_then(Value::as_str),
            value,
        ),
        headers: websocket_retry_headers(value),
        body_hint: rate_limit_body_hint_value(value, std::time::SystemTime::now()),
        previous_response_not_found: previous_response_not_found_value(value),
    }
}

pub(super) fn incomplete_status(category: &str) -> Option<StatusCode> {
    match category {
        "websocket_idle_timeout" => Some(StatusCode::GATEWAY_TIMEOUT),
        "stream_semantic_timeout" => Some(StatusCode::GATEWAY_TIMEOUT),
        "stream_event_too_large"
        | "upstream_transport"
        | "upstream_websocket"
        | "upstream_websocket_closed" => Some(StatusCode::BAD_GATEWAY),
        _ => None,
    }
}

pub(super) fn incomplete_requires_cooldown(category: &str) -> bool {
    matches!(
        category,
        "stream_event_too_large"
            | "upstream_transport"
            | "upstream_websocket"
            | "upstream_websocket_closed"
            | "websocket_idle_timeout"
            | "stream_semantic_timeout"
    )
}

pub(super) fn terminal_failure_status(status: Option<StatusCode>) -> StatusCode {
    status
        .filter(|status| !status.is_success())
        .unwrap_or(StatusCode::BAD_GATEWAY)
}

pub(super) fn websocket_retry_headers(value: &Value) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let retry_after = websocket_reset_delay_seconds(value, now_ms() / 1_000)
        .map(|seconds| seconds.to_string())
        .or_else(|| {
            value
                .pointer("/headers/retry-after")
                .or_else(|| value.pointer("/headers/retry_after"))
                .or_else(|| value.pointer("/body/headers/retry-after"))
                .or_else(|| value.pointer("/body/error/resets_in_seconds"))
                .or_else(|| value.pointer("/error/resets_in_seconds"))
                .and_then(|value| match value {
                    Value::String(value) => Some(value.clone()),
                    Value::Number(value) => Some(value.to_string()),
                    _ => None,
                })
        });
    if let Some(value) = retry_after
        .filter(|value| value.len() <= 128)
        .and_then(|value| HeaderValue::from_str(&value).ok())
    {
        headers.insert(RETRY_AFTER, value);
    }
    for name in [
        "x-codex-primary-used-percent",
        "x-codex-primary-reset-after-seconds",
        "x-codex-primary-window-minutes",
        "x-codex-secondary-used-percent",
        "x-codex-secondary-reset-after-seconds",
        "x-codex-secondary-window-minutes",
    ] {
        if let Some(value) = websocket_header_value(value, name)
            .filter(|value| value.len() <= 128)
            .and_then(|value| HeaderValue::from_str(&value).ok())
        {
            headers.insert(HeaderName::from_static(name), value);
        }
    }
    headers
}

fn websocket_header_value(value: &Value, name: &str) -> Option<String> {
    let alternate = name.replace('-', "_");
    [
        value.get("headers"),
        value.pointer("/body/headers"),
        value.pointer("/response/headers"),
    ]
    .into_iter()
    .flatten()
    .find_map(|headers| headers.get(name).or_else(|| headers.get(&alternate)))
    .and_then(|value| match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

pub(super) fn websocket_reset_delay_seconds(value: &Value, now_seconds: u64) -> Option<u64> {
    let reset_at = value
        .pointer("/body/error/resets_at")
        .or_else(|| value.pointer("/response/error/resets_at"))
        .or_else(|| value.pointer("/error/resets_at"))?;
    let mut reset_at = reset_at
        .as_u64()
        .or_else(|| reset_at.as_str().and_then(|value| value.parse().ok()))?;
    if reset_at > 10_000_000_000 {
        reset_at /= 1_000;
    }
    reset_at
        .checked_sub(now_seconds)
        .filter(|seconds| *seconds > 0)
}
