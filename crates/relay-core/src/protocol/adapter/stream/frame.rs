use serde_json::{json, Value};
use std::collections::VecDeque;

pub(crate) fn merge_usage(usage_snapshot: &mut Option<Value>, next_usage: &Value) {
    if !next_usage.is_object() {
        return;
    }
    let snapshot = usage_snapshot.get_or_insert_with(|| json!({}));
    merge_usage_fields(snapshot, next_usage);
}

fn merge_usage_fields(snapshot: &mut Value, next: &Value) {
    let Some(next_fields) = next.as_object() else {
        return;
    };
    if !snapshot.is_object() {
        *snapshot = json!({});
    }
    let previous_fields = snapshot
        .as_object_mut()
        .expect("usage snapshot is an object");
    let counts_changed = [
        "input_tokens",
        "prompt_tokens",
        "output_tokens",
        "completion_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
        "promptTokenCount",
        "candidatesTokenCount",
        "thoughtsTokenCount",
    ]
    .iter()
    .any(|field| {
        next_fields.get(*field).is_some_and(|value| {
            value.as_u64().is_some() && previous_fields.get(*field) != Some(value)
        })
    });
    if counts_changed {
        // A prior total belongs to the prior counters. Derive a fresh total
        // when a later cumulative update changes them without a new total.
        for field in ["total_tokens", "totalTokenCount"] {
            if next_fields.get(field).and_then(Value::as_u64).is_none() {
                previous_fields.remove(field);
            }
        }
    }
    for (field, value) in next_fields {
        // Providers use null for unavailable counters, including in terminal
        // snapshots. It must not erase consumption reported in earlier frames.
        if value.is_null() {
            continue;
        }
        if value.is_object() {
            merge_usage_fields(
                previous_fields
                    .entry(field.clone())
                    .or_insert_with(|| json!({})),
                value,
            );
        } else {
            previous_fields.insert(field.clone(), value.clone());
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

pub(super) fn tool_arguments_value(arguments: &str) -> Option<Value> {
    if arguments.trim().is_empty() {
        return Some(json!({}));
    }
    serde_json::from_str::<Value>(arguments)
        .ok()
        .filter(Value::is_object)
}
