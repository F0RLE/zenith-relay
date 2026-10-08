use serde_json::{json, Value};
use std::collections::VecDeque;

pub(super) fn merge_usage(usage_snapshot: &mut Option<Value>, next_usage: &Value) {
    let Some(next_usage_object) = next_usage.as_object() else {
        *usage_snapshot = Some(next_usage.clone());
        return;
    };
    let Some(previous_usage) = usage_snapshot.as_mut().and_then(Value::as_object_mut) else {
        *usage_snapshot = Some(next_usage.clone());
        return;
    };
    for (usage_key, next_value) in next_usage_object {
        if let (Some(previous_field), Some(next_field)) = (
            previous_usage
                .get_mut(usage_key)
                .and_then(Value::as_object_mut),
            next_value.as_object(),
        ) {
            for (nested_key, nested_value) in next_field {
                previous_field.insert(nested_key.clone(), nested_value.clone());
            }
        } else {
            previous_usage.insert(usage_key.clone(), next_value.clone());
        }
    }
}

pub(super) fn push_sse_frame(
    frames: &mut VecDeque<Vec<u8>>,
    event: &str,
    event_payload: &Value,
) -> bool {
    let Ok(encoded_payload) = serde_json::to_vec(event_payload) else {
        return false;
    };
    let mut frame = Vec::with_capacity(event.len() + encoded_payload.len() + 20);
    frame.extend_from_slice(b"event: ");
    frame.extend_from_slice(event.as_bytes());
    frame.extend_from_slice(b"\ndata: ");
    frame.extend_from_slice(&encoded_payload);
    frame.extend_from_slice(b"\n\n");
    frames.push_back(frame);
    true
}

/// Client-facing Responses failure shared by the native stream bridges.
/// The error envelope stays `invalid_request_error`; callers supply the id,
/// model, and adapter code. Routes with extra metadata keep their own payload.
pub(super) fn failed_responses_event(
    response_id: &str,
    model_id: &str,
    error_code: &str,
    error_message: &str,
) -> Value {
    json!({
        "type": "response.failed",
        "response": {
            "id": response_id,
            "object": "response",
            "status": "failed",
            "model": model_id,
            "output": [],
            "error": {
                "type": "invalid_request_error",
                "code": error_code,
                "message": error_message,
            }
        }
    })
}

pub(super) fn sse_done(event: &[u8]) -> bool {
    crate::protocol::sse_lines(event).any(|line| {
        line.strip_prefix(b"data:")
            .map(|line_payload| line_payload.trim_ascii() == b"[DONE]")
            .unwrap_or(false)
    })
}

pub(super) fn incremental_delta(previous_text: &str, incoming_text: &str) -> String {
    incoming_text
        .strip_prefix(previous_text)
        .unwrap_or(incoming_text)
        .to_string()
}

pub(super) fn parse_sse_data(event: &[u8]) -> Option<Value> {
    let sse_payload = crate::protocol::sse_data(event);
    (!sse_payload.is_empty())
        .then(|| serde_json::from_slice(&sse_payload).ok())
        .flatten()
}

pub(super) fn sse_event_has_data(event: &[u8]) -> bool {
    crate::protocol::sse_lines(event).any(|line| {
        line.strip_prefix(b"data:")
            .is_some_and(|line_payload| line_payload.iter().any(|byte| !byte.is_ascii_whitespace()))
    })
}

pub(super) fn is_ignorable_metadata_event(kind: &str) -> bool {
    matches!(
        kind,
        "message_metadata" | "content_block_metadata" | "citation" | "message_citation"
    ) || kind.ends_with("_metadata")
        || kind.ends_with("_citation")
}

pub(super) fn tool_arguments_value(arguments: &str) -> Option<Value> {
    if arguments.trim().is_empty() {
        return Some(json!({}));
    }
    serde_json::from_str::<Value>(arguments)
        .ok()
        .filter(Value::is_object)
}
