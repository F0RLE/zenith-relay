use super::super::{
    checked, optional_bool, required_text, AdapterError, AdapterResult, Block, Message, Reasoning,
    Request, Role,
};
use crate::WireApi;
use serde_json::{json, Value};

pub(super) fn decode(request_body: &Value) -> AdapterResult<Request> {
    super::super::super::contracts::validate_responses_bridge_request(
        request_body,
        WireApi::ChatCompletions,
    )?;
    let mut decoded_request = Request::default();
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
                    checked(response_item, &["type", "role", "content", "id", "status"])?;
                    decoded_request.messages.push(Message {
                        role: super::content::role(response_item)?,
                        blocks: super::content::text_blocks(
                            response_item
                                .get("content")
                                .ok_or_else(AdapterError::invalid_request)?,
                            WireApi::Responses,
                        )?,
                    });
                }
                "function_call" => {
                    checked(
                        response_item,
                        &["type", "id", "status", "call_id", "name", "arguments"],
                    )?;
                    append_assistant_blocks(
                        &mut decoded_request.messages,
                        vec![Block::ToolCall {
                            id: required_text(response_item, "call_id")?.into(),
                            name: required_text(response_item, "name")?.into(),
                            arguments: required_text(response_item, "arguments")?.into(),
                        }],
                    );
                }
                "custom_tool_call" => {
                    checked(
                        response_item,
                        &["type", "id", "status", "call_id", "name", "input"],
                    )?;
                    let tool_input = required_text(response_item, "input")?;
                    append_assistant_blocks(
                        &mut decoded_request.messages,
                        vec![Block::ToolCall {
                            id: required_text(response_item, "call_id")?.into(),
                            name: required_text(response_item, "name")?.into(),
                            arguments: serde_json::to_string(&json!({"input": tool_input}))
                                .map_err(|_| AdapterError::invalid_request())?,
                        }],
                    );
                }
                "function_call_output" | "custom_tool_call_output" => {
                    checked(
                        response_item,
                        &["type", "id", "status", "call_id", "output"],
                    )?;
                    decoded_request.messages.push(Message {
                        role: Role::User,
                        blocks: vec![Block::ToolResult {
                            id: required_text(response_item, "call_id")?.into(),
                            name: String::new(),
                            content: super::content::plain_text(
                                response_item
                                    .get("output")
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
                    checked(response_item, &["type", "id", "status", "summary"])?;
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
                _ => return Err(AdapterError::parameter_unsupported()),
            }
        }
    }
    decoded_request.tools = super::content::tools(request_body.get("tools"), WireApi::Responses)?;
    decoded_request.tool_choice =
        super::content::choice(request_body.get("tool_choice"), WireApi::Responses)?;
    decoded_request.parallel_tools = optional_bool(request_body, "parallel_tool_calls")?;
    super::content::common(
        &mut decoded_request,
        request_body,
        "max_output_tokens",
        "top_p",
        "stop",
    )?;
    if let Some(text_config) = request_body
        .get("text")
        .filter(|text_config_value| !text_config_value.is_null())
    {
        checked(text_config, &["format"])?;
        decoded_request.output_format =
            super::content::output_format(text_config.get("format").unwrap_or(&Value::Null))?;
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
