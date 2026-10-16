//! Replay native Responses history and repair client-visible item ids.

use super::{AdapterError, AdapterResult};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

/// Local history for native Responses recovery before client-visible output.
///
/// The initial request and completed output are kept in memory so the next
/// request can replay the conversation without pretending that the upstream
/// response id is portable across transports. This is deliberately separate
/// from `ResponsesToMessages`: no protocol conversion happens here.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NativeResponsesReplayState {
    pub(super) model: String,
    initial_request: Value,
    completed_output: Vec<Value>,
}

impl NativeResponsesReplayState {
    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    pub fn from_response(
        initial_request: &Value,
        model: &str,
        upstream_response: &Value,
    ) -> Option<(String, Self)> {
        let response_body = upstream_response
            .pointer("/response/response")
            .or_else(|| upstream_response.get("response"))
            .unwrap_or(upstream_response);
        let response_id = response_body
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|response_id_value| !response_id_value.is_empty())?
            .to_string();
        if native_replay_has_provider_state(initial_request) {
            return None;
        }
        let initial_request = initial_request.as_object()?.clone();
        if !initial_request.contains_key("input") {
            return None;
        }
        let initial_input = initial_request.get("input")?;
        let input_items = initial_input
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_else(|| std::slice::from_ref(initial_input));
        if !native_tool_outputs_are_materialized(input_items) {
            return None;
        }
        // The stored request must already be self-contained. If a predecessor
        // was unavailable, retaining this opaque id would create a replay that
        // appears valid while silently losing the earlier conversation.
        if initial_request
            .get("previous_response_id")
            .and_then(Value::as_str)
            .is_some_and(|response_id_value| !response_id_value.trim().is_empty())
        {
            return None;
        }
        // A native replay is safe only when the completed response contains
        // its output items. Never store a request-only snapshot: replaying it
        // after a quota handoff would silently drop tool/context output.
        let completed_output = response_body
            .get("output")
            .and_then(Value::as_array)?
            .clone();
        Some((
            response_id,
            Self {
                model: model.to_string(),
                initial_request: Value::Object(initial_request),
                completed_output,
            },
        ))
    }

    /// Builds a new native Responses request with the prior turn materialized
    /// in `input`, so an unavailable owner or rejected response reference can
    /// recover before the stream is committed.
    pub fn replay_request(
        &self,
        continuation: &Value,
        model: &str,
        stream: bool,
    ) -> AdapterResult<Value> {
        if !self.model.eq_ignore_ascii_case(model) || native_replay_has_provider_state(continuation)
        {
            return Err(AdapterError::continuation_mismatch());
        }
        let continuation_request = continuation
            .as_object()
            .ok_or_else(AdapterError::invalid_request)?;
        let initial_input = self
            .initial_request
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?;
        let current_input = continuation_request
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?;
        let mut replay_input = Vec::new();
        append_replay_input(&mut replay_input, initial_input)?;
        replay_input.extend(self.completed_output.iter().cloned());
        append_replay_input(&mut replay_input, current_input)?;
        if replay_input.is_empty() {
            return Err(AdapterError::invalid_request());
        }
        if !native_tool_outputs_are_materialized(&replay_input) {
            return Err(AdapterError::continuation_mismatch());
        }

        // Only history crosses turns. Reusing the old request template would
        // revive old instructions, output settings or transport control fields.
        let mut replay_request = continuation_request.clone();
        replay_request.remove("previous_response_id");
        replay_request.insert("model".to_string(), Value::String(model.to_string()));
        replay_request.insert("stream".to_string(), Value::Bool(stream));
        replay_request.insert("input".to_string(), Value::Array(replay_input));
        Ok(Value::Object(replay_request))
    }
}

fn native_tool_outputs_are_materialized(input_items: &[Value]) -> bool {
    let mut calls = BTreeSet::new();
    for input_item in input_items {
        let Some(kind) = input_item.get("type").and_then(Value::as_str) else {
            continue;
        };
        let call_id = input_item
            .get("call_id")
            .and_then(Value::as_str)
            .filter(|call_id_value| !call_id_value.trim().is_empty());
        let output_call_kind = if kind == "tool_search_output" {
            Some("tool_search_call")
        } else if kind.ends_with("_call_output") {
            kind.strip_suffix("_output")
        } else {
            None
        };
        if let Some(call_kind) = output_call_kind {
            if !call_id.is_some_and(|call_id_value| calls.remove(&(call_kind, call_id_value))) {
                return false;
            }
        } else if kind.ends_with("_call") {
            if let Some(call_id_value) = call_id {
                calls.insert((kind, call_id_value));
            }
        }
    }
    true
}

fn native_replay_has_provider_state(request_body: &Value) -> bool {
    if request_body
        .get("conversation")
        .is_some_and(|conversation_value| !conversation_value.is_null())
    {
        return true;
    }
    let is_reference = |input_item: &Value| {
        input_item.get("type").and_then(Value::as_str) == Some("item_reference")
            || (input_item.get("id").is_some()
                && input_item.get("type").is_none()
                && input_item.get("role").is_none())
    };
    match request_body.get("input") {
        Some(Value::Array(input_items)) => input_items.iter().any(is_reference),
        Some(input_item @ Value::Object(_)) => is_reference(input_item),
        _ => false,
    }
}

fn append_replay_input(replay_input: &mut Vec<Value>, input_value: &Value) -> AdapterResult<()> {
    match input_value {
        Value::String(text) => replay_input.push(json!({
            "role": "user",
            "content": [{"type": "input_text", "text": text}],
        })),
        Value::Array(input_items) => replay_input.extend(input_items.iter().cloned()),
        Value::Object(input_item) => replay_input.push(Value::Object(input_item.clone())),
        _ => return Err(AdapterError::invalid_request()),
    }
    Ok(())
}

fn prefixed_id(identifier: &str, prefix: &str) -> String {
    if identifier.starts_with(prefix) {
        identifier.to_string()
    } else {
        format!("{prefix}{identifier}")
    }
}

fn repair_input_item_ids(
    request_body: &mut Value,
    mut repair_input_item: impl FnMut(&mut Map<String, Value>) -> bool,
) -> bool {
    let Some(input_items) = request_body.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut repaired = false;
    for input_item in input_items {
        let Some(input_item) = input_item.as_object_mut() else {
            continue;
        };
        repaired |= repair_input_item(input_item);
    }
    repaired
}

/// Repairs a historic Responses function item only after a strict upstream has
/// rejected its item-id namespace.
///
/// `call_id` is the stable link used by `function_call_output`; the item `id`
/// is a separate opaque Responses item identifier. Some compatible upstreams
/// emit the call identifier in both fields, but strict Responses endpoints
/// require the function item identifier to use their `fc_` namespace. Keeping
/// this repair narrow lets native routes stay byte-for-byte passthrough until
/// an upstream proves that its stricter item contract is required.
pub(crate) fn repair_call_prefixed_function_item_ids(request_body: &mut Value) -> bool {
    repair_input_item_ids(request_body, |input_item| {
        if input_item.get("type").and_then(Value::as_str) != Some("function_call") {
            return false;
        }
        let Some(item_id) = input_item.get("id").and_then(Value::as_str) else {
            return false;
        };
        if item_id.starts_with("fc_") || item_id.is_empty() {
            return false;
        }
        input_item.insert("id".to_string(), Value::String(prefixed_id(item_id, "fc_")));
        true
    })
}

/// Strict Responses endpoints use a separate `ctc_` namespace for
/// `custom_tool_call.id`. The `call_id` remains the stable link used by the
/// matching `custom_tool_call_output`, so only the item identifier is changed.
pub(in crate::protocol::adapter) fn custom_tool_item_id(call_id: &str) -> String {
    prefixed_id(call_id.trim(), "ctc_")
}

/// Repairs a historic Responses custom-tool item only after a strict upstream
/// has rejected its item-id namespace. This is deliberately separate from the
/// function-call repair because the two item types have different namespaces.
pub(crate) fn repair_custom_tool_item_ids(request_body: &mut Value) -> bool {
    repair_input_item_ids(request_body, |input_item| {
        let Some(item_id) = input_item.get("id").and_then(Value::as_str) else {
            return false;
        };
        if input_item.get("type").and_then(Value::as_str) != Some("custom_tool_call") {
            return false;
        }
        let normalized = custom_tool_item_id(item_id);
        if normalized == item_id {
            return false;
        }
        input_item.insert("id".to_string(), Value::String(normalized));
        true
    })
}

/// Drops only foreign `item_` identifiers from message inputs after a strict
/// native Responses endpoint rejects them. Message item IDs are opaque and
/// server-owned, so Relay must not fabricate a `msg_` replacement. Preserve
/// native `msg_` IDs and every non-message item (especially reasoning and
/// tool-call links) exactly as the client supplied them.
pub(crate) fn remove_item_prefixed_message_ids(request_body: &mut Value) -> bool {
    repair_input_item_ids(request_body, |input_item| {
        let is_message = input_item.get("type").and_then(Value::as_str) == Some("message")
            || matches!(
                input_item.get("role").and_then(Value::as_str),
                Some("user" | "assistant" | "developer" | "system")
            );
        if !is_message
            || !input_item
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|item_id| item_id.starts_with("item_"))
        {
            return false;
        }
        input_item.remove("id");
        true
    })
}
