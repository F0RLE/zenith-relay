use super::catalog::{client_tool_call_name, history_tool, tool_spec, ClientTool};
use super::codec::{json_text, parse_function_arguments};
use super::{is_transport_tool, TRANSPORT_TOOL};
use crate::protocol::AdapterError;
use serde_json::{json, Map, Value};

fn function_transport_id(call_id: &str) -> String {
    if call_id.starts_with("fc_") {
        call_id.to_string()
    } else {
        format!("fc_{call_id}")
    }
}

pub(super) fn transport_call(
    item: &Map<String, Value>,
    tool: &ClientTool,
) -> Result<Value, AdapterError> {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| AdapterError::invalid_request().with_parameter("input.call_id"))?;
    // Basis Points v0.1.14 routes the client tool through the outer
    // `references` field. `code` is the payload itself: JSON text for a
    // function tool and unchanged text for a custom tool. Keeping the
    // payload unwrapped is required for quotes, backslashes and patches.
    let code = if tool.kind == "custom" {
        item.get("input")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| AdapterError::invalid_request().with_parameter("input"))?
    } else {
        json_text(&parse_function_arguments(item)?)?
    };
    let arguments = json!({
        "summary": format!("Run client tool {}", tool.call_name()),
        "extended_summary": "Relay client tool through the Excel / Basis Points transport",
        "destructive": false,
        "references": [tool.call_name()],
        "code": code,
    });
    let id = function_transport_id(call_id);
    Ok(json!({
        "type": "function_call",
        "id": id,
        "call_id": call_id,
        "name": TRANSPORT_TOOL,
        "arguments": json_text(&arguments)?,
        "status": "completed",
    }))
}

pub(super) fn record_client_call(
    object: &Map<String, Value>,
    tool: &ClientTool,
    result: &mut Vec<Value>,
    transport_call_ids: &mut std::collections::HashSet<String>,
) -> Result<(), AdapterError> {
    let call = transport_call(object, tool)?;
    if let Some(call_id) = object.get("call_id").and_then(Value::as_str) {
        transport_call_ids.insert(call_id.to_string());
    }
    result.push(call);
    Ok(())
}

pub(super) fn translate_input_items(
    items: Vec<Value>,
    tools: &[ClientTool],
) -> Result<Vec<Value>, AdapterError> {
    let mut result = Vec::with_capacity(items.len());
    let mut transport_call_ids = std::collections::HashSet::new();
    for value in items {
        let Some(object) = value.as_object() else {
            result.push(value);
            continue;
        };
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "function_call" | "custom_tool_call" => {
                let name = client_tool_call_name(object);
                if is_transport_tool(&name) {
                    if let Some(call_id) = object.get("call_id").and_then(Value::as_str) {
                        transport_call_ids.insert(call_id.to_string());
                    }
                    result.push(value);
                } else if let Some(tool) = tool_spec(tools, &name) {
                    record_client_call(object, tool, &mut result, &mut transport_call_ids)?;
                } else if let Some(tool) = history_tool(object) {
                    record_client_call(object, &tool, &mut result, &mut transport_call_ids)?;
                } else {
                    return Err(AdapterError::invalid_request().with_parameter("input.name"));
                }
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = object
                    .get("call_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                    .ok_or_else(|| {
                        AdapterError::invalid_request().with_parameter("input.call_id")
                    })?;
                if transport_call_ids.contains(call_id) {
                    let mut output = object.clone();
                    output.insert(
                        "type".to_string(),
                        Value::String("function_call_output".to_string()),
                    );
                    output.insert(
                        "id".to_string(),
                        Value::String(function_transport_id(call_id)),
                    );
                    output.remove("name");
                    output.remove("namespace");
                    result.push(Value::Object(output));
                } else {
                    // Keep outputs for calls that were not created by this
                    // transport. They may belong to a native account route.
                    result.push(value);
                }
            }
            "reasoning" => {
                // Same-account continuity needs the ciphertext on the first
                // attempt. A rejection drops the whole item: the ciphertext is
                // bound to that item and cannot be edited safely.
                if has_ciphertext(object.get("encrypted_content")) {
                    result.push(value);
                }
            }
            "additional_tools" => {}
            "item_reference" => {}
            _ => result.push(value),
        }
    }
    Ok(result)
}

/// Remove reasoning and compaction items that carry ciphertext another model
/// or account cannot decrypt. Visible messages, tool history and items without
/// ciphertext stay. Returns whether the request changed.
pub(in crate::gateway::execution) fn drop_foreign_encrypted_context(request: &mut Value) -> bool {
    let Some(items) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let before = items.len();
    items.retain(|item| !is_foreign_encrypted_context(item));
    items.len() != before
}

fn is_foreign_encrypted_context(item: &Value) -> bool {
    let Some(object) = item.as_object() else {
        return false;
    };
    if !has_ciphertext(object.get("encrypted_content")) {
        return false;
    }
    let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
    if matches!(kind, "reasoning" | "compaction" | "compaction_summary") {
        return true;
    }
    object
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| id.starts_with("rs_") || id.starts_with("cmp_"))
}

fn has_ciphertext(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(text)) => !text.trim().is_empty(),
        None | Some(Value::Null) => false,
        Some(_) => true,
    }
}
