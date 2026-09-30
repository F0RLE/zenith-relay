use serde_json::{json, Value};
use std::collections::VecDeque;

pub(super) fn merge_usage(target: &mut Option<Value>, next: &Value) {
    let Some(next_object) = next.as_object() else {
        *target = Some(next.clone());
        return;
    };
    let Some(previous) = target.as_mut().and_then(Value::as_object_mut) else {
        *target = Some(next.clone());
        return;
    };
    for (key, value) in next_object {
        if let (Some(previous_object), Some(next_object)) = (
            previous.get_mut(key).and_then(Value::as_object_mut),
            value.as_object(),
        ) {
            for (nested_key, nested_value) in next_object {
                previous_object.insert(nested_key.clone(), nested_value.clone());
            }
        } else {
            previous.insert(key.clone(), value.clone());
        }
    }
}

pub(super) fn push_sse_frame(output: &mut VecDeque<Vec<u8>>, event: &str, payload: &Value) -> bool {
    let Ok(payload) = serde_json::to_vec(payload) else {
        return false;
    };
    let mut frame = Vec::with_capacity(event.len() + payload.len() + 20);
    frame.extend_from_slice(b"event: ");
    frame.extend_from_slice(event.as_bytes());
    frame.extend_from_slice(b"\ndata: ");
    frame.extend_from_slice(&payload);
    frame.extend_from_slice(b"\n\n");
    output.push_back(frame);
    true
}

/// Client-facing Responses failure shared by the native stream bridges.
/// The error envelope stays `invalid_request_error`; callers supply the id,
/// model, and adapter code. Routes with extra metadata keep their own payload.
pub(super) fn failed_responses_event(id: &str, model: &str, code: &str, message: &str) -> Value {
    json!({
        "type": "response.failed",
        "response": {
            "id": id,
            "object": "response",
            "status": "failed",
            "model": model,
            "output": [],
            "error": {
                "type": "invalid_request_error",
                "code": code,
                "message": message,
            }
        }
    })
}

pub(super) fn sse_done(event: &[u8]) -> bool {
    crate::protocol::sse_lines(event).any(|line| {
        line.strip_prefix(b"data:")
            .map(|value| value.trim_ascii() == b"[DONE]")
            .unwrap_or(false)
    })
}

pub(super) fn incremental_delta(previous: &str, incoming: &str) -> String {
    incoming
        .strip_prefix(previous)
        .unwrap_or(incoming)
        .to_string()
}

pub(super) fn parse_sse_data(event: &[u8]) -> Option<Value> {
    let data = crate::protocol::sse_data(event);
    (!data.is_empty())
        .then(|| serde_json::from_slice(&data).ok())
        .flatten()
}

pub(super) fn sse_event_has_data(event: &[u8]) -> bool {
    crate::protocol::sse_lines(event).any(|line| {
        line.strip_prefix(b"data:")
            .is_some_and(|value| value.iter().any(|byte| !byte.is_ascii_whitespace()))
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
