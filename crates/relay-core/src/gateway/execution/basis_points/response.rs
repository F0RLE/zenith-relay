use super::catalog::{client_tools, requires_tool_call, selected_tools, tool_spec, ClientTool};
use super::codec::{json_text, parse_json_object};
use super::history::fit_item_id;
use super::is_transport_tool;
use crate::protocol::AdapterError;
use serde_json::{json, Map, Value};

pub(super) fn invalid_tool_output(parameter: &'static str) -> AdapterError {
    let error = match parameter {
        "output.run_officejs.args.schema" => AdapterError::upstream_tool_arguments_invalid(),
        "output.run_officejs.args.frame" => AdapterError::upstream_tool_arguments_frame_invalid(),
        "output.run_officejs.args.json.eof" => {
            AdapterError::upstream_tool_arguments_json_incomplete()
        }
        "output.run_officejs.args.json.syntax" => {
            AdapterError::upstream_tool_arguments_json_syntax_invalid()
        }
        "output.run_officejs.args.json" | "output.run_officejs.args.object" => {
            AdapterError::upstream_tool_arguments_json_invalid()
        }
        _ => AdapterError::upstream_response_invalid(),
    };
    error.with_parameter(parameter)
}

fn validate_function_arguments(arguments: Value, tool: &ClientTool) -> Result<Value, AdapterError> {
    let Some(schema) = super::schema::parameter_schema(tool) else {
        return Ok(arguments);
    };
    let Err(original_error) = super::schema::validate(&arguments, schema) else {
        return Ok(arguments);
    };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Err(original_error);
    };
    let Some(envelope) = arguments.as_object().filter(|object| object.len() == 2) else {
        return Err(original_error);
    };
    let name_key = if envelope.contains_key("tool") {
        "tool"
    } else {
        "name"
    };
    let args_key = if envelope.contains_key("args") {
        "args"
    } else {
        "arguments"
    };
    // A stale transport envelope is not a client's argument object. Unwrap only
    // an exact, same-tool envelope that the closed client schema cannot describe.
    if schema.get("type").and_then(Value::as_str) != Some("object")
        || schema.get("additionalProperties") != Some(&Value::Bool(false))
        || schema.get("patternProperties").is_some()
        || properties.contains_key(name_key)
        || properties.contains_key(args_key)
        || envelope.get(name_key).and_then(Value::as_str) != Some(tool.key().as_str())
    {
        return Err(original_error);
    }
    let Some(nested) = envelope.get(args_key) else {
        return Err(original_error);
    };
    let recovered = super::codec::function_arguments(nested)?;
    if !recovered.is_object() {
        return Err(invalid_tool_output("output.run_officejs.args.object"));
    }
    super::schema::validate(&recovered, schema)?;
    Ok(recovered)
}

fn parse_direct_tool_call(code: &str) -> Option<Map<String, Value>> {
    // Some upstream responses contain a direct client-tool call instead of
    // the documented nested transport object. Parse only its strict
    // `name(JSON value)` shape; never evaluate the surrounding text.
    fn strip_keyword<'a>(code: &'a str, keyword: &str) -> Option<&'a str> {
        let rest = code.strip_prefix(keyword)?;
        rest.chars()
            .next()
            .is_some_and(|character| character.is_whitespace())
            .then(|| rest.trim_start())
    }

    let mut code = code.trim();
    if let Some(rest) = strip_keyword(code, "return") {
        code = rest;
        if let Some(rest) = strip_keyword(code, "await") {
            code = rest;
        }
    } else if let Some(rest) = strip_keyword(code, "await") {
        code = rest;
    }

    let open_parenthesis = code.find('(')?;
    let tool = code[..open_parenthesis].trim();
    if tool.is_empty()
        || is_transport_tool(tool)
        || !tool.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
        })
    {
        return None;
    }

    let call_tail = code[open_parenthesis + 1..].trim_end();
    let arguments_text = call_tail.strip_suffix(')')?.trim();
    if arguments_text.is_empty() {
        return None;
    }
    let arguments = serde_json::from_str::<Value>(arguments_text).ok()?;
    Some(Map::from_iter([
        ("tool".to_string(), Value::String(tool.to_string())),
        ("args".to_string(), arguments),
    ]))
}

pub(super) fn validate_tool_completion(item: &Value) -> Result<(), AdapterError> {
    // BPS may omit item status, but an explicit unfinished call is not executable.
    if item
        .get("status")
        .is_some_and(|status| status != "completed")
    {
        return Err(invalid_tool_output("output.run_officejs.status"));
    }
    Ok(())
}

fn invalid_tool_identity(reason: &'static str) -> AdapterError {
    invalid_tool_output("output.run_officejs.tool").with_tool_identity_diagnostic(reason)
}

fn transport_tool_name(envelope: &Map<String, Value>) -> Result<&str, AdapterError> {
    let name = envelope
        .get("tool")
        .or_else(|| envelope.get("name"))
        .ok_or_else(|| invalid_tool_identity("missing_name"))?
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| invalid_tool_identity("invalid_name"))?;
    if envelope.contains_key("tool")
        && envelope
            .get("name")
            .is_some_and(|other| other.as_str() != Some(name))
    {
        return Err(invalid_tool_identity("conflicting_names"));
    }
    Ok(name)
}

pub(super) fn parse_transport_envelope(
    tool_call_item: &Map<String, Value>,
) -> Result<(Map<String, Value>, bool), AdapterError> {
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

    // Explicit references route the client payload. Function-wrapper recovery
    // is considered later, only after original-schema validation fails.
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
        return Ok((
            Map::from_iter([
                ("tool".to_string(), Value::String(tool.to_string())),
                ("args".to_string(), Value::String(code.to_string())),
            ]),
            false,
        ));
    }

    // Accept the legacy nested envelope for already materialized history, but
    // never generate it for new calls.
    let (mut inner, direct_call) = match parse_json_object(Some(code))
        .and_then(|nested_code_value| nested_code_value.as_object().cloned())
    {
        Some(inner) => (inner, false),
        None => {
            let code = code
                .as_str()
                .ok_or_else(|| invalid_tool_output("output.run_officejs.code"))?;
            (
                parse_direct_tool_call(code)
                    .ok_or_else(|| invalid_tool_output("output.run_officejs.code"))?,
                true,
            )
        }
    };
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
    // Normalize only the already-supported legacy envelope field aliases.
    // Discovery and client translation must see the same identity and payload;
    // never infer a tool name or inspect referenced/custom payload contents here.
    let name = transport_tool_name(&inner)?.to_string();
    inner.insert("tool".to_string(), Value::String(name));
    if !inner.contains_key("args") {
        if let Some(arguments) = inner.get("arguments").cloned() {
            inner.insert("args".to_string(), arguments);
        }
    }
    Ok((inner, direct_call))
}

/// Convert a completed Basis Points response back to the client Responses
/// tool-call shape. The returned body is still ordinary Responses JSON.
pub(in crate::gateway) fn translate_response(
    upstream_response_body: &[u8],
    request_body: &Value,
) -> Result<Vec<u8>, AdapterError> {
    let upstream_response: Value = serde_json::from_slice(upstream_response_body)
        .map_err(|_| AdapterError::upstream_response_invalid())?;
    let translated = translate_response_value(upstream_response, request_body)?;
    serde_json::to_vec(&translated).map_err(|_| AdapterError::upstream_response_invalid())
}

/// Streaming already owns the parsed terminal response. Do not serialize and
/// parse its complete output a second time just to validate client tools.
pub(in crate::gateway) fn translate_response_value(
    mut upstream_response: Value,
    request_body: &Value,
) -> Result<Value, AdapterError> {
    if !matches!(
        upstream_response.get("status").and_then(Value::as_str),
        Some("completed" | "incomplete")
    ) {
        return Err(AdapterError::upstream_response_invalid().with_parameter("response.status"));
    }
    let completed = upstream_response["status"] == "completed";
    let Some(output_items) = upstream_response
        .get_mut("output")
        .and_then(Value::as_array_mut)
    else {
        return Err(AdapterError::upstream_response_invalid().with_parameter("response.output"));
    };
    // An incomplete JSON response has the same safety boundary as incomplete
    // SSE: retain text/reasoning and reported usage, but never execute tools.
    if !completed {
        output_items.retain(|item| {
            !matches!(
                item["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )
        });
    }
    let all_tools = client_tools(request_body)?;
    super::batch::expand_output(output_items, request_body, &all_tools)?;
    let tools = selected_tools(request_body, &all_tools);
    let mut call_ids = std::collections::HashSet::new();
    let history_call_ids = request_body
        .get("input")
        .into_iter()
        .flat_map(|input| {
            input
                .as_array()
                .map_or(std::slice::from_ref(input), Vec::as_slice)
        })
        .filter_map(|item| item.get("call_id").and_then(Value::as_str))
        .collect::<std::collections::HashSet<_>>();
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
        let (inner, direct_call) = parse_transport_envelope(output_object)?;
        let tool_name = transport_tool_name(&inner)?;
        let tool = if let Some(tool) = tool_spec(&tools, tool_name) {
            tool
        } else if tool_spec(&all_tools, tool_name).is_some() {
            return Err(invalid_tool_output("output.run_officejs.tool_choice"));
        } else {
            let reason = if all_tools
                .iter()
                .filter(|tool| tool.name == tool_name)
                .count()
                > 1
            {
                "ambiguous_name"
            } else {
                "unknown_tool"
            };
            return Err(invalid_tool_identity(reason));
        };
        let call_id = output_object
            .get("call_id")
            .and_then(Value::as_str)
            .filter(|call_id| !call_id.trim().is_empty())
            .ok_or_else(|| invalid_tool_output("output.run_officejs.call_id"))?
            .to_string();
        if history_call_ids.contains(call_id.as_str()) || !call_ids.insert(call_id.clone()) {
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
            let function_arguments = match tool_arguments {
                Some(Value::Object(arguments)) => Value::Object(arguments.clone()),
                Some(arguments @ Value::String(code)) if !direct_call => {
                    if let Some(mut call) = parse_direct_tool_call(code) {
                        if call.get("tool").and_then(Value::as_str) != Some(tool_name) {
                            return Err(invalid_tool_identity("reference_name_mismatch"));
                        }
                        call.remove("args")
                            .ok_or_else(|| invalid_tool_output("output.run_officejs.args.object"))?
                    } else {
                        super::codec::function_arguments(arguments)?
                    }
                }
                _ => return Err(invalid_tool_output("output.run_officejs.args.object")),
            };
            if !function_arguments.is_object() {
                return Err(invalid_tool_output("output.run_officejs.args.object"));
            }
            let function_arguments = validate_function_arguments(function_arguments, tool)?;
            translated.insert(
                "arguments".to_string(),
                Value::String(json_text(&function_arguments)?),
            );
            translated.insert("status".to_string(), Value::String("completed".to_string()));
        }
        *output_item = Value::Object(translated);
    }
    if completed && requires_tool_call(request_body.get("tool_choice")) && call_ids.is_empty() {
        return Err(invalid_tool_output("output.tool_call"));
    }
    if request_body.get("parallel_tool_calls") == Some(&Value::Bool(false)) && call_ids.len() > 1 {
        // The strict upstream body cannot enforce this Responses option.
        // Reject the completed result without regenerating it.
        return Err(invalid_tool_output(
            "output.run_officejs.parallel_tool_calls",
        ));
    }
    if let Some(structured) = super::structured_output::prepare(request_body)? {
        structured.validate(&upstream_response)?;
    }
    // Do not expose BPS task/turn bookkeeping as the client's metadata. The
    // strict upstream schema cannot carry that client map, so echo the exact
    // original map here before JSON or synthetic SSE is returned.
    upstream_response["metadata"] = request_body
        .get("metadata")
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok(upstream_response)
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

pub(in crate::gateway) mod stream;

pub(in crate::gateway) use stream::synthetic_stream;
