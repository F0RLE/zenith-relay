use super::super::{
    checked, optional_bool, optional_u64, required_text, AdapterError, AdapterResult, Block,
    Message, Reasoning, Request, Role,
};
use crate::WireApi;
use serde_json::Value;

pub(super) fn decode(request_body: &Value) -> AdapterResult<Request> {
    checked(
        request_body,
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
    if optional_u64(request_body, "n")?.is_some_and(|n| n != 1)
        || optional_bool(request_body, "store")? == Some(true)
    {
        return Err(AdapterError::parameter_unsupported());
    }
    if let Some(options) = request_body
        .get("stream_options")
        .filter(|stream_options_value| !stream_options_value.is_null())
    {
        checked(options, &["include_usage"])?;
    }
    let mut decoded_request = Request::default();
    for message in request_body
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
            .filter(|field_value| !field_value.is_null());
        if reasoning.is_some() && super::content::role(message)? != Role::Assistant {
            return Err(AdapterError::invalid_request());
        }
        if message.get("role").and_then(Value::as_str) == Some("tool") {
            decoded_request.messages.push(Message {
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
        decoded_request.messages.push(Message {
            role: super::content::role(message)?,
            blocks,
        });
    }
    decoded_request.tools =
        super::content::tools(request_body.get("tools"), WireApi::ChatCompletions)?;
    decoded_request.tool_choice =
        super::content::choice(request_body.get("tool_choice"), WireApi::ChatCompletions)?;
    decoded_request.parallel_tools = optional_bool(request_body, "parallel_tool_calls")?;
    super::content::common(
        &mut decoded_request,
        request_body,
        if request_body.get("max_completion_tokens").is_some() {
            "max_completion_tokens"
        } else {
            "max_tokens"
        },
        "top_p",
        "stop",
    )?;
    decoded_request.output_format =
        super::content::output_format(request_body.get("response_format").unwrap_or(&Value::Null))?;
    decoded_request.reasoning = request_body
        .get("reasoning_effort")
        .filter(|reasoning_effort_value| !reasoning_effort_value.is_null())
        .map(|reasoning_effort_value| {
            reasoning_effort_value
                .as_str()
                .map(|effort| Reasoning::Effort(effort.into()))
                .ok_or_else(AdapterError::invalid_request)
        })
        .transpose()?;
    Ok(decoded_request)
}
