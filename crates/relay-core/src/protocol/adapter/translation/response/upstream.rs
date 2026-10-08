use super::super::*;
use super::{finish, usage, validate_calls};
use serde_json::Value;

pub(in crate::protocol::adapter::translation) fn decode(
    protocol: WireApi,
    upstream_response: &Value,
    seed: &str,
) -> AdapterResult<Response> {
    let invalid = AdapterError::upstream_response_invalid;
    if upstream_response
        .get("error")
        .is_some_and(|error| !error.is_null())
    {
        return Err(invalid());
    }
    let mut response = Response {
        id: upstream_response
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(seed)
            .into(),
        blocks: Vec::new(),
        usage: usage(protocol, upstream_response),
        finish: Finish::Stop,
    };
    match protocol {
        WireApi::ChatCompletions => {
            decode_chat_completion(upstream_response, protocol, &mut response, invalid)?
        }
        WireApi::Responses => decode_responses(upstream_response, &mut response, invalid)?,
        WireApi::Messages => decode_messages(upstream_response, protocol, &mut response, invalid)?,
        WireApi::Gemini => {
            if super::super::super::gemini::prompt_blocked(upstream_response)
                .map_err(|()| invalid())?
            {
                response.finish = Finish::Filter;
                return Ok(response);
            }
            decode_gemini(upstream_response, protocol, seed, &mut response, invalid)?;
        }
    }
    validate_calls(&response.blocks)?;
    Ok(response)
}

fn decode_chat_completion(
    upstream_response: &Value,
    protocol: WireApi,
    response: &mut Response,
    invalid: fn() -> AdapterError,
) -> AdapterResult<()> {
    let choices = upstream_response
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
        .filter(|refusal_value| !refusal_value.is_null())
        .map(|refusal_value| refusal_value.as_str().ok_or_else(invalid))
        .transpose()?
        .filter(|refusal_text| !refusal_text.is_empty());
    if refusal.is_some() {
        response.finish = Finish::Filter;
    }
    if let Some(reasoning) = message
        .get("reasoning_content")
        .filter(|reasoning_value| !reasoning_value.is_null())
    {
        response.blocks.push(Block::Reasoning(
            reasoning.as_str().ok_or_else(invalid)?.into(),
        ));
    }
    if let Some(text) = message
        .get("content")
        .filter(|content_value| !content_value.is_null())
    {
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
    upstream_response: &Value,
    response: &mut Response,
    invalid: fn() -> AdapterError,
) -> AdapterResult<()> {
    response.finish = match upstream_response.get("status").and_then(Value::as_str) {
        Some("completed") => Finish::Stop,
        Some("incomplete") => match upstream_response
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
        {
            Some("max_output_tokens") => Finish::Length,
            Some("content_filter") => Finish::Filter,
            _ => return Err(invalid()),
        },
        _ => return Err(invalid()),
    };
    for response_item in upstream_response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?
    {
        match response_item.get("type").and_then(Value::as_str) {
            Some("message") => {
                for response_content in response_item
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(invalid)?
                {
                    if response_content.get("type").and_then(Value::as_str) != Some("output_text") {
                        return Err(invalid());
                    }
                    if response_content
                        .get("annotations")
                        .and_then(Value::as_array)
                        .is_some_and(|annotation_items| !annotation_items.is_empty())
                    {
                        return Err(invalid());
                    }
                    response.blocks.push(Block::Text(
                        response_content
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?
                            .into(),
                    ));
                }
            }
            Some("function_call") => response.blocks.push(Block::ToolCall {
                id: required_text(response_item, "call_id")
                    .map_err(|_| invalid())?
                    .into(),
                name: required_text(response_item, "name")
                    .map_err(|_| invalid())?
                    .into(),
                arguments: required_text(response_item, "arguments")
                    .map_err(|_| invalid())?
                    .into(),
            }),
            Some("reasoning") => {
                if response_item
                    .get("encrypted_content")
                    .is_some_and(|encrypted_content| !encrypted_content.is_null())
                {
                    return Err(invalid());
                }
                for reasoning_summary in response_item
                    .get("summary")
                    .and_then(Value::as_array)
                    .ok_or_else(invalid)?
                {
                    response.blocks.push(Block::Reasoning(
                        reasoning_summary
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
    upstream_response: &Value,
    protocol: WireApi,
    response: &mut Response,
    invalid: fn() -> AdapterError,
) -> AdapterResult<()> {
    response.finish = finish(
        protocol,
        upstream_response
            .get("stop_reason")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?,
    )?;
    for message_block in upstream_response
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?
    {
        match message_block.get("type").and_then(Value::as_str) {
            Some("text") => {
                checked(message_block, &["type", "text"])?;
                response.blocks.push(Block::Text(
                    message_block
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid)?
                        .into(),
                ));
            }
            Some("thinking") => {
                checked(message_block, &["type", "thinking"])?;
                response.blocks.push(Block::Reasoning(
                    message_block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid)?
                        .into(),
                ));
            }
            Some("tool_use") => {
                checked(message_block, &["type", "id", "name", "input"])?;
                response.blocks.push(Block::ToolCall {
                    id: required_text(message_block, "id")
                        .map_err(|_| invalid())?
                        .into(),
                    name: required_text(message_block, "name")
                        .map_err(|_| invalid())?
                        .into(),
                    arguments: message_block
                        .get("input")
                        .filter(|tool_input| tool_input.is_object())
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
    upstream_response: &Value,
    protocol: WireApi,
    seed: &str,
    response: &mut Response,
    invalid: fn() -> AdapterError,
) -> AdapterResult<()> {
    let candidates = upstream_response
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
