use super::failure::zenith_gateway_invalid_request_value;
use super::upstream_failure_message;
use crate::error_codes;
use axum::http::StatusCode;
use serde_json::Value;

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
        Err(_) => classify_upstream_error_text(status, &normalized_error_text(response_body)),
    }
}

pub(in crate::gateway) fn classify_upstream_error_value(
    status: StatusCode,
    error_payload: &Value,
) -> UpstreamErrorClassification {
    if zenith_gateway_invalid_request_value(error_payload) {
        return UpstreamErrorClassification {
            // This gateway envelope hides the actual cause, including route
            // and model access failures. It does not prove invalid client input.
            category: error_codes::UPSTREAM_CANDIDATE_REJECTED,
            message: upstream_failure_message(error_codes::UPSTREAM_CANDIDATE_REJECTED),
        };
    }
    classify_upstream_error_text(status, &upstream_error_text(error_payload))
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
