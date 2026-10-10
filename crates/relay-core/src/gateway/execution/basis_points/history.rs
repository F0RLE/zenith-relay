use super::catalog::{client_tool_call_name, history_tool, tool_spec, ClientTool};
use super::codec::{json_text, parse_function_arguments};
use super::{is_transport_tool, TRANSPORT_TOOL};
use crate::gateway::request::responses_item_has_ciphertext as has_item_ciphertext;
use crate::protocol::AdapterError;
use serde_json::{json, Map, Value};

mod identifiers;

pub(in crate::gateway::execution::basis_points) use identifiers::fit_responses_id as fit_item_id;
use identifiers::fit_responses_id;

fn function_transport_id(call_id: &str) -> String {
    let normalized_call_id = if call_id.starts_with("fc_") {
        call_id.to_string()
    } else {
        format!("fc_{call_id}")
    };
    fit_responses_id(&normalized_call_id)
}

pub(super) fn transport_call(
    tool_call_object: &Map<String, Value>,
    tool: &ClientTool,
) -> Result<Value, AdapterError> {
    let call_id = tool_call_object
        .get("call_id")
        .and_then(Value::as_str)
        .filter(|call_id| !call_id.trim().is_empty())
        .ok_or_else(|| AdapterError::invalid_request().with_parameter("input.call_id"))?;
    // Basis Points v0.1.14 routes the client tool through the outer
    // `references` field. `code` is the payload itself: JSON text for a
    // function tool and unchanged text for a custom tool. Keeping the
    // payload unwrapped is required for quotes, backslashes and patches.
    let code = if tool.kind == "custom" {
        tool_call_object
            .get("input")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| AdapterError::invalid_request().with_parameter("input"))?
    } else {
        json_text(&parse_function_arguments(tool_call_object)?)?
    };
    let arguments = json!({
        "summary": format!("Run client tool {}", tool.call_name()),
        "extended_summary": "Relay client tool through the Excel / Basis Points transport",
        "destructive": false,
        "references": [tool.call_name()],
        "code": code,
    });
    let transport_id = function_transport_id(call_id);
    Ok(json!({
        "type": "function_call",
        "id": transport_id,
        "call_id": call_id,
        "name": TRANSPORT_TOOL,
        "arguments": json_text(&arguments)?,
        "status": "completed",
    }))
}

pub(super) fn record_client_call(
    tool_call_object: &Map<String, Value>,
    tool: &ClientTool,
    translated_items: &mut Vec<Value>,
    transport_call_ids: &mut std::collections::HashSet<String>,
) -> Result<(), AdapterError> {
    let transport_call = transport_call(tool_call_object, tool)?;
    if let Some(call_id) = tool_call_object.get("call_id").and_then(Value::as_str) {
        transport_call_ids.insert(call_id.to_string());
    }
    translated_items.push(transport_call);
    Ok(())
}

pub(super) fn translate_input_items(
    input_items: Vec<Value>,
    tools: &[ClientTool],
) -> Result<Vec<Value>, AdapterError> {
    let mut translated_items = Vec::with_capacity(input_items.len());
    let mut transport_call_ids = std::collections::HashSet::new();
    for input_item in input_items {
        let Some(input_object) = input_item.as_object() else {
            translated_items.push(input_item);
            continue;
        };
        let item_type = input_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match item_type {
            "function_call" | "custom_tool_call" => {
                let tool_name = client_tool_call_name(input_object);
                if is_transport_tool(&tool_name) {
                    if let Some(call_id) = input_object.get("call_id").and_then(Value::as_str) {
                        transport_call_ids.insert(call_id.to_string());
                    }
                    translated_items.push(input_item);
                } else if let Some(tool) = tool_spec(tools, &tool_name) {
                    record_client_call(
                        input_object,
                        tool,
                        &mut translated_items,
                        &mut transport_call_ids,
                    )?;
                } else if let Some(tool) = history_tool(input_object) {
                    record_client_call(
                        input_object,
                        &tool,
                        &mut translated_items,
                        &mut transport_call_ids,
                    )?;
                } else {
                    return Err(AdapterError::invalid_request().with_parameter("input.name"));
                }
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = input_object
                    .get("call_id")
                    .and_then(Value::as_str)
                    .filter(|call_id| !call_id.trim().is_empty())
                    .ok_or_else(|| {
                        AdapterError::invalid_request().with_parameter("input.call_id")
                    })?;
                if transport_call_ids.contains(call_id) {
                    let mut translated_output = input_object.clone();
                    translated_output.insert(
                        "type".to_string(),
                        Value::String("function_call_output".to_string()),
                    );
                    translated_output.insert(
                        "id".to_string(),
                        Value::String(function_transport_id(call_id)),
                    );
                    translated_output.remove("name");
                    translated_output.remove("namespace");
                    translated_items.push(Value::Object(translated_output));
                } else {
                    // Keep outputs for calls that were not created by this
                    // transport. They may belong to a native account route.
                    translated_items.push(input_item);
                }
            }
            "reasoning" => {
                // Preserve ciphertext on the first attempt for same-account
                // continuity. Account recovery removes a rejected blob and its
                // bound ID while retaining any visible summary.
                if has_item_ciphertext(input_object.get("encrypted_content")) {
                    translated_items.push(input_item);
                } else if let Some(visible_item) = visible_item_without_ciphertext(input_object) {
                    translated_items.push(visible_item);
                }
            }
            "additional_tools" | "tool_search_output" => {}
            "item_reference" => {}
            _ => translated_items.push(input_item),
        }
    }
    identifiers::limit_responses_identifiers(&mut translated_items);
    Ok(translated_items)
}

/// Drop a ciphertext blob and the id bound to it. Visible summary text stays.
fn visible_item_without_ciphertext(item_object: &Map<String, Value>) -> Option<Value> {
    let mut kept = item_object.clone();
    kept.remove("encrypted_content");
    kept.remove("id");
    if has_visible_text(&kept) {
        Some(Value::Object(kept))
    } else {
        None
    }
}

fn has_visible_text(item_object: &Map<String, Value>) -> bool {
    text_list_has_content(item_object.get("summary"))
        || text_list_has_content(item_object.get("content"))
}

fn text_list_has_content(content_value: Option<&Value>) -> bool {
    content_value
        .and_then(Value::as_array)
        .is_some_and(|content_parts| {
            content_parts.iter().any(|content_part| {
                content_part
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
            })
        })
}
