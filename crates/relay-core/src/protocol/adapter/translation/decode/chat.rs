use super::super::{
    checked, optional_bool, optional_u64, required_text, AdapterError, AdapterResult, Block,
    Message, Reasoning, Request, Role,
};
use crate::WireApi;
use serde_json::Value;

pub(super) fn decode(value: &Value) -> AdapterResult<Request> {
    checked(
        value,
        &[
            "model",
            "stream",
            "messages",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
            "max_tokens",
            "max_completion_tokens",
            "temperature",
            "top_p",
            "stop",
            "response_format",
            "reasoning_effort",
            "stream_options",
            "n",
            "store",
        ],
    )?;
    if optional_u64(value, "n")?.is_some_and(|n| n != 1)
        || optional_bool(value, "store")? == Some(true)
    {
        return Err(AdapterError::parameter_unsupported());
    }
    if let Some(options) = value.get("stream_options").filter(|v| !v.is_null()) {
        checked(options, &["include_usage"])?;
    }
    let mut request = Request::default();
    for message in value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::invalid_request)?
    {
        checked(
            message,
            &[
                "role",
                "content",
                "tool_calls",
                "tool_call_id",
                "reasoning_content",
            ],
        )?;
        let reasoning = message
            .get("reasoning_content")
            .filter(|value| !value.is_null());
        if reasoning.is_some() && super::content::role(message)? != Role::Assistant {
            return Err(AdapterError::invalid_request());
        }
        if message.get("role").and_then(Value::as_str) == Some("tool") {
            request.messages.push(Message {
                role: Role::User,
                blocks: vec![Block::ToolResult {
                    id: required_text(message, "tool_call_id")?.into(),
                    name: String::new(),
                    content: super::content::plain_text(
                        message.get("content").unwrap_or(&Value::Null),
                        WireApi::ChatCompletions,
                    )?,
                    is_error: false,
                }],
            });
            continue;
        }
        let mut blocks = super::content::text_blocks(
            message.get("content").unwrap_or(&Value::Null),
            WireApi::ChatCompletions,
        )?;
        if let Some(reasoning) = reasoning {
            blocks.insert(
                0,
                Block::Reasoning(
                    reasoning
                        .as_str()
                        .ok_or_else(AdapterError::invalid_request)?
                        .into(),
                ),
            );
        }
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                checked(call, &["id", "type", "function"])?;
                if required_text(call, "type")? != "function" {
                    return Err(AdapterError::unsupported_tool());
                }
                let function = call
                    .get("function")
                    .ok_or_else(AdapterError::invalid_request)?;
                checked(function, &["name", "arguments"])?;
                blocks.push(Block::ToolCall {
                    id: required_text(call, "id")?.into(),
                    name: required_text(function, "name")?.into(),
                    arguments: required_text(function, "arguments")?.into(),
                });
            }
        }
        request.messages.push(Message {
            role: super::content::role(message)?,
            blocks,
        });
    }
    request.tools = super::content::tools(value.get("tools"), WireApi::ChatCompletions)?;
    request.tool_choice =
        super::content::choice(value.get("tool_choice"), WireApi::ChatCompletions)?;
    request.parallel_tools = optional_bool(value, "parallel_tool_calls")?;
    super::content::common(
        &mut request,
        value,
        if value.get("max_completion_tokens").is_some() {
            "max_completion_tokens"
        } else {
            "max_tokens"
        },
        "top_p",
        "stop",
    )?;
    request.output_format =
        super::content::output_format(value.get("response_format").unwrap_or(&Value::Null))?;
    request.reasoning = value
        .get("reasoning_effort")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .map(|effort| Reasoning::Effort(effort.into()))
                .ok_or_else(AdapterError::invalid_request)
        })
        .transpose()?;
    Ok(request)
}
