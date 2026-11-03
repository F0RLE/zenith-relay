use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// A compaction item is a stateless context checkpoint, not an opaque response
/// reference. Keep the whole supplied window, including retained items; only
/// remove the redundant predecessor when no unresolved references remain.
pub(super) fn reset_compacted_history(request: &mut Value) -> bool {
    if !request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|response_id| !response_id.trim().is_empty())
        || !has_compacted_history(request)
    {
        return false;
    }
    request
        .as_object_mut()
        .is_some_and(|request_object| request_object.remove("previous_response_id").is_some())
}

fn has_compacted_history(request: &Value) -> bool {
    if request
        .get("conversation")
        .is_some_and(|conversation_value| !conversation_value.is_null())
    {
        return false;
    }
    let Some(history_items) = request.get("input").and_then(Value::as_array) else {
        return false;
    };
    let mut has_checkpoint = false;
    let mut pending_calls = BTreeMap::new();
    let mut seen_calls = BTreeSet::new();
    for history_item in history_items {
        let Some(item_object) = history_item.as_object() else {
            return false;
        };
        match item_object.get("type").and_then(Value::as_str) {
            Some(item_type) if crate::protocol::is_compaction_checkpoint_type(item_type) => {
                if nonempty_string(history_item, "encrypted_content").is_none() {
                    return false;
                }
                has_checkpoint = true;
            }
            None | Some("message") => {
                if !matches!(
                    item_object.get("role").and_then(Value::as_str),
                    Some("user" | "assistant" | "developer" | "system")
                ) || !super::replay::message_has_plaintext_content(item_object)
                    || super::replay::is_tool_state_item(history_item)
                    || super::replay::contains_encrypted_content(history_item)
                {
                    return false;
                }
            }
            Some("reasoning") => {
                // Retained reasoning is portable only with its actual payload,
                // not a provider-side item ID. It is never itself a checkpoint.
                if nonempty_string(history_item, "encrypted_content").is_none() {
                    return false;
                }
            }
            Some(kind @ ("function_call" | "custom_tool_call")) => {
                let Some(call_id) = nonempty_string(history_item, "call_id") else {
                    return false;
                };
                let argument_field_name = if kind == "function_call" {
                    "arguments"
                } else {
                    "input"
                };
                if nonempty_string(history_item, "name").is_none()
                    || !history_item
                        .get(argument_field_name)
                        .is_some_and(Value::is_string)
                    || !seen_calls.insert(call_id)
                {
                    return false;
                }
                pending_calls.insert(call_id, kind);
            }
            Some(kind @ ("function_call_output" | "custom_tool_call_output")) => {
                let Some(call_id) = nonempty_string(history_item, "call_id") else {
                    return false;
                };
                let expected = if kind == "function_call_output" {
                    "function_call"
                } else {
                    "custom_tool_call"
                };
                if pending_calls.remove(call_id) != Some(expected)
                    || !history_item.get("output").is_some_and(|output_value| {
                        output_value.is_string() || output_value.is_array()
                    })
                {
                    return false;
                }
            }
            _ => return false,
        }
    }
    has_checkpoint && pending_calls.is_empty()
}

fn nonempty_string<'a>(json_value: &'a Value, field: &str) -> Option<&'a str> {
    json_value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compacted_window_replaces_predecessor_without_pruning_or_merging() {
        for kind in ["compaction", "compaction_summary"] {
            let mut request = json!({
                "previous_response_id":"resp_before_compaction",
                "context_management":[{"type":"compaction","compact_threshold":1000}],
                "input":[
                    {"role":"user","content":"Retained user message"},
                    {"id":"cmp_test","type":kind,"encrypted_content":"synthetic-summary"},
                    {"type":"reasoning","encrypted_content":"synthetic-reasoning","summary":[]},
                    {"type":"function_call","call_id":"call_test","name":"lookup","arguments":"{}"},
                    {"type":"function_call_output","call_id":"call_test","output":"synthetic result"},
                    {"role":"user","content":"Continue"}
                ]
            });
            let mut expected = request.clone();
            expected
                .as_object_mut()
                .unwrap()
                .remove("previous_response_id");
            assert!(reset_compacted_history(&mut request));
            assert_eq!(request, expected);
        }
    }

    #[test]
    fn incomplete_compacted_history_never_loses_its_predecessor() {
        let compact = json!({"type":"compaction","encrypted_content":"synthetic-summary"});
        for input in [
            json!([{"type":"compaction","encrypted_content":""}]),
            json!([{"type":"compaction"}]),
            json!([compact, {"type":"item_reference","id":"msg_missing"}]),
            json!([compact, {"type":"function_call_output","call_id":"missing","output":"value"}]),
            json!([compact, {"type":"function_call","call_id":"pending","name":"lookup","arguments":"{}"}]),
            json!([compact,
                {"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{}"},
                {"type":"custom_tool_call_output","call_id":"call_1","output":"value"}]),
            json!([compact, {"type":"reasoning","id":"rs_missing","summary":[]}]),
            json!([{"type":"reasoning","encrypted_content":"synthetic-reasoning","summary":[]}]),
        ] {
            let mut request = json!({"previous_response_id":"resp_external","input":input});
            let original = request.clone();
            assert!(!reset_compacted_history(&mut request));
            assert_eq!(request, original);
        }
    }

    #[test]
    fn provider_conversation_and_compaction_settings_are_not_checkpoints() {
        for mut request in [
            json!({"conversation":"conv_1","input":[{"type":"compaction","encrypted_content":"synthetic"}]}),
            json!({"context_management":[{"type":"compaction"}],"input":[{"role":"user","content":"Continue"}]}),
            json!({"input":[{"role":"assistant","content":"Partial text"}]}),
        ] {
            request["previous_response_id"] = json!("resp_external");
            let original = request.clone();
            assert!(!reset_compacted_history(&mut request));
            assert_eq!(request, original);
        }
    }
}
