//! The Excel/Basis Points account transport.
//!
//! Basis Points exposes a Responses-shaped endpoint, but its client-tool
//! contract is different: client tools are invoked through one native
//! `run_officejs` function. Keep that protocol detail inside this executor so
//! the rest of the gateway can continue to operate on ordinary Responses
//! requests and responses.

use crate::protocol::AdapterError;
use serde_json::{json, Map, Value};

const TRANSPORT_TOOL: &str = "run_officejs";
const TRANSPORT_TOOL_ALIAS: &str = "functions.run_officejs";

#[derive(Clone, Debug)]
struct ClientTool {
    name: String,
    namespace: Option<String>,
    kind: String,
    spec: Map<String, Value>,
}

impl ClientTool {
    fn key(&self) -> String {
        self.namespace
            .as_ref()
            .map(|namespace| format!("{namespace}.{}", self.name))
            .unwrap_or_else(|| self.name.clone())
    }

    fn call_name(&self) -> String {
        self.key()
    }
}

fn collect_tools(value: Option<&Value>, namespace: Option<&str>, output: &mut Vec<ClientTool>) {
    let Some(Value::Array(tools)) = value else {
        return;
    };
    for value in tools {
        let Some(object) = value.as_object() else {
            continue;
        };
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("function")
            .trim()
            .to_ascii_lowercase();
        let Some(name) = object.get("name").and_then(Value::as_str) else {
            if kind == "namespace" {
                continue;
            }
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if kind == "namespace" {
            collect_tools(object.get("tools"), Some(name), output);
        } else if matches!(kind.as_str(), "function" | "custom") {
            let tool = ClientTool {
                name: name.to_string(),
                namespace: namespace.map(str::to_string),
                kind,
                spec: object.clone(),
            };
            // A later additional_tools item replaces the earlier definition
            // for this qualified name, including its kind and schema.
            output.retain(|previous| previous.key() != tool.key());
            output.push(tool);
        }
    }
}

fn client_tools(request: &Value) -> Vec<ClientTool> {
    let mut result = Vec::new();
    collect_tools(request.get("tools"), None, &mut result);
    if let Some(items) = request.get("input").and_then(Value::as_array) {
        for item in items {
            if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                collect_tools(item.get("tools"), None, &mut result);
            }
        }
    }
    result
}

fn selected_tools(request: &Value, tools: &[ClientTool]) -> Vec<ClientTool> {
    let Some(choice) = request.get("tool_choice") else {
        return tools.to_vec();
    };
    if choice.as_str() == Some("none") {
        return Vec::new();
    }
    let Some(object) = choice.as_object() else {
        return tools.to_vec();
    };
    if object.get("type").and_then(Value::as_str) == Some("allowed_tools") {
        let Some(allowed) = object.get("tools").and_then(Value::as_array) else {
            return Vec::new();
        };
        return tools
            .iter()
            .filter(|tool| {
                allowed
                    .iter()
                    .any(|allowed| choice_matches_tool(allowed, tool))
            })
            .cloned()
            .collect();
    }
    if object.get("type").and_then(Value::as_str) == Some("auto") {
        return tools.to_vec();
    }
    tools
        .iter()
        .filter(|tool| choice_matches_tool(choice, tool))
        .cloned()
        .collect()
}

fn choice_matches_tool(choice: &Value, tool: &ClientTool) -> bool {
    let Some(name) = choice.get("name").and_then(Value::as_str) else {
        return false;
    };
    let name_matches = match choice.get("namespace").and_then(Value::as_str) {
        Some(namespace) => tool.namespace.as_deref() == Some(namespace) && tool.name == name,
        None => name == tool.name || name == tool.key(),
    };
    name_matches
        && choice
            .get("type")
            .and_then(Value::as_str)
            .is_none_or(|kind| kind == tool.kind)
}

fn requires_tool_call(choice: Option<&Value>) -> bool {
    choice.is_some_and(|choice| {
        choice.as_str() == Some("required")
            || choice.as_object().is_some_and(|choice| {
                matches!(
                    choice.get("type").and_then(Value::as_str),
                    Some("function" | "custom")
                ) || (choice.get("type").and_then(Value::as_str) == Some("allowed_tools")
                    && choice.get("mode").and_then(Value::as_str) == Some("required"))
            })
    })
}

fn tool_spec<'a>(tools: &'a [ClientTool], name: &str) -> Option<&'a ClientTool> {
    // A qualified namespace is authoritative. A bare name is only safe when
    // it identifies one tool; otherwise it could dispatch to the wrong client.
    if let Some(tool) = tools.iter().find(|tool| tool.key() == name) {
        return Some(tool);
    }
    let mut matches = tools.iter().filter(|tool| tool.name == name);
    let tool = matches.next()?;
    matches.next().is_none().then_some(tool)
}

fn as_input_items(input: Option<&Value>) -> Vec<Value> {
    match input {
        Some(Value::String(text)) => vec![json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": text}],
        })],
        Some(Value::Object(item)) => vec![Value::Object(item.clone())],
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

fn text_message(role: &str, content_type: &str, text: String) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{"type": content_type, "text": text}],
    })
}

fn json_text(value: &Value) -> Result<String, AdapterError> {
    serde_json::to_string(value).map_err(|_| AdapterError::invalid_request())
}

fn parse_json_object(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::Object(object)) => Some(Value::Object(object.clone())),
        Some(Value::String(text)) => serde_json::from_str(text).ok(),
        _ => None,
    }
}

fn parse_function_arguments(item: &Map<String, Value>) -> Result<Value, AdapterError> {
    let raw = item.get("arguments");
    let parsed = parse_json_object(raw).ok_or_else(AdapterError::invalid_request)?;
    if !parsed.is_object() {
        return Err(AdapterError::invalid_request().with_parameter("input.arguments"));
    }
    Ok(parsed)
}

fn inner_tool_call(item: &Map<String, Value>, tool: &ClientTool) -> Result<Value, AdapterError> {
    let name = tool.call_name();
    let args = if tool.kind == "custom" {
        item.get("input")
            .and_then(Value::as_str)
            .map(|input| Value::String(input.to_string()))
            .ok_or_else(|| AdapterError::invalid_request().with_parameter("input"))?
    } else {
        parse_function_arguments(item)?
    };
    Ok(json!({"tool": name, "args": args}))
}

fn transport_call(item: &Map<String, Value>, tool: &ClientTool) -> Result<Value, AdapterError> {
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| AdapterError::invalid_request().with_parameter("input.call_id"))?;
    let inner = inner_tool_call(item, tool)?;
    let code = json_text(&inner)?;
    let arguments = json!({
        "summary": format!("Run client tool {}", tool.call_name()),
        "extended_summary": "Relay client tool through the Excel / Basis Points transport",
        "destructive": false,
        "references": [],
        "code": code,
    });
    let id = if call_id.starts_with("fc_") {
        call_id.to_string()
    } else {
        format!("fc_{call_id}")
    };
    Ok(json!({
        "type": "function_call",
        "id": id,
        "call_id": call_id,
        "name": TRANSPORT_TOOL,
        "arguments": json_text(&arguments)?,
        "status": "completed",
    }))
}

fn translate_input_items(
    items: Vec<Value>,
    tools: &[ClientTool],
) -> Result<Vec<Value>, AdapterError> {
    let mut result = Vec::with_capacity(items.len());
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
                let name = object
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if name == TRANSPORT_TOOL || name == TRANSPORT_TOOL_ALIAS {
                    result.push(value);
                } else if let Some(tool) = tool_spec(tools, name) {
                    result.push(transport_call(object, tool)?);
                } else {
                    return Err(AdapterError::parameter_unsupported_for("input.tool_call"));
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
                let mut output = object.clone();
                output.insert(
                    "type".to_string(),
                    Value::String("function_call_output".to_string()),
                );
                output.insert(
                    "id".to_string(),
                    Value::String(if call_id.starts_with("fc_") {
                        call_id.to_string()
                    } else {
                        format!("fc_{call_id}")
                    }),
                );
                output.remove("name");
                output.remove("namespace");
                result.push(Value::Object(output));
            }
            "reasoning" => {
                if object
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .is_some_and(|content| !content.trim().is_empty())
                {
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

fn tool_instructions(tools: &[ClientTool], request: &Value) -> String {
    if tools.is_empty() {
        return "This request is relayed through Excel / Basis Points. Do not call server-injected Office or workbook tools. Return assistant text.".to_string();
    }
    let mut lines = Vec::with_capacity(tools.len());
    for tool in tools {
        let description = tool
            .spec
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let schema = tool
            .spec
            .get("parameters")
            .or_else(|| tool.spec.get("input_schema"));
        let schema_text = schema.and_then(|value| serde_json::to_string(value).ok());
        let mut line = format!("- {} ({})", tool.key(), tool.kind);
        if !description.is_empty() {
            line.push_str(": ");
            line.push_str(description);
        }
        if let Some(schema_text) = schema_text {
            line.push_str(" JSON Schema: ");
            line.push_str(&schema_text);
        }
        lines.push(line);
    }
    let choice = request
        .get("tool_choice")
        .and_then(|value| serde_json::to_string(value).ok())
        .map(|value| format!(" Client tool_choice: {value}."))
        .unwrap_or_default();
    format!(
        "This request is relayed through Excel / Basis Points. Use exactly one outer native {TRANSPORT_TOOL} call for each client tool invocation. Put a JSON object as JSON text in its code field with tool and args fields; do not put JavaScript or another {TRANSPORT_TOOL} envelope there. The available client tools are authoritative:{choice}\n{}",
        lines.join("\n")
    )
}

/// Prepare a client Responses request for the Basis Points executor.
pub(super) fn prepare_request(request: &Value) -> Result<Value, AdapterError> {
    let object = request
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    // This transport does not implement server-side continuation. Dropping an
    // opaque predecessor would silently turn a continuation into a new chat.
    if object
        .get("previous_response_id")
        .is_some_and(|value| !value.is_null())
    {
        return Err(AdapterError::parameter_unsupported_for(
            "previous_response_id",
        ));
    }
    let all_tools = client_tools(request);
    let callable = selected_tools(request, &all_tools);
    let tool_choice_requires_call = requires_tool_call(object.get("tool_choice"));
    if tool_choice_requires_call && callable.is_empty() {
        return Err(AdapterError::invalid_request().with_parameter("tool_choice"));
    }
    let mut output = object.clone();
    output.remove("previous_response_id");
    output.remove("service_tier");
    output.remove("store");
    if output
        .get("context_management")
        .is_some_and(|value| match value {
            Value::Null => true,
            Value::Array(items) => items.is_empty(),
            Value::Object(fields) => fields.is_empty(),
            _ => false,
        })
    {
        output.remove("context_management");
    }
    output.insert("store".to_string(), Value::Bool(false));
    output.insert("stream".to_string(), Value::Bool(false));

    let mut input = translate_input_items(as_input_items(object.get("input")), &all_tools)?;
    let mut prologue = Vec::new();
    if let Some(instructions) = object.get("instructions").and_then(Value::as_str) {
        if !instructions.trim().is_empty() {
            prologue.push(text_message(
                "developer",
                "input_text",
                instructions.to_string(),
            ));
        }
    }
    prologue.push(text_message(
        "developer",
        "input_text",
        tool_instructions(&callable, request),
    ));
    input.splice(0..0, prologue);
    output.insert("input".to_string(), Value::Array(input));
    output.remove("instructions");

    if callable.is_empty() || object.get("tool_choice").and_then(Value::as_str) == Some("none") {
        output.remove("tools");
        output.remove("tool_choice");
    } else {
        output.insert(
            "tools".to_string(),
            json!([{
                "type": "function",
                "name": TRANSPORT_TOOL,
                "description": "Relay one client tool invocation through the external client.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "summary": {"type": "string"},
                        "extended_summary": {"type": "string"},
                        "destructive": {"type": "boolean"},
                        "references": {"type": "array", "items": {"type": "string"}},
                        "code": {"type": "string"}
                    },
                    "required": ["summary", "extended_summary", "destructive", "references", "code"],
                    "additionalProperties": false
                }
            }]),
        );
        output.insert(
            "tool_choice".to_string(),
            if tool_choice_requires_call {
                json!({"type": "function", "name": TRANSPORT_TOOL})
            } else {
                Value::String("auto".to_string())
            },
        );
    }
    if let Some(reasoning) = object.get("reasoning").and_then(Value::as_object) {
        if let Some(effort) = reasoning.get("effort").and_then(Value::as_str) {
            output.insert(
                "reasoning_effort".to_string(),
                Value::String(effort.to_string()),
            );
        }
        output.remove("reasoning");
    }
    Ok(Value::Object(output))
}

fn invalid_tool_output(parameter: &'static str) -> AdapterError {
    AdapterError::upstream_response_invalid().with_parameter(parameter)
}

fn parse_transport_envelope(item: &Map<String, Value>) -> Result<Map<String, Value>, AdapterError> {
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_tool_output("output.run_officejs.name"))?;
    if name != TRANSPORT_TOOL && name != TRANSPORT_TOOL_ALIAS {
        return Err(invalid_tool_output("output.run_officejs.name"));
    }
    let outer = parse_json_object(item.get("arguments"))
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| invalid_tool_output("output.run_officejs.arguments"))?;
    let code = outer
        .get("code")
        .ok_or_else(|| invalid_tool_output("output.run_officejs.code"))?;
    let mut inner = parse_json_object(Some(code))
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| invalid_tool_output("output.run_officejs.code"))?;
    for _ in 0..2 {
        let inner_name = inner.get("name").and_then(Value::as_str);
        if inner_name != Some(TRANSPORT_TOOL) && inner_name != Some(TRANSPORT_TOOL_ALIAS) {
            break;
        }
        let nested_code = inner
            .get("code")
            .ok_or_else(|| invalid_tool_output("output.run_officejs.code"))?;
        inner = parse_json_object(Some(nested_code))
            .and_then(|value| value.as_object().cloned())
            .ok_or_else(|| invalid_tool_output("output.run_officejs.code"))?;
    }
    Ok(inner)
}

/// Convert a completed Basis Points response back to the client Responses
/// tool-call shape. The returned body is still ordinary Responses JSON.
pub(super) fn translate_response(body: &[u8], request: &Value) -> Result<Vec<u8>, AdapterError> {
    let mut response: Value =
        serde_json::from_slice(body).map_err(|_| AdapterError::upstream_response_invalid())?;
    if !matches!(
        response.get("status").and_then(Value::as_str),
        Some("completed" | "incomplete")
    ) {
        return Err(AdapterError::upstream_response_invalid().with_parameter("response.status"));
    }
    let Some(output) = response.get_mut("output").and_then(Value::as_array_mut) else {
        return Err(AdapterError::upstream_response_invalid().with_parameter("response.output"));
    };
    let all_tools = client_tools(request);
    let tools = selected_tools(request, &all_tools);
    let mut call_ids = std::collections::HashSet::new();
    for item in output.iter_mut() {
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(kind, "function_call" | "custom_tool_call") {
            continue;
        }
        let inner = parse_transport_envelope(object)?;
        let tool_name = inner
            .get("tool")
            .or_else(|| inner.get("name"))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| invalid_tool_output("output.run_officejs.tool"))?;
        let tool = tool_spec(&tools, tool_name)
            .ok_or_else(|| invalid_tool_output("output.run_officejs.tool"))?;
        let call_id = object
            .get("call_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| invalid_tool_output("output.run_officejs.call_id"))?
            .to_string();
        if !call_ids.insert(call_id.clone()) {
            return Err(invalid_tool_output("output.run_officejs.call_id"));
        }
        let item_id = if tool.kind == "custom" {
            format!("ctc_{call_id}")
        } else {
            format!("fc_{call_id}")
        };
        let args = inner.get("args").or_else(|| inner.get("arguments"));
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
            let input = args
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_tool_output("output.run_officejs.args"))?;
            translated.insert("input".to_string(), Value::String(input.to_string()));
        } else {
            let args = args
                .filter(|value| value.is_object())
                .ok_or_else(|| invalid_tool_output("output.run_officejs.args"))?;
            translated.insert("arguments".to_string(), Value::String(json_text(args)?));
            translated.insert("status".to_string(), Value::String("completed".to_string()));
        }
        *item = Value::Object(translated);
    }
    if requires_tool_call(request.get("tool_choice")) && call_ids.is_empty() {
        return Err(invalid_tool_output("output.tool_call"));
    }
    serde_json::to_vec(&response).map_err(|_| AdapterError::upstream_response_invalid())
}

/// Basis Points may return a completed JSON response even when a caller asked
/// for SSE. Emit the standard terminal Responses events so clients keep their
/// normal stream contract without exposing the internal transport.
pub(super) fn synthetic_stream(body: &[u8]) -> Result<Vec<u8>, AdapterError> {
    let response: Value =
        serde_json::from_slice(body).map_err(|_| AdapterError::upstream_stream_invalid())?;
    let terminal_event = match response.get("status").and_then(Value::as_str) {
        Some("completed") => "response.completed",
        Some("incomplete") => "response.incomplete",
        _ => return Err(AdapterError::upstream_stream_invalid()),
    };
    if response.get("output").and_then(Value::as_array).is_none() {
        return Err(AdapterError::upstream_stream_invalid());
    }
    let mut created = response.clone();
    if let Some(object) = created.as_object_mut() {
        object.insert(
            "status".to_string(),
            Value::String("in_progress".to_string()),
        );
        object.insert("output".to_string(), Value::Array(Vec::new()));
    }
    let mut sequence = 0_u64;
    let mut result = String::new();
    let emit =
        |result: &mut String, sequence: &mut u64, event: &str, mut payload: Map<String, Value>| {
            payload.insert("type".to_string(), Value::String(event.to_string()));
            payload.insert(
                "sequence_number".to_string(),
                Value::Number((*sequence).into()),
            );
            *sequence += 1;
            result.push_str("event: ");
            result.push_str(event);
            result.push_str("\ndata: ");
            result.push_str(
                &serde_json::to_string(&Value::Object(payload))
                    .unwrap_or_else(|_| "{}".to_string()),
            );
            result.push_str("\n\n");
        };
    emit(
        &mut result,
        &mut sequence,
        "response.created",
        Map::from_iter([(String::from("response"), created.clone())]),
    );
    emit(
        &mut result,
        &mut sequence,
        "response.in_progress",
        Map::from_iter([(String::from("response"), created)]),
    );
    if let Some(items) = response.get("output").and_then(Value::as_array) {
        for (index, item) in items.iter().enumerate() {
            let Some(object) = item.as_object() else {
                continue;
            };
            let output_index = Value::Number((index as u64).into());
            let item_id = object.get("id").cloned().unwrap_or(Value::Null);
            let item_type = object.get("type").and_then(Value::as_str);
            if item_type == Some("message") {
                let mut added = object.clone();
                added.insert(
                    "status".to_string(),
                    Value::String("in_progress".to_string()),
                );
                added.insert("content".to_string(), Value::Array(Vec::new()));
                emit(
                    &mut result,
                    &mut sequence,
                    "response.output_item.added",
                    Map::from_iter([
                        (String::from("output_index"), output_index.clone()),
                        (String::from("item"), Value::Object(added)),
                    ]),
                );
                if let Some(content) = object.get("content").and_then(Value::as_array) {
                    for (content_index, part) in content.iter().enumerate() {
                        let Some(part_object) = part.as_object() else {
                            continue;
                        };
                        if part_object.get("type").and_then(Value::as_str) != Some("output_text") {
                            continue;
                        }
                        let text = part_object
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let mut empty_part = part_object.clone();
                        empty_part.insert("text".to_string(), Value::String(String::new()));
                        emit(
                            &mut result,
                            &mut sequence,
                            "response.content_part.added",
                            Map::from_iter([
                                (String::from("output_index"), output_index.clone()),
                                (String::from("item_id"), item_id.clone()),
                                (
                                    String::from("content_index"),
                                    Value::Number((content_index as u64).into()),
                                ),
                                (String::from("part"), Value::Object(empty_part)),
                            ]),
                        );
                        if !text.is_empty() {
                            emit(
                                &mut result,
                                &mut sequence,
                                "response.output_text.delta",
                                Map::from_iter([
                                    (String::from("output_index"), output_index.clone()),
                                    (String::from("item_id"), item_id.clone()),
                                    (
                                        String::from("content_index"),
                                        Value::Number((content_index as u64).into()),
                                    ),
                                    (String::from("delta"), Value::String(text.to_string())),
                                ]),
                            );
                        }
                        emit(
                            &mut result,
                            &mut sequence,
                            "response.output_text.done",
                            Map::from_iter([
                                (String::from("output_index"), output_index.clone()),
                                (String::from("item_id"), item_id.clone()),
                                (
                                    String::from("content_index"),
                                    Value::Number((content_index as u64).into()),
                                ),
                                (String::from("text"), Value::String(text.to_string())),
                            ]),
                        );
                        emit(
                            &mut result,
                            &mut sequence,
                            "response.content_part.done",
                            Map::from_iter([
                                (String::from("output_index"), output_index.clone()),
                                (String::from("item_id"), item_id.clone()),
                                (
                                    String::from("content_index"),
                                    Value::Number((content_index as u64).into()),
                                ),
                                (String::from("part"), part.clone()),
                            ]),
                        );
                    }
                }
            } else {
                let mut added = object.clone();
                let field = match item_type {
                    Some("function_call") => Some("arguments"),
                    Some("custom_tool_call") => Some("input"),
                    _ => None,
                };
                if let Some(field) = field {
                    added.insert(field.to_string(), Value::String(String::new()));
                }
                emit(
                    &mut result,
                    &mut sequence,
                    "response.output_item.added",
                    Map::from_iter([
                        (String::from("output_index"), output_index.clone()),
                        (String::from("item"), Value::Object(added)),
                    ]),
                );
                if let Some(field) = field {
                    if let Some(text) = object.get(field).and_then(Value::as_str) {
                        if !text.is_empty() {
                            let event = if field == "arguments" {
                                "response.function_call_arguments.delta"
                            } else {
                                "response.custom_tool_call_input.delta"
                            };
                            emit(
                                &mut result,
                                &mut sequence,
                                event,
                                Map::from_iter([
                                    (String::from("output_index"), output_index.clone()),
                                    (String::from("item_id"), item_id.clone()),
                                    (String::from("delta"), Value::String(text.to_string())),
                                ]),
                            );
                        }
                        let event = if field == "arguments" {
                            "response.function_call_arguments.done"
                        } else {
                            "response.custom_tool_call_input.done"
                        };
                        emit(
                            &mut result,
                            &mut sequence,
                            event,
                            Map::from_iter([
                                (String::from("output_index"), output_index.clone()),
                                (String::from("item_id"), item_id.clone()),
                                (String::from(field), Value::String(text.to_string())),
                            ]),
                        );
                    }
                }
            }
            emit(
                &mut result,
                &mut sequence,
                "response.output_item.done",
                Map::from_iter([
                    (String::from("output_index"), output_index),
                    (String::from("item"), item.clone()),
                ]),
            );
        }
    }
    emit(
        &mut result,
        &mut sequence,
        terminal_event,
        Map::from_iter([(String::from("response"), response)]),
    );
    result.push_str("data: [DONE]\n\n");
    Ok(result.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with_tool() -> Value {
        json!({
            "model": "gpt-6-astra",
            "input": "Inspect the repo",
            "tools": [{"type":"function","name":"exec_command","description":"Run a command","parameters":{"type":"object","properties":{"cmd":{"type":"string"}},"required":["cmd"]}}]
        })
    }

    #[test]
    fn preparation_wraps_tools_and_omits_empty_context() {
        let mut request = request_with_tool();
        request["context_management"] = json!([]);
        let prepared = prepare_request(&request).unwrap();
        assert_eq!(prepared["stream"], false);
        assert!(prepared.get("context_management").is_none());
        assert_eq!(prepared["tools"][0]["name"], TRANSPORT_TOOL);
        assert!(prepared["input"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("exec_command"));
    }

    #[test]
    fn unsupported_opaque_continuation_is_not_silently_dropped() {
        let mut request = request_with_tool();
        request["previous_response_id"] = json!("resp_previous");
        let error = prepare_request(&request).unwrap_err();
        assert_eq!(error.parameter(), Some("previous_response_id"));
    }

    #[test]
    fn response_translates_outer_call_to_client_tool_call() {
        let request = request_with_tool();
        let body = json!({
            "id":"resp_1",
            "status":"completed",
            "output":[{"type":"function_call","id":"fc_outer","call_id":"call_1","name":TRANSPORT_TOOL,"arguments":serde_json::to_string(&json!({"code":serde_json::to_string(&json!({"tool":"exec_command","args":{"cmd":"pwd"}})).unwrap()})).unwrap()}]
        });
        let translated =
            translate_response(serde_json::to_string(&body).unwrap().as_bytes(), &request).unwrap();
        let translated: Value = serde_json::from_slice(&translated).unwrap();
        assert_eq!(translated["output"][0]["name"], "exec_command");
        assert_eq!(translated["output"][0]["call_id"], "call_1");
        assert_eq!(translated["output"][0]["arguments"], "{\"cmd\":\"pwd\"}");
    }

    fn tool_response(code: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": "resp_1",
            "status": "completed",
            "output": [{
                "type": "function_call",
                "call_id": "call_1",
                "name": TRANSPORT_TOOL,
                "arguments": json!({"code": code}).to_string()
            }]
        }))
        .unwrap()
    }

    #[test]
    fn malformed_tool_code_is_classified_without_exposing_its_contents() {
        let code = r#"{"tool":"exec_command","args":{"cmd":"echo "private""}}"#;
        let error = translate_response(&tool_response(code), &request_with_tool()).unwrap_err();
        assert_eq!(error.code(), "adapter_upstream_response_invalid");
        assert_eq!(error.parameter(), Some("output.run_officejs.code"));
        assert!(!error.message().contains("private"));
    }

    #[test]
    fn function_tool_arguments_must_be_an_object() {
        let code = r#"{"tool":"exec_command","args":"pwd"}"#;
        let error = translate_response(&tool_response(code), &request_with_tool()).unwrap_err();
        assert_eq!(error.parameter(), Some("output.run_officejs.args"));
    }

    #[test]
    fn ambiguous_namespace_tool_name_is_rejected() {
        let request = json!({
            "tools": [
                {"type":"namespace","name":"first","tools":[{"type":"function","name":"js"}]},
                {"type":"namespace","name":"second","tools":[{"type":"function","name":"js"}]}
            ]
        });
        let error =
            translate_response(&tool_response(r#"{"tool":"js","args":{}}"#), &request).unwrap_err();
        assert_eq!(error.parameter(), Some("output.run_officejs.tool"));

        let translated = translate_response(
            &tool_response(r#"{"tool":"second.js","args":{}}"#),
            &request,
        )
        .unwrap();
        let translated: Value = serde_json::from_slice(&translated).unwrap();
        assert_eq!(translated["output"][0]["name"], "js");
        assert_eq!(translated["output"][0]["namespace"], "second");
    }

    #[test]
    fn additional_tools_replace_earlier_definitions_and_keep_namespaces() {
        let request = json!({
            "model": "gpt-6-astra",
            "tools": [{"type":"namespace","name":"functions","tools":[
                {"type":"function","name":"exec","description":"OLD_DEFINITION"}
            ]}],
            "input": [
                {"type":"additional_tools","tools":[{"type":"namespace","name":"clock","tools":[
                    {"type":"function","name":"sleep"}
                ]}]},
                {"type":"additional_tools","tools":[{"type":"namespace","name":"functions","tools":[
                    {"type":"custom","name":"exec","description":"LATEST_DEFINITION","format":{"type":"text"}}
                ]}]},
                {"role":"user","content":[{"type":"input_text","text":"Run a command"}]}
            ],
            "tool_choice": {"type":"allowed_tools","mode":"required","tools":[
                {"type":"custom","namespace":"functions","name":"exec"}
            ]}
        });
        let prepared = prepare_request(&request).unwrap();
        let instructions = prepared["input"][0]["content"][0]["text"].as_str().unwrap();
        assert!(instructions.contains("functions.exec (custom): LATEST_DEFINITION"));
        assert!(!instructions.contains("OLD_DEFINITION"));
        assert!(!instructions.contains("clock.sleep"));
        assert!(prepared["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| { item.get("type").and_then(Value::as_str) != Some("additional_tools") }));

        let response = tool_response(r#"{"tool":"functions.exec","args":"pwd"}"#);
        let translated = translate_response(&response, &request).unwrap();
        let translated: Value = serde_json::from_slice(&translated).unwrap();
        assert_eq!(translated["output"][0]["type"], "custom_tool_call");
        assert_eq!(translated["output"][0]["namespace"], "functions");
        assert_eq!(translated["output"][0]["input"], "pwd");
    }

    #[test]
    fn disallowed_tool_calls_are_rejected_even_if_the_tool_is_in_history() {
        let mut request = request_with_tool();
        request["tool_choice"] = json!("none");
        let response = tool_response(r#"{"tool":"exec_command","args":{"cmd":"pwd"}}"#);
        let error = translate_response(&response, &request).unwrap_err();
        assert_eq!(error.parameter(), Some("output.run_officejs.tool"));

        request["tool_choice"] = json!({"type":"allowed_tools","tools":[]});
        let error = translate_response(&response, &request).unwrap_err();
        assert_eq!(error.parameter(), Some("output.run_officejs.tool"));

        request["tool_choice"] = json!({"type":"auto"});
        assert!(translate_response(&response, &request).is_ok());
    }

    #[test]
    fn custom_tool_and_output_keep_the_client_call_contract() {
        let request = json!({
            "model": "gpt-6-astra",
            "input": [
                {"type":"custom_tool_call","call_id":"call_1","name":"apply_patch","input":"diff --git a/a b/a"},
                {"type":"custom_tool_call_output","call_id":"call_1","output":"ok"}
            ],
            "tools": [{"type":"custom","name":"apply_patch","description":"Apply a patch","format":{"type":"text"}}]
        });
        let prepared = prepare_request(&request).unwrap();
        assert_eq!(prepared["input"][1]["name"], TRANSPORT_TOOL);
        assert_eq!(prepared["input"][2]["type"], "function_call_output");
        assert_eq!(prepared["input"][2]["id"], "fc_call_1");

        let body = json!({
            "id":"resp_1",
            "status":"completed",
            "output":[{"type":"function_call","id":"fc_outer","call_id":"call_2","name":TRANSPORT_TOOL,"arguments":serde_json::to_string(&json!({"code":serde_json::to_string(&json!({"tool":"apply_patch","args":"diff --git a/a b/a"})).unwrap()})).unwrap()}]
        });
        let translated =
            translate_response(serde_json::to_string(&body).unwrap().as_bytes(), &request).unwrap();
        let translated: Value = serde_json::from_slice(&translated).unwrap();
        assert_eq!(translated["output"][0]["type"], "custom_tool_call");
        assert_eq!(translated["output"][0]["input"], "diff --git a/a b/a");
    }

    #[test]
    fn required_tool_choice_without_a_tool_is_rejected_before_dispatch() {
        let request = json!({
            "model": "gpt-6-astra",
            "input": "hello",
            "tool_choice": "required"
        });
        let error = prepare_request(&request).unwrap_err();
        assert_eq!(error.parameter(), Some("tool_choice"));
    }

    #[test]
    fn required_allowed_tools_rejects_a_response_without_a_call() {
        let mut request = request_with_tool();
        request["tool_choice"] = json!({
            "type": "allowed_tools",
            "mode": "required",
            "tools": [{"type": "function", "name": "exec_command"}]
        });
        let prepared = prepare_request(&request).unwrap();
        assert_eq!(prepared["tool_choice"]["name"], TRANSPORT_TOOL);
        let response = json!({"id": "resp_1", "status": "completed", "output": []});
        assert!(translate_response(
            serde_json::to_string(&response).unwrap().as_bytes(),
            &request
        )
        .is_err());
    }

    #[test]
    fn synthetic_stream_has_terminal_responses_events() {
        let body = json!({
            "id":"resp_1",
            "status":"completed",
            "output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"hello","annotations":[]}]}]
        });
        let stream = synthetic_stream(serde_json::to_string(&body).unwrap().as_bytes()).unwrap();
        let stream = String::from_utf8(stream).unwrap();
        assert!(stream.contains("response.created"));
        assert!(stream.contains("response.output_text.delta"));
        assert!(stream.contains("response.output_text.done"));
        assert!(stream.contains("response.completed"));
        assert!(stream.ends_with("data: [DONE]\n\n"));
    }

    #[test]
    fn incomplete_response_is_not_reported_as_completed() {
        let request = json!({"model": "gpt-6-astra", "input": "hello"});
        let incomplete = json!({
            "id": "resp_1", "status": "incomplete", "output": [],
            "incomplete_details": {"reason": "max_output_tokens"}
        });
        let body = translate_response(&serde_json::to_vec(&incomplete).unwrap(), &request).unwrap();
        let stream = String::from_utf8(synthetic_stream(&body).unwrap()).unwrap();
        assert!(stream.contains("event: response.incomplete\n"));
        assert!(!stream.contains("event: response.completed\n"));
        assert!(stream.contains("max_output_tokens"));

        for response in [
            json!({"status": "completed"}),
            json!({"status": "failed", "output": []}),
            json!({"status": "cancelled", "output": []}),
            json!({"output": []}),
        ] {
            assert!(translate_response(&serde_json::to_vec(&response).unwrap(), &request).is_err());
        }
    }
}
