use super::super::errors::{
    classify_upstream_error, is_deactivated_workspace_value, previous_response_not_found_value,
    rate_limit_body_hint_value, upstream_event_failure_category, upstream_failure_status,
    upstream_status_from_value, RateLimitBodyHint,
};
use super::super::now_ms;
use crate::error_codes;
use axum::http::header::{HeaderName, HeaderValue, RETRY_AFTER};
use axum::http::{HeaderMap, StatusCode};
use serde_json::Value;

#[derive(Default)]
pub(super) struct EventTerminal {
    pub(super) upstream_error: Option<crate::usage::UpstreamErrorDetails>,
    pub(super) outcome: Option<EventTerminalOutcome>,
    /// Completed Responses object used to materialize a safe native replay.
    pub(super) response: Option<Value>,
    pub(super) status: Option<StatusCode>,
    pub(super) error_category: Option<&'static str>,
    pub(super) headers: HeaderMap,
    pub(super) body_hint: RateLimitBodyHint,
    pub(super) previous_response_not_found: bool,
    pub(super) deactivated_workspace: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EventTerminalOutcome {
    Success,
    Incomplete,
    Failure,
}

pub(super) fn event_terminal(event_payload: &Value) -> EventTerminal {
    let event_type = event_payload.get("type").and_then(Value::as_str);
    let mut outcome = match event_type {
        Some("response.completed" | "response.done") => Some(EventTerminalOutcome::Success),
        Some("response.incomplete") => Some(EventTerminalOutcome::Incomplete),
        Some("response.failed" | "response.cancelled" | "response.canceled" | "error") => {
            Some(EventTerminalOutcome::Failure)
        }
        _ => None,
    };
    let error_category = upstream_event_failure_category(event_type, event_payload);
    if let Some(category) = error_category {
        let explicitly_incomplete = outcome == Some(EventTerminalOutcome::Incomplete)
            || matches!(event_type, Some("response.completed" | "response.done"))
                && event_payload
                    .pointer("/response/status")
                    .and_then(Value::as_str)
                    == Some("incomplete");
        outcome = Some(
            if category == error_codes::RESPONSE_INCOMPLETE && explicitly_incomplete {
                EventTerminalOutcome::Incomplete
            } else {
                EventTerminalOutcome::Failure
            },
        );
    }
    let status = upstream_status_from_value(event_payload);
    EventTerminal {
        upstream_error: (outcome == Some(EventTerminalOutcome::Failure))
            .then(|| crate::usage::UpstreamErrorDetails::from_value(None, event_payload)),
        outcome,
        response: event_payload.get("response").cloned(),
        status,
        error_category,
        headers: websocket_retry_headers(event_payload),
        body_hint: rate_limit_body_hint_value(event_payload, std::time::SystemTime::now()),
        previous_response_not_found: previous_response_not_found_value(event_payload),
        deactivated_workspace: is_deactivated_workspace_value(event_payload),
    }
}

pub(super) fn incomplete_status(category: &str) -> Option<StatusCode> {
    match category {
        error_codes::WEBSOCKET_IDLE_TIMEOUT => Some(StatusCode::GATEWAY_TIMEOUT),
        error_codes::STREAM_SEMANTIC_TIMEOUT => Some(StatusCode::GATEWAY_TIMEOUT),
        error_codes::STREAM_EVENT_TOO_LARGE
        | error_codes::UPSTREAM_TRANSPORT
        | error_codes::UPSTREAM_WEBSOCKET
        | error_codes::UPSTREAM_WEBSOCKET_CLOSED => Some(StatusCode::BAD_GATEWAY),
        _ => None,
    }
}

pub(super) fn incomplete_requires_cooldown(category: &str) -> bool {
    matches!(
        category,
        error_codes::STREAM_EVENT_TOO_LARGE
            | error_codes::UPSTREAM_TRANSPORT
            | error_codes::UPSTREAM_WEBSOCKET
            | error_codes::UPSTREAM_WEBSOCKET_CLOSED
            | error_codes::WEBSOCKET_IDLE_TIMEOUT
            | error_codes::STREAM_SEMANTIC_TIMEOUT
    )
}

pub(super) fn terminal_failure_status(status: Option<StatusCode>) -> StatusCode {
    status
        .filter(|status| !status.is_success())
        .unwrap_or(StatusCode::BAD_GATEWAY)
}

/// Category and non-success status for one terminal event. Callers that need
/// the gateway's canonical status apply that separately.
pub(super) fn resolved_terminal_failure(terminal: &EventTerminal) -> (StatusCode, &'static str) {
    let category = terminal.error_category.unwrap_or_else(|| {
        classify_upstream_error(terminal_failure_status(terminal.status), None).category
    });
    let status = terminal
        .status
        .filter(|status| !status.is_success())
        .unwrap_or_else(|| upstream_failure_status(category));
    (status, category)
}

pub(super) fn websocket_retry_headers(event_payload: &Value) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let retry_after = websocket_reset_delay_seconds(event_payload, now_ms() / 1_000)
        .map(|seconds| seconds.to_string())
        .or_else(|| {
            event_payload
                .pointer("/headers/retry-after")
                .or_else(|| event_payload.pointer("/headers/retry_after"))
                .or_else(|| event_payload.pointer("/body/headers/retry-after"))
                .or_else(|| event_payload.pointer("/body/error/resets_in_seconds"))
                .or_else(|| event_payload.pointer("/error/resets_in_seconds"))
                .and_then(|header_value| match header_value {
                    Value::String(header_text) => Some(header_text.clone()),
                    Value::Number(header_number) => Some(header_number.to_string()),
                    _ => None,
                })
        });
    if let Some(retry_header_value) = retry_after
        .filter(|retry_text| retry_text.len() <= 128)
        .and_then(|retry_text| HeaderValue::from_str(&retry_text).ok())
    {
        headers.insert(RETRY_AFTER, retry_header_value);
    }
    for header_name in [
        "x-codex-primary-used-percent",
        "x-codex-primary-reset-after-seconds",
        "x-codex-primary-window-minutes",
        "x-codex-secondary-used-percent",
        "x-codex-secondary-reset-after-seconds",
        "x-codex-secondary-window-minutes",
    ] {
        if let Some(header_value) = websocket_header_value(event_payload, header_name)
            .filter(|header_text| header_text.len() <= 128)
            .and_then(|header_text| HeaderValue::from_str(&header_text).ok())
        {
            headers.insert(HeaderName::from_static(header_name), header_value);
        }
    }
    headers
}

fn websocket_header_value(event_payload: &Value, header_name: &str) -> Option<String> {
    let alternate_name = header_name.replace('-', "_");
    [
        event_payload.get("headers"),
        event_payload.pointer("/body/headers"),
        event_payload.pointer("/response/headers"),
    ]
    .into_iter()
    .flatten()
    .find_map(|headers| {
        headers
            .get(header_name)
            .or_else(|| headers.get(&alternate_name))
    })
    .and_then(|header_value| match header_value {
        Value::String(header_text) => Some(header_text.clone()),
        Value::Number(header_number) => Some(header_number.to_string()),
        _ => None,
    })
}

pub(super) fn websocket_reset_delay_seconds(
    event_payload: &Value,
    now_seconds: u64,
) -> Option<u64> {
    let reset_at = event_payload
        .pointer("/body/error/resets_at")
        .or_else(|| event_payload.pointer("/response/error/resets_at"))
        .or_else(|| event_payload.pointer("/error/resets_at"))?;
    let mut reset_at = reset_at.as_u64().or_else(|| {
        reset_at
            .as_str()
            .and_then(|reset_text| reset_text.parse().ok())
    })?;
    if reset_at > 10_000_000_000 {
        reset_at /= 1_000;
    }
    reset_at
        .checked_sub(now_seconds)
        .filter(|seconds| *seconds > 0)
}
