use crate::GatewayRuntime;
use serde_json::{Map, Value};

pub(in crate::gateway) fn drop_materialized_previous_response_id(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &mut Value,
    model: &str,
    now_ms: u64,
) -> bool {
    if super::compacted::reset_compacted_history(request) {
        return true;
    }
    let Some(previous_id) = request.get("previous_response_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(owner) = runtime
        .response_affinity_key(Some(previous_id))
        .and_then(|key| runtime.response_affinity_candidate(&key, now_ms))
    else {
        return false;
    };
    let Some(replay) =
        runtime.load_native_responses_replay(local_key_id, previous_id, &owner, now_ms)
    else {
        return false;
    };
    let Ok(mut materialized) = replay.replay_request(
        request,
        replay.model(),
        request
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    ) else {
        return false;
    };
    if !has_materialized_plaintext_history(&materialized)
        || request
            .get("conversation")
            .is_some_and(|conversation_value| !conversation_value.is_null())
        || request.get("context_management").is_some()
    {
        return false;
    }
    materialized["model"] = Value::String(model.to_string());
    *request = materialized;
    true
}

/// Replays a saved native Responses turn after the provider explicitly rejects
/// one unanswered function/custom-tool call. The replay and removal are staged
/// together so a failed or ambiguous repair leaves the original request intact.
pub(in crate::gateway) fn recover_stale_tool_history(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &mut Value,
    model: &str,
    now_ms: u64,
    stream: bool,
    upstream_error: &[u8],
) -> bool {
    if !super::super::errors::responses_tool_call_is_missing_output(upstream_error)
        || request.get("context_management").is_some()
        || request.get("truncation").is_some()
        || contains_encrypted_content(request)
    {
        return false;
    }
    let Some(previous_id) = request.get("previous_response_id").and_then(Value::as_str) else {
        return false;
    };
    let Some(current_input_item_count) = replay_input_item_count(request) else {
        return false;
    };
    let Some(owner) = runtime
        .response_affinity_key(Some(previous_id))
        .and_then(|key| runtime.response_affinity_candidate(&key, now_ms))
    else {
        return false;
    };
    let Some(replay) =
        runtime.load_native_responses_replay(local_key_id, previous_id, &owner, now_ms)
    else {
        return false;
    };
    let Ok(mut materialized) = replay.replay_request(request, replay.model(), stream) else {
        return false;
    };
    let Some(historical_item_count) = materialized
        .get("input")
        .and_then(Value::as_array)
        .and_then(|input_items| input_items.len().checked_sub(current_input_item_count))
    else {
        return false;
    };
    if !has_materialized_tool_history(&materialized)
        || !super::super::request::remove_unpaired_responses_tool_call(
            &mut materialized,
            historical_item_count,
            upstream_error,
        )
    {
        return false;
    }
    materialized["model"] = Value::String(model.to_string());
    *request = materialized;
    true
}

fn replay_input_item_count(request: &Value) -> Option<usize> {
    match request.get("input")? {
        Value::Array(input_items) => Some(input_items.len()),
        Value::String(_) | Value::Object(_) => Some(1),
        _ => None,
    }
}

fn has_materialized_tool_history(request: &Value) -> bool {
    if request.get("context_management").is_some()
        || request.get("truncation").is_some()
        || contains_encrypted_content(request)
        || request
            .get("conversation")
            .is_some_and(|conversation_value| !conversation_value.is_null())
    {
        return false;
    }
    let Some(input_items) = request.get("input").and_then(Value::as_array) else {
        return false;
    };
    !input_items.is_empty()
        && input_items.iter().all(|input_item| {
            let Some(item_object) = input_item.as_object() else {
                return false;
            };
            match item_object.get("type").and_then(Value::as_str) {
                Some("message") | None => item_object
                    .get("role")
                    .and_then(Value::as_str)
                    .is_some_and(|role| {
                        matches!(role, "user" | "assistant" | "developer" | "system")
                    }),
                Some(
                    "function_call"
                    | "function_call_output"
                    | "custom_tool_call"
                    | "custom_tool_call_output"
                    | "reasoning",
                ) => true,
                Some(_) => false,
            }
        })
}

fn has_materialized_plaintext_history(request: &Value) -> bool {
    if request.get("context_management").is_some()
        || request.get("truncation").is_some()
        || contains_encrypted_content(request)
        || contains_tool_state(request)
        || request
            .get("conversation")
            .is_some_and(|conversation_value| !conversation_value.is_null())
    {
        return false;
    }
    let Some(input_items) = request.get("input").and_then(Value::as_array) else {
        return false;
    };
    if input_items.is_empty() {
        return false;
    }

    for input_item in input_items {
        let Some(message_object) = input_item.as_object() else {
            return false;
        };
        if message_object
            .get("type")
            .is_some_and(|kind| kind.as_str() != Some("message"))
        {
            return false;
        }
        let Some(role) = message_object.get("role").and_then(Value::as_str) else {
            return false;
        };
        if !matches!(role, "user" | "assistant" | "developer" | "system")
            || !message_has_plaintext_content(message_object)
        {
            return false;
        }
    }
    true
}

pub(super) fn message_has_plaintext_content(message_object: &Map<String, Value>) -> bool {
    match message_object.get("content") {
        Some(Value::String(_)) => true,
        Some(Value::Array(parts)) => !parts.is_empty() && parts.iter().all(plaintext_content_part),
        _ => false,
    }
}

fn plaintext_content_part(content_part: &Value) -> bool {
    let Some(part_object) = content_part.as_object() else {
        return false;
    };
    match part_object.get("type").and_then(Value::as_str) {
        Some("input_text" | "output_text" | "text") => {
            part_object.get("text").is_some_and(Value::is_string)
        }
        Some("refusal") => part_object.get("refusal").is_some_and(Value::is_string),
        _ => false,
    }
}

pub(super) fn contains_encrypted_content(json_value: &Value) -> bool {
    match json_value {
        Value::Array(json_values) => json_values.iter().any(contains_encrypted_content),
        Value::Object(json_object) => {
            json_object.contains_key("encrypted_content")
                || json_object.values().any(contains_encrypted_content)
        }
        _ => false,
    }
}

fn contains_tool_state(request: &Value) -> bool {
    request
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|input_items| input_items.iter().any(is_tool_state_item))
}

pub(super) fn is_tool_state_item(input_item: &Value) -> bool {
    let Some(item_object) = input_item.as_object() else {
        return false;
    };
    let kind = item_object.get("type").and_then(Value::as_str);
    kind.is_some_and(|kind| {
        kind.ends_with("_call") || kind.ends_with("_call_output") || kind.ends_with("_output")
    }) || matches!(
        item_object.get("role").and_then(Value::as_str),
        Some("tool" | "function")
    ) || ["tool_calls", "tool_call_id", "function_call"]
        .iter()
        .any(|field| item_object.contains_key(*field))
}
