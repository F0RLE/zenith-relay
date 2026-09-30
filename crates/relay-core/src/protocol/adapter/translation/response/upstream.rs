use super::super::*;
use super::{finish, usage, validate_calls};
use serde_json::Value;

pub(in crate::protocol::adapter::translation) fn decode(
    protocol: WireApi,
    value: &Value,
    seed: &str,
) -> AdapterResult<Response> {
    let invalid = AdapterError::upstream_response_invalid;
    if value.get("error").is_some_and(|error| !error.is_null()) {
        return Err(invalid());
    }
    let mut response = Response {
        id: value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(seed)
            .into(),
        blocks: Vec::new(),
        usage: usage(protocol, value),
        finish: Finish::Stop,
    };
    match protocol {
        WireApi::ChatCompletions => {
            decode_chat_completion(value, protocol, &mut response, invalid)?
        }
        WireApi::Responses => decode_responses(value, &mut response, invalid)?,
        WireApi::Messages => decode_messages(value, protocol, &mut response, invalid)?,
        WireApi::Gemini => {
            if super::super::super::gemini::prompt_blocked(value).map_err(|()| invalid())? {
                response.finish = Finish::Filter;
                return Ok(response);
            }
            decode_gemini(value, protocol, seed, &mut response, invalid)?;
        }
    }
    validate_calls(&response.blocks)?;
    Ok(response)
}

fn decode_chat_completion(
    value: &Value,
    protocol: WireApi,
    response: &mut Response,
    invalid: fn() -> AdapterError,
) -> AdapterResult<()> {
    let choices = value
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if choices.len() != 1 {
        return Err(invalid());
    }
    let choice = &choices[0];
    response.finish = finish(
        protocol,
        choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?,
    )?;
    let message = choice.get("message").ok_or_else(invalid)?;
    let refusal = message
        .get("refusal")
        .filter(|value| !value.is_null())
        .map(|value| value.as_str().ok_or_else(invalid))
        .transpose()?
        .filter(|value| !value.is_empty());
    if refusal.is_some() {
        response.finish = Finish::Filter;
    }
    if let Some(reasoning) = message
        .get("reasoning_content")
        .filter(|value| !value.is_null())
    {
        response.blocks.push(Block::Reasoning(
            reasoning.as_str().ok_or_else(invalid)?.into(),
        ));
    }
    if let Some(text) = message.get("content").filter(|value| !value.is_null()) {
        if let Some(text) = text.as_str() {
            response.blocks.push(Block::Text(text.into()));
        } else {
            for part in text.as_array().ok_or_else(invalid)? {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => response.blocks.push(Block::Text(
                        part.get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?
                            .into(),
                    )),
                    Some("refusal") => {
                        response.finish = Finish::Filter;
                        response.blocks.push(Block::Text(
                            part.get("refusal")
                                .and_then(Value::as_str)
                                .ok_or_else(invalid)?
                                .into(),
                        ));
                    }
                    _ => return Err(invalid()),
                }
            }
        }
    } else if let Some(refusal) = refusal {
        response.blocks.push(Block::Text(refusal.into()));
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            if call.get("type").and_then(Value::as_str) != Some("function") {
                return Err(invalid());
            }
            let function = call.get("function").ok_or_else(invalid)?;
            response.blocks.push(Block::ToolCall {
                id: required_text(call, "id").map_err(|_| invalid())?.into(),
                name: required_text(function, "name")
                    .map_err(|_| invalid())?
                    .into(),
                arguments: required_text(function, "arguments")
                    .map_err(|_| invalid())?
                    .into(),
            });
        }
    }

    Ok(())
}

fn decode_responses(
    value: &Value,
    response: &mut Response,
    invalid: fn() -> AdapterError,
) -> AdapterResult<()> {
    response.finish = match value.get("status").and_then(Value::as_str) {
        Some("completed") => Finish::Stop,
        Some("incomplete") => match value
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
        {
            Some("max_output_tokens") => Finish::Length,
            Some("content_filter") => Finish::Filter,
            _ => return Err(invalid()),
        },
        _ => return Err(invalid()),
    };
    for item in value
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?
    {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                for content in item
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(invalid)?
                {
                    if content.get("type").and_then(Value::as_str) != Some("output_text") {
                        return Err(invalid());
                    }
                    if content
                        .get("annotations")
                        .and_then(Value::as_array)
                        .is_some_and(|items| !items.is_empty())
                    {
                        return Err(invalid());
                    }
                    response.blocks.push(Block::Text(
                        content
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?
                            .into(),
                    ));
                }
            }
            Some("function_call") => response.blocks.push(Block::ToolCall {
                id: required_text(item, "call_id")
                    .map_err(|_| invalid())?
                    .into(),
                name: required_text(item, "name").map_err(|_| invalid())?.into(),
                arguments: required_text(item, "arguments")
                    .map_err(|_| invalid())?
                    .into(),
            }),
            Some("reasoning") => {
                if item
                    .get("encrypted_content")
                    .is_some_and(|value| !value.is_null())
                {
                    return Err(invalid());
                }
                for summary in item
                    .get("summary")
                    .and_then(Value::as_array)
                    .ok_or_else(invalid)?
                {
                    response.blocks.push(Block::Reasoning(
                        summary
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?
                            .into(),
                    ));
                }
            }
            _ => return Err(invalid()),
        }
    }
    if response.finish == Finish::Stop
        && response
            .blocks
            .iter()
            .any(|block| matches!(block, Block::ToolCall { .. }))
    {
        response.finish = Finish::Tools;
    }

    Ok(())
}

fn decode_messages(
    value: &Value,
    protocol: WireApi,
    response: &mut Response,
    invalid: fn() -> AdapterError,
) -> AdapterResult<()> {
    response.finish = finish(
        protocol,
        value
            .get("stop_reason")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?,
    )?;
    for block in value
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?
    {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                checked(block, &["type", "text"])?;
                response.blocks.push(Block::Text(
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid)?
                        .into(),
                ));
            }
            Some("thinking") => {
                checked(block, &["type", "thinking"])?;
                response.blocks.push(Block::Reasoning(
                    block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid)?
                        .into(),
                ));
            }
            Some("tool_use") => {
                checked(block, &["type", "id", "name", "input"])?;
                response.blocks.push(Block::ToolCall {
                    id: required_text(block, "id").map_err(|_| invalid())?.into(),
                    name: required_text(block, "name").map_err(|_| invalid())?.into(),
                    arguments: block
                        .get("input")
                        .filter(|value| value.is_object())
                        .ok_or_else(invalid)?
                        .to_string(),
                });
            }
            // Signed thinking is provider-owned continuation state.
            // It cannot be converted to another client's history.
            _ => return Err(invalid()),
        }
    }

    Ok(())
}

fn decode_gemini(
    value: &Value,
    protocol: WireApi,
    seed: &str,
    response: &mut Response,
    invalid: fn() -> AdapterError,
) -> AdapterResult<()> {
    let candidates = value
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if candidates.len() != 1 {
        return Err(invalid());
    }
    let candidate = &candidates[0];
    response.finish = finish(
        protocol,
        candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?,
    )?;
    let parts = candidate
        .pointer("/content/parts")
        .and_then(Value::as_array);
    if parts.is_none() && !matches!(response.finish, Finish::Filter | Finish::Length) {
        return Err(invalid());
    }
    for (index, part) in parts.into_iter().flatten().enumerate() {
        if part.get("thoughtSignature").is_some() {
            return Err(invalid());
        }
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            response.blocks.push(
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    Block::Reasoning(text.into())
                } else {
                    Block::Text(text.into())
                },
            );
        } else if let Some(call) = part.get("functionCall") {
            response.blocks.push(Block::ToolCall {
                id: call
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{seed}_call_{index}")),
                name: required_text(call, "name").map_err(|_| invalid())?.into(),
                arguments: call
                    .get("args")
                    .filter(|args| args.is_object())
                    .ok_or_else(invalid)?
                    .to_string(),
            });
        } else {
            return Err(invalid());
        }
    }
    if response.finish == Finish::Stop
        && response
            .blocks
            .iter()
            .any(|block| matches!(block, Block::ToolCall { .. }))
    {
        response.finish = Finish::Tools;
    }

    Ok(())
}
