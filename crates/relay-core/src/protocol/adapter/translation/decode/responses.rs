use super::super::{
    checked, optional_bool, required_text, validate_direct_tool_execution, AdapterError,
    AdapterResult, Block, Message, Reasoning, Request, Role,
};
use crate::WireApi;
use serde_json::{json, Value};

pub(super) fn decode(request_body: &Value) -> AdapterResult<Request> {
    super::super::super::contracts::validate_responses_bridge_request(
        request_body,
        WireApi::Gemini,
    )?;
    let mut decoded_request = Request {
        metadata: super::super::options::metadata(request_body, WireApi::Responses)?,
        ..Default::default()
    };
    if let Some(instructions) = request_body
        .get("instructions")
        .filter(|instructions_value| !instructions_value.is_null())
    {
        decoded_request.instructions = Some(Message {
            role: Role::System,
            blocks: super::content::text_blocks(instructions, WireApi::Responses)?,
        });
    }
    let input_items = request_body
        .get("input")
        .ok_or_else(AdapterError::invalid_request)?;
    if let Some(text) = input_items.as_str() {
        decoded_request.messages.push(Message {
            role: Role::User,
            blocks: vec![Block::Text(text.into())],
        });
    } else {
        for response_item in input_items
            .as_array()
            .ok_or_else(AdapterError::invalid_request)?
        {
            match response_item
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("message")
            {
                "message" => {
                    checked_item(
                        response_item,
                        &["type", "role", "content", "id", "status", "phase"],
                    )?;
                    let role = super::content::role(response_item, WireApi::Responses)?;
                    let blocks = super::content::text_blocks(
                        response_item
                            .get("content")
                            .ok_or_else(AdapterError::invalid_request)?,
                        WireApi::Responses,
                    )?;
                    if role == Role::Assistant {
                        append_assistant_blocks(&mut decoded_request.messages, blocks);
                    } else {
                        decoded_request.messages.push(Message { role, blocks });
                    }
                }
                // Tools loaded during the turn are part of the catalog, which
                // `responses_tools` reads; they are not conversation content.
                "additional_tools" => {}
                "tool_search_call" | "tool_search_output"
                    if response_item.get("execution").and_then(Value::as_str) == Some("client") =>
                {
                    let block = super::super::tool_search::history_block(response_item)?;
                    if matches!(block, Block::ToolCall { .. }) {
                        append_assistant_blocks(&mut decoded_request.messages, vec![block]);
                    } else {
                        decoded_request.messages.push(Message {
                            role: Role::User,
                            blocks: vec![block],
                        });
                    }
                }
                "tool_search_call" | "tool_search_output" => {
                    // Client discovery is a call/result roundtrip, not hosted
                    // search metadata. Until it has a lossless representation,
                    // reject it before transport rather than erase the turn.
                    if response_item.get("execution").and_then(Value::as_str) == Some("client")
                        || response_item.get("call_id").is_some_and(|id| !id.is_null())
                    {
                        return Err(AdapterError::unsupported_tool().with_parameter("input.type"));
                    }
                }
                "function_call" => {
                    checked_item(
                        response_item,
                        &[
                            "type",
                            "id",
                            "status",
                            "call_id",
                            "name",
                            "namespace",
                            "arguments",
                            "async",
                            "caller",
                        ],
                    )?;
                    validate_call_execution(response_item)?;
                    append_assistant_blocks(
                        &mut decoded_request.messages,
                        vec![Block::ToolCall {
                            id: required_text(response_item, "call_id")?.into(),
                            name: upstream_call_name(response_item)?,
                            arguments: required_text(response_item, "arguments")?.into(),
                        }],
                    );
                }
                "custom_tool_call" => {
                    checked_item(
                        response_item,
                        &[
                            "type",
                            "id",
                            "status",
                            "call_id",
                            "name",
                            "namespace",
                            "input",
                            "async",
                            "caller",
                        ],
                    )?;
                    validate_call_execution(response_item)?;
                    let tool_input = response_item
                        .get("input")
                        .and_then(Value::as_str)
                        .ok_or_else(AdapterError::invalid_request)?;
                    append_assistant_blocks(
                        &mut decoded_request.messages,
                        vec![Block::ToolCall {
                            id: required_text(response_item, "call_id")?.into(),
                            name: upstream_call_name(response_item)?,
                            arguments: serde_json::to_string(&json!({"input": tool_input}))
                                .map_err(|_| AdapterError::invalid_request())?,
                        }],
                    );
                }
                "function_call_output" | "custom_tool_call_output" => {
                    checked_item(
                        response_item,
                        &[
                            "type",
                            "id",
                            "status",
                            "call_id",
                            "name",
                            "output",
                            "namespace",
                            "caller",
                        ],
                    )?;
                    validate_call_execution(response_item)?;
                    if response_item
                        .get("name")
                        .is_some_and(|name| !name.is_null())
                    {
                        if response_item.get("type").and_then(Value::as_str)
                            == Some("custom_tool_call_output")
                        {
                            return Err(AdapterError::parameter_unsupported_for("input.name"));
                        }
                        required_text(response_item, "name")
                            .map_err(|error| error.with_parameter("input.name"))?;
                    }
                    decoded_request.messages.push(Message {
                        role: Role::User,
                        blocks: vec![Block::ToolResult {
                            id: tool_result_id(response_item)?.into(),
                            name: String::new(),
                            content: tool_result_content(response_item)?,
                            is_error: false,
                            raw_text: response_item.get("type").and_then(Value::as_str)
                                == Some("custom_tool_call_output"),
                        }],
                    });
                }
                "reasoning" => {
                    // History preparation removes foreign ciphertext only
                    // from completed turns before a new user message.
                    // Bridge validation rejects any remaining opaque state.
                    checked_item(
                        response_item,
                        &["type", "id", "status", "summary", "encrypted_content"],
                    )?;
                    let blocks = response_item
                        .get("summary")
                        .and_then(Value::as_array)
                        .ok_or_else(AdapterError::invalid_request)?
                        .iter()
                        .map(|summary_item| {
                            checked(summary_item, &["type", "text"])?;
                            if required_text(summary_item, "type")? != "summary_text" {
                                return Err(AdapterError::parameter_unsupported());
                            }
                            Ok(Block::Reasoning(
                                summary_item
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .ok_or_else(AdapterError::invalid_request)?
                                    .into(),
                            ))
                        })
                        .collect::<AdapterResult<Vec<_>>>()?;
                    append_assistant_blocks(&mut decoded_request.messages, blocks);
                }
                _ => return Err(AdapterError::parameter_unsupported_for("input.type")),
            }
        }
    }
    (decoded_request.tools, decoded_request.client_tools) =
        super::content::responses_tools(request_body)?;
    let (choice, allowed) = super::super::super::contracts::resolve_responses_tool_choice(
        request_body.get("tool_choice").unwrap_or(&Value::Null),
        &decoded_request.client_tools,
    )?;
    if let Some(allowed) = allowed {
        decoded_request
            .tools
            .retain(|tool| allowed.contains(&tool.name));
    }
    decoded_request.tool_choice = choice;
    super::super::tool_search::filter_deferred(&mut decoded_request, request_body)?;
    decoded_request.parallel_tools = optional_bool(request_body, "parallel_tool_calls")?;
    super::content::common(
        &mut decoded_request,
        request_body,
        "max_output_tokens",
        "top_p",
        "stop",
    )?;
    super::content::sampling_extensions(&mut decoded_request, request_body, false)?;
    if let Some(text_config) = request_body
        .get("text")
        .filter(|text_config_value| !text_config_value.is_null())
    {
        checked(text_config, &["format"])?;
        decoded_request.output_format =
            super::content::output_format(text_config.get("format").unwrap_or(&Value::Null))?;
    }
    if let Some(format) = request_body
        .get("response_format")
        .filter(|value| !value.is_null())
    {
        if request_body
            .pointer("/text/format")
            .is_some_and(|value| !value.is_null())
        {
            return Err(AdapterError::parameter_unsupported_for("response_format"));
        }
        decoded_request.output_format = super::content::output_format(format)?;
    }
    if let Some(reasoning) = request_body
        .get("reasoning")
        .filter(|reasoning_value| !reasoning_value.is_null())
    {
        if let Some(effort) = reasoning
            .get("effort")
            .filter(|field_value| !field_value.is_null())
        {
            decoded_request.reasoning = Some(Reasoning::Effort(
                effort
                    .as_str()
                    .ok_or_else(AdapterError::invalid_request)?
                    .into(),
            ));
        }
    }
    Ok(decoded_request)
}

/// Turn IDs and creation timestamps belong to the receiving harness's trace,
/// not to model input or signed provider state. Validate this narrow metadata
/// contract on both history lookup and translation; do not add a blanket
/// exception to the shared protocol field guard.
fn checked_item(item: &Value, allowed: &[&str]) -> AdapterResult<()> {
    const METADATA: &str = "internal_chat_message_metadata_passthrough";
    const PARAMETER: &str = "input.internal_chat_message_metadata_passthrough";
    let fields = item.as_object().ok_or_else(AdapterError::invalid_request)?;
    if fields.iter().any(|(name, value)| {
        !allowed.contains(&name.as_str()) && name != METADATA && !value.is_null()
    }) {
        return Err(AdapterError::parameter_unsupported_for("input"));
    }
    let Some(metadata) = fields.get(METADATA).filter(|value| !value.is_null()) else {
        return Ok(());
    };
    let metadata = metadata
        .as_object()
        .ok_or_else(|| AdapterError::invalid_request().with_parameter(PARAMETER))?;
    if metadata.iter().any(|(name, value)| {
        !matches!(name.as_str(), "turn_id" | "create_time") && !value.is_null()
    }) {
        // Content classifications, attribution and execution records are not
        // tracing IDs. Preserve the guard until their semantics are supported.
        return Err(AdapterError::parameter_unsupported_for(PARAMETER));
    }
    if metadata
        .get("turn_id")
        .is_some_and(|value| !value.is_null() && !value.is_string())
        || metadata
            .get("create_time")
            .is_some_and(|value| !value.is_null() && !value.is_number())
    {
        return Err(AdapterError::invalid_request().with_parameter(PARAMETER));
    }
    Ok(())
}

pub(super) fn tool_result_id(item: &Value) -> AdapterResult<&str> {
    // A named standalone result is valid Responses input, but foreign tool
    // histories require an actual call; neither a name nor an item ID is one.
    if item.get("type").and_then(Value::as_str) == Some("function_call_output")
        && item.get("call_id").is_none_or(Value::is_null)
    {
        return Err(AdapterError::parameter_unsupported_for("input.call_id"));
    }
    required_text(item, "call_id").map_err(|error| error.with_parameter("input.call_id"))
}

fn tool_result_content(item: &Value) -> AdapterResult<Vec<Block>> {
    let output = item
        .get("output")
        .ok_or_else(AdapterError::invalid_request)?;
    if item.get("type").and_then(Value::as_str) == Some("custom_tool_call_output")
        && !match output {
            Value::String(_) => true,
            Value::Array(parts) => parts.iter().all(|part| part.get("type").is_some()),
            _ => false,
        }
    {
        return Err(AdapterError::invalid_request().with_parameter("input.output"));
    }
    super::content::tool_result_blocks(output, WireApi::Responses)
}

fn validate_call_execution(call: &Value) -> AdapterResult<()> {
    validate_direct_tool_execution(
        call,
        AdapterError::parameter_unsupported_for("input.async"),
        AdapterError::parameter_unsupported_for("input.caller"),
    )
}

/// The upstream name of a replayed call: namespaced calls use the flattened
/// name that the tool catalog declared for them.
fn upstream_call_name(call_item: &Value) -> AdapterResult<String> {
    let name = required_text(call_item, "name")?;
    let Some(namespace) = call_item
        .get("namespace")
        .filter(|namespace_value| !namespace_value.is_null())
    else {
        return Ok(name.to_owned());
    };
    let namespace = namespace
        .as_str()
        .filter(|namespace_name| !namespace_name.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    Ok(super::super::super::contracts::bridged_namespace_tool_name(
        namespace, name,
    ))
}

/// Responses emits each tool call as an output item; Chat Completions needs
/// adjacent calls in one assistant message before the corresponding results.
fn append_assistant_blocks(messages: &mut Vec<Message>, blocks: Vec<Block>) {
    if let Some(message) = messages
        .last_mut()
        .filter(|message| message.role == Role::Assistant)
    {
        message.blocks.extend(blocks);
    } else {
        messages.push(Message {
            role: Role::Assistant,
            blocks,
        });
    }
}
