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
    request: Value,
    output: Vec<Value>,
}

impl NativeResponsesReplayState {
    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    pub fn from_response(request: &Value, model: &str, upstream: &Value) -> Option<(String, Self)> {
        let response = upstream
            .pointer("/response/response")
            .or_else(|| upstream.get("response"))
            .unwrap_or(upstream);
        let response_id = response
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())?
            .to_string();
        if native_replay_has_provider_state(request) {
            return None;
        }
        let request = request.as_object()?.clone();
        if !request.contains_key("input") {
            return None;
        }
        let input = request.get("input")?;
        let items = input
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_else(|| std::slice::from_ref(input));
        if !native_tool_outputs_are_materialized(items) {
            return None;
        }
        // The stored request must already be self-contained. If a predecessor
        // was unavailable, retaining this opaque id would create a replay that
        // appears valid while silently losing the earlier conversation.
        if request
            .get("previous_response_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.trim().is_empty())
        {
            return None;
        }
        // A native replay is safe only when the completed response contains
        // its output items. Never store a request-only snapshot: replaying it
        // after a quota handoff would silently drop tool/context output.
        let output = response.get("output").and_then(Value::as_array)?.clone();
        Some((
            response_id,
            Self {
                model: model.to_string(),
                request: Value::Object(request),
                output,
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
        let continuation = continuation
            .as_object()
            .ok_or_else(AdapterError::invalid_request)?;
        let initial_input = self
            .request
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?;
        let current_input = continuation
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?;
        let mut input = Vec::new();
        append_replay_input(&mut input, initial_input)?;
        input.extend(self.output.iter().cloned());
        append_replay_input(&mut input, current_input)?;
        if input.is_empty() {
            return Err(AdapterError::invalid_request());
        }
        if !native_tool_outputs_are_materialized(&input) {
            return Err(AdapterError::continuation_mismatch());
        }

        // Only history crosses turns. Reusing the old request template would
        // revive old instructions, output settings or transport control fields.
        let mut request = continuation.clone();
        request.remove("previous_response_id");
        request.insert("model".to_string(), Value::String(model.to_string()));
        request.insert("stream".to_string(), Value::Bool(stream));
        request.insert("input".to_string(), Value::Array(input));
        Ok(Value::Object(request))
    }
}

fn native_tool_outputs_are_materialized(items: &[Value]) -> bool {
    let mut calls = BTreeSet::new();
    for item in items {
        let Some(kind) = item.get("type").and_then(Value::as_str) else {
            continue;
        };
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty());
        let output_call_kind = if kind == "tool_search_output" {
            Some("tool_search_call")
        } else if kind.ends_with("_call_output") {
            kind.strip_suffix("_output")
        } else {
            None
        };
        if let Some(call_kind) = output_call_kind {
            if !call_id.is_some_and(|id| calls.remove(&(call_kind, id))) {
                return false;
            }
        } else if kind.ends_with("_call") {
            if let Some(id) = call_id {
                calls.insert((kind, id));
            }
        }
    }
    true
}

fn native_replay_has_provider_state(request: &Value) -> bool {
    if request
        .get("conversation")
        .is_some_and(|value| !value.is_null())
    {
        return true;
    }
    let is_reference = |item: &Value| {
        item.get("type").and_then(Value::as_str) == Some("item_reference")
            || (item.get("id").is_some()
                && item.get("type").is_none()
                && item.get("role").is_none())
    };
    match request.get("input") {
        Some(Value::Array(items)) => items.iter().any(is_reference),
        Some(item @ Value::Object(_)) => is_reference(item),
        _ => false,
    }
}

fn append_replay_input(target: &mut Vec<Value>, input: &Value) -> AdapterResult<()> {
    match input {
        Value::String(text) => target.push(json!({
            "role": "user",
            "content": [{"type": "input_text", "text": text}],
        })),
        Value::Array(items) => target.extend(items.iter().cloned()),
        Value::Object(item) => target.push(Value::Object(item.clone())),
        _ => return Err(AdapterError::invalid_request()),
    }
    Ok(())
}

fn prefixed_id(id: &str, prefix: &str) -> String {
    if id.starts_with(prefix) {
        id.to_string()
    } else {
        format!("{prefix}{id}")
    }
}

fn repair_input_item_ids(
    request: &mut Value,
    mut repair_item: impl FnMut(&mut Map<String, Value>) -> bool,
) -> bool {
    let Some(input) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut repaired = false;
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        repaired |= repair_item(item);
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
pub(crate) fn repair_call_prefixed_function_item_ids(request: &mut Value) -> bool {
    repair_input_item_ids(request, |item| {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return false;
        }
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            return false;
        };
        if id.starts_with("fc_") || id.is_empty() {
            return false;
        }
        item.insert("id".to_string(), Value::String(prefixed_id(id, "fc_")));
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
pub(crate) fn repair_custom_tool_item_ids(request: &mut Value) -> bool {
    repair_input_item_ids(request, |item| {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            return false;
        };
        if item.get("type").and_then(Value::as_str) != Some("custom_tool_call") {
            return false;
        }
        let normalized = custom_tool_item_id(id);
        if normalized == id {
            return false;
        }
        item.insert("id".to_string(), Value::String(normalized));
        true
    })
}

/// Drops only foreign `item_` identifiers from message inputs after a strict
/// native Responses endpoint rejects them. Message item IDs are opaque and
/// server-owned, so Relay must not fabricate a `msg_` replacement. Preserve
/// native `msg_` IDs and every non-message item (especially reasoning and
/// tool-call links) exactly as the client supplied them.
pub(crate) fn remove_item_prefixed_message_ids(request: &mut Value) -> bool {
    repair_input_item_ids(request, |item| {
        let is_message = item.get("type").and_then(Value::as_str) == Some("message")
            || matches!(
                item.get("role").and_then(Value::as_str),
                Some("user" | "assistant" | "developer" | "system")
            );
        if !is_message
            || !item
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with("item_"))
        {
            return false;
        }
        item.remove("id");
        true
    })
}
