use super::catalog::{client_tools, requires_tool_call, selected_tools, tool_spec};
use super::codec::{json_text, parse_json_object};
use super::history::fit_item_id;
use super::is_transport_tool;
use crate::protocol::AdapterError;
use serde_json::{Map, Value};

pub(super) fn invalid_tool_output(parameter: &'static str) -> AdapterError {
    AdapterError::upstream_response_invalid().with_parameter(parameter)
}

pub(super) fn parse_transport_envelope(
    tool_call_item: &Map<String, Value>,
) -> Result<Map<String, Value>, AdapterError> {
    let tool_name = tool_call_item
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_tool_output("output.run_officejs.name"))?;
    if !is_transport_tool(tool_name) {
        return Err(invalid_tool_output("output.run_officejs.name"));
    }
    let outer = parse_json_object(tool_call_item.get("arguments"))
        .and_then(|arguments_value| arguments_value.as_object().cloned())
        .ok_or_else(|| invalid_tool_output("output.run_officejs.arguments"))?;
    let code = outer
        .get("code")
        .ok_or_else(|| invalid_tool_output("output.run_officejs.code"))?;

    // v0.1.14 and newer identify the client tool in the outer envelope. The
    // code field is the exact function JSON text or custom raw input; it must
    // not be parsed as another wrapper.
    if let Some(references) = outer.get("references") {
        let references = references
            .as_array()
            .filter(|reference_items| reference_items.len() == 1)
            .ok_or_else(|| invalid_tool_output("output.run_officejs.references"))?;
        let tool = references[0]
            .as_str()
            .filter(|tool_name| !tool_name.trim().is_empty())
            .filter(|tool_name| !is_transport_tool(tool_name))
            .ok_or_else(|| invalid_tool_output("output.run_officejs.references"))?;
        let code = code
            .as_str()
            .ok_or_else(|| invalid_tool_output("output.run_officejs.code"))?;
        return Ok(Map::from_iter([
            ("tool".to_string(), Value::String(tool.to_string())),
            ("args".to_string(), Value::String(code.to_string())),
        ]));
    }

    // Accept the legacy nested envelope for already materialized history, but
    // never generate it for new calls.
    let mut inner = parse_json_object(Some(code))
        .and_then(|nested_code_value| nested_code_value.as_object().cloned())
        .ok_or_else(|| invalid_tool_output("output.run_officejs.code"))?;
    for _ in 0..2 {
        let inner_name = inner.get("name").and_then(Value::as_str);
        if !inner_name.is_some_and(is_transport_tool) {
            break;
        }
        let nested_code = inner
            .get("code")
            .ok_or_else(|| invalid_tool_output("output.run_officejs.code"))?;
        inner = parse_json_object(Some(nested_code))
            .and_then(|nested_code_value| nested_code_value.as_object().cloned())
            .ok_or_else(|| invalid_tool_output("output.run_officejs.code"))?;
    }
    Ok(inner)
}

/// Convert a completed Basis Points response back to the client Responses
/// tool-call shape. The returned body is still ordinary Responses JSON.
pub(in crate::gateway::execution) fn translate_response(
    upstream_response_body: &[u8],
    request_body: &Value,
) -> Result<Vec<u8>, AdapterError> {
    let mut upstream_response: Value = serde_json::from_slice(upstream_response_body)
        .map_err(|_| AdapterError::upstream_response_invalid())?;
    if !matches!(
        upstream_response.get("status").and_then(Value::as_str),
        Some("completed" | "incomplete")
    ) {
        return Err(AdapterError::upstream_response_invalid().with_parameter("response.status"));
    }
    let Some(output_items) = upstream_response
        .get_mut("output")
        .and_then(Value::as_array_mut)
    else {
        return Err(AdapterError::upstream_response_invalid().with_parameter("response.output"));
    };
    let all_tools = client_tools(request_body)?;
    let tools = selected_tools(request_body, &all_tools);
    let mut call_ids = std::collections::HashSet::new();
    for output_item in output_items.iter_mut() {
        let Some(output_object) = output_item.as_object_mut() else {
            continue;
        };
        let kind = output_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(kind, "function_call" | "custom_tool_call") {
            continue;
        }
        let inner = parse_transport_envelope(output_object)?;
        let tool_name = inner
            .get("tool")
            .or_else(|| inner.get("name"))
            .and_then(Value::as_str)
            .filter(|tool_name| !tool_name.is_empty())
            .ok_or_else(|| invalid_tool_output("output.run_officejs.tool"))?;
        let tool = if let Some(tool) = tool_spec(&tools, tool_name) {
            tool
        } else if tool_spec(&all_tools, tool_name).is_some() {
            return Err(invalid_tool_output("output.run_officejs.tool_choice"));
        } else {
            return Err(invalid_tool_output("output.run_officejs.tool"));
        };
        let call_id = output_object
            .get("call_id")
            .and_then(Value::as_str)
            .filter(|call_id| !call_id.is_empty())
            .ok_or_else(|| invalid_tool_output("output.run_officejs.call_id"))?
            .to_string();
        if !call_ids.insert(call_id.clone()) {
            return Err(invalid_tool_output("output.run_officejs.call_id"));
        }
        let item_id = fit_item_id(&if tool.kind == "custom" {
            format!("ctc_{call_id}")
        } else {
            format!("fc_{call_id}")
        });
        let tool_arguments = inner.get("args").or_else(|| inner.get("arguments"));
        let mut translated = Map::new();
        translated.insert(
            "type".to_string(),
            Value::String(if tool.kind == "custom" {
                "custom_tool_call".to_string()
            } else {
                "function_call".to_string()
            }),
        );
        translated.insert("id".to_string(), Value::String(item_id));
        translated.insert("call_id".to_string(), Value::String(call_id));
        translated.insert("name".to_string(), Value::String(tool.name.clone()));
        if let Some(namespace) = &tool.namespace {
            translated.insert("namespace".to_string(), Value::String(namespace.clone()));
        }
        if tool.kind == "custom" {
            let custom_tool_input = tool_arguments
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_tool_output("output.run_officejs.args"))?;
            translated.insert(
                "input".to_string(),
                Value::String(custom_tool_input.to_string()),
            );
        } else {
            let function_arguments = tool_arguments
                .and_then(|argument_value| {
                    if argument_value.is_object() {
                        Some(argument_value.clone())
                    } else {
                        parse_json_object(Some(argument_value))
                    }
                })
                .filter(|argument_value| argument_value.is_object())
                .ok_or_else(|| invalid_tool_output("output.run_officejs.args"))?;
            if super::schema::parameter_schema(tool)
                .is_some_and(|schema| !super::schema::matches(&function_arguments, schema))
            {
                return Err(invalid_tool_output("output.run_officejs.args"));
            }
            translated.insert(
                "arguments".to_string(),
                Value::String(json_text(&function_arguments)?),
            );
            translated.insert("status".to_string(), Value::String("completed".to_string()));
        }
        *output_item = Value::Object(translated);
    }
    if requires_tool_call(request_body.get("tool_choice")) && call_ids.is_empty() {
        return Err(invalid_tool_output("output.tool_call"));
    }
    if request_body.get("parallel_tool_calls") == Some(&Value::Bool(false)) && call_ids.len() > 1 {
        // The strict upstream body cannot enforce this Responses option.
        // Reject the completed result without regenerating it.
        return Err(invalid_tool_output(
            "output.run_officejs.parallel_tool_calls",
        ));
    }
    serde_json::to_vec(&upstream_response).map_err(|_| AdapterError::upstream_response_invalid())
}

pub(super) fn has_encrypted_agent_message(request_input: Option<&Value>) -> bool {
    let is_encrypted_agent_message = |output_item: &Value| {
        output_item.get("type").and_then(Value::as_str) == Some("agent_message")
            && (output_item
                .get("encrypted_content")
                .is_some_and(|content| !content.is_null())
                || output_item
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|content| {
                        content.iter().any(|part| {
                            part.get("type").and_then(Value::as_str) == Some("encrypted_content")
                        })
                    }))
    };
    match request_input {
        Some(Value::Array(output_items)) => output_items.iter().any(is_encrypted_agent_message),
        Some(Value::Object(_)) => request_input.is_some_and(is_encrypted_agent_message),
        _ => false,
    }
}

mod stream;

pub(in crate::gateway::execution) use stream::synthetic_stream;
