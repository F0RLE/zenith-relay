use super::super::{
    AdapterError, AdapterResult, Block, Message, Reasoning, Request, Role, checked, optional_bool,
    optional_u64, required_text,
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
            "presence_penalty",
            "frequency_penalty",
            "seed",
            "stop",
            "response_format",
            "reasoning_effort",
            "stream_options",
            "n",
            "store",
            "metadata",
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
        optional_bool(options, "include_usage")
            .map_err(|error| error.with_parameter("stream_options.include_usage"))?;
    }
    let mut decoded_request = Request {
        metadata: super::super::options::metadata(request_body, WireApi::ChatCompletions)?,
        ..Default::default()
    };
    super::content::sampling_extensions(&mut decoded_request, request_body, false)?;
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
                "function_call",
                "tool_call_id",
                "name",
                "reasoning_content",
                "reasoning",
                "refusal",
            ],
        )?;
        if message.get("name").is_some_and(|name| !name.is_null())
            && message.get("role").and_then(Value::as_str) != Some("function")
        {
            return Err(AdapterError::parameter_unsupported_for("messages[].name"));
        }
        let refusal = message
            .get("refusal")
            .filter(|value| !value.is_null())
            .map(|value| {
                value.as_str().ok_or_else(|| {
                    AdapterError::invalid_request().with_parameter("messages[].refusal")
                })
            })
            .transpose()?;
        if refusal.is_some() && message.get("role").and_then(Value::as_str) != Some("assistant") {
            return Err(AdapterError::invalid_request().with_parameter("messages[].refusal"));
        }
        let calls = message
            .get("tool_calls")
            .filter(|value| !value.is_null())
            .map(|value| {
                value.as_array().ok_or_else(|| {
                    AdapterError::invalid_request().with_parameter("messages[].tool_calls")
                })
            })
            .transpose()?;
        let legacy_call = message
            .get("function_call")
            .filter(|value| !value.is_null());
        if legacy_call.is_some() && calls.is_some_and(|calls| !calls.is_empty()) {
            return Err(AdapterError::invalid_request().with_parameter("messages[].function_call"));
        }
        let reasoning = super::super::chat_reasoning_text(
            message,
            AdapterError::invalid_request().with_parameter("messages[].reasoning"),
        )?;
        if reasoning.is_some()
            && super::content::role(message, WireApi::ChatCompletions)? != Role::Assistant
        {
            return Err(AdapterError::invalid_request());
        }
        if matches!(
            message.get("role").and_then(Value::as_str),
            Some("tool" | "function")
        ) {
            if legacy_call.is_some() || calls.is_some_and(|calls| !calls.is_empty()) {
                return Err(AdapterError::invalid_request().with_parameter("messages[].tool_calls"));
            }
            let (tool_result_id, tool_result_name) =
                if message.get("role").and_then(Value::as_str) == Some("function") {
                    // Legacy `function` role carries `name`, not `tool_call_id`.
                    (String::new(), required_text(message, "name")?.to_string())
                } else {
                    (
                        required_text(message, "tool_call_id")?.to_string(),
                        String::new(),
                    )
                };
            decoded_request.messages.push(Message {
                role: Role::User,
                blocks: vec![Block::ToolResult {
                    id: tool_result_id,
                    name: tool_result_name,
                    content: super::content::tool_result_blocks(
                        message.get("content").unwrap_or(&Value::Null),
                        WireApi::ChatCompletions,
                    )?,
                    is_error: false,
                    raw_text: true,
                }],
            });
            continue;
        }
        let mut blocks = super::content::text_blocks(
            message.get("content").unwrap_or(&Value::Null),
            WireApi::ChatCompletions,
        )?;
        let role = super::content::role(message, WireApi::ChatCompletions)?;
        if let Some(refusal) = refusal {
            if blocks
                .iter()
                .any(|block| matches!(block, Block::Refusal(_)))
            {
                let content_refusal = blocks
                    .iter()
                    .filter_map(|block| match block {
                        Block::Refusal(text) => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>();
                if content_refusal != refusal {
                    return Err(
                        AdapterError::invalid_request().with_parameter("messages[].refusal")
                    );
                }
            } else {
                blocks.push(Block::Refusal(refusal.into()));
            }
        }
        if let Some(reasoning) = reasoning {
            blocks.insert(0, Block::Reasoning(reasoning.into()));
        }
        if let Some(calls) = calls {
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
        // Legacy single `function_call` field (pre-tool_calls format).
        if let Some(legacy_call) = legacy_call {
            checked(legacy_call, &["name", "arguments"])?;
            blocks.push(Block::ToolCall {
                id: String::new(),
                name: required_text(legacy_call, "name")?.into(),
                arguments: legacy_call
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
            });
        }
        decoded_request.messages.push(Message { role, blocks });
    }
    decoded_request.tools =
        super::content::tools(request_body.get("tools"), WireApi::ChatCompletions)?;
    decoded_request.tool_choice =
        super::content::choice(request_body.get("tool_choice"), WireApi::ChatCompletions)?;
    decoded_request.parallel_tools = optional_bool(request_body, "parallel_tool_calls")?;
    super::content::common(
        &mut decoded_request,
        request_body,
        "max_completion_tokens",
        "top_p",
        "stop",
    )?;
    let old_limit = optional_u64(request_body, "max_tokens")?;
    if decoded_request
        .max_tokens
        .zip(old_limit)
        .is_some_and(|(limit, old_limit)| limit != old_limit)
    {
        return Err(AdapterError::invalid_request().with_parameter("max_completion_tokens"));
    }
    decoded_request.max_tokens = decoded_request.max_tokens.or(old_limit);
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
