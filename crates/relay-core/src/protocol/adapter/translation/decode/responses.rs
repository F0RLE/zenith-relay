use super::super::{
    checked, optional_bool, required_text, AdapterError, AdapterResult, Block, Message, Reasoning,
    Request, Role,
};
use crate::WireApi;
use serde_json::{json, Value};

pub(super) fn decode(value: &Value) -> AdapterResult<Request> {
    super::super::super::contracts::validate_responses_bridge_request(
        value,
        WireApi::ChatCompletions,
    )?;
    let mut request = Request::default();
    if let Some(instructions) = value.get("instructions").filter(|v| !v.is_null()) {
        request.instructions = Some(Message {
            role: Role::System,
            blocks: super::content::text_blocks(instructions, WireApi::Responses)?,
        });
    }
    let input = value
        .get("input")
        .ok_or_else(AdapterError::invalid_request)?;
    if let Some(text) = input.as_str() {
        request.messages.push(Message {
            role: Role::User,
            blocks: vec![Block::Text(text.into())],
        });
    } else {
        for item in input.as_array().ok_or_else(AdapterError::invalid_request)? {
            match item
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("message")
            {
                "message" => {
                    checked(item, &["type", "role", "content", "id", "status"])?;
                    request.messages.push(Message {
                        role: super::content::role(item)?,
                        blocks: super::content::text_blocks(
                            item.get("content")
                                .ok_or_else(AdapterError::invalid_request)?,
                            WireApi::Responses,
                        )?,
                    });
                }
                "function_call" => {
                    checked(
                        item,
                        &["type", "id", "status", "call_id", "name", "arguments"],
                    )?;
                    append_assistant_blocks(
                        &mut request.messages,
                        vec![Block::ToolCall {
                            id: required_text(item, "call_id")?.into(),
                            name: required_text(item, "name")?.into(),
                            arguments: required_text(item, "arguments")?.into(),
                        }],
                    );
                }
                "custom_tool_call" => {
                    checked(item, &["type", "id", "status", "call_id", "name", "input"])?;
                    let input = required_text(item, "input")?;
                    append_assistant_blocks(
                        &mut request.messages,
                        vec![Block::ToolCall {
                            id: required_text(item, "call_id")?.into(),
                            name: required_text(item, "name")?.into(),
                            arguments: serde_json::to_string(&json!({"input": input}))
                                .map_err(|_| AdapterError::invalid_request())?,
                        }],
                    );
                }
                "function_call_output" | "custom_tool_call_output" => {
                    checked(item, &["type", "id", "status", "call_id", "output"])?;
                    request.messages.push(Message {
                        role: Role::User,
                        blocks: vec![Block::ToolResult {
                            id: required_text(item, "call_id")?.into(),
                            name: String::new(),
                            content: super::content::plain_text(
                                item.get("output")
                                    .ok_or_else(AdapterError::invalid_request)?,
                                WireApi::Responses,
                            )?,
                            is_error: false,
                        }],
                    });
                }
                "reasoning" => {
                    // Public summaries are portable. Encrypted state is not:
                    // `checked` rejects it instead of dropping its ownership.
                    checked(item, &["type", "id", "status", "summary"])?;
                    let blocks = item
                        .get("summary")
                        .and_then(Value::as_array)
                        .ok_or_else(AdapterError::invalid_request)?
                        .iter()
                        .map(|summary| {
                            checked(summary, &["type", "text"])?;
                            if required_text(summary, "type")? != "summary_text" {
                                return Err(AdapterError::parameter_unsupported());
                            }
                            Ok(Block::Reasoning(
                                summary
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .ok_or_else(AdapterError::invalid_request)?
                                    .into(),
                            ))
                        })
                        .collect::<AdapterResult<Vec<_>>>()?;
                    append_assistant_blocks(&mut request.messages, blocks);
                }
                _ => return Err(AdapterError::parameter_unsupported()),
            }
        }
    }
    request.tools = super::content::tools(value.get("tools"), WireApi::Responses)?;
    request.tool_choice = super::content::choice(value.get("tool_choice"), WireApi::Responses)?;
    request.parallel_tools = optional_bool(value, "parallel_tool_calls")?;
    super::content::common(&mut request, value, "max_output_tokens", "top_p", "stop")?;
    if let Some(text) = value.get("text").filter(|v| !v.is_null()) {
        checked(text, &["format"])?;
        request.output_format =
            super::content::output_format(text.get("format").unwrap_or(&Value::Null))?;
    }
    if let Some(reasoning) = value.get("reasoning").filter(|v| !v.is_null()) {
        if let Some(effort) = reasoning.get("effort").filter(|value| !value.is_null()) {
            request.reasoning = Some(Reasoning::Effort(
                effort
                    .as_str()
                    .ok_or_else(AdapterError::invalid_request)?
                    .into(),
            ));
        }
    }
    Ok(request)
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
