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
        || crate::protocol::completion::ResponseCompletion::for_protocol(
            protocol,
            upstream_response,
        ) == crate::protocol::completion::ResponseCompletion::Invalid
    {
        return Err(invalid()
            .with_parameter("response")
            .with_shape_diagnostic(upstream_response, None));
    }
    let mut response = Response {
        id: upstream_response
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(seed)
            .into(),
        blocks: Vec::new(),
        citations: BTreeMap::new(),
        usage: usage(protocol, upstream_response),
        finish: Finish::Stop,
        native_output: Vec::new(),
    };
    match protocol {
        WireApi::GeminiInteractions => {
            return Err(AdapterError::parameter_unsupported_for(
                "gemini_interactions.native_only",
            ));
        }
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
    validate_calls(&response.blocks, response.finish)?;
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
    if choice
        .get("index")
        .is_some_and(|index| index.as_u64() != Some(0))
    {
        return Err(invalid().with_parameter("choices[].index"));
    }
    response.finish = finish(
        protocol,
        choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?,
    )?;
    let message = choice.get("message").ok_or_else(invalid)?;
    checked(
        message,
        &[
            "role",
            "content",
            "tool_calls",
            "refusal",
            "reasoning_content",
            "reasoning",
            "annotations",
        ],
    )
    .map_err(|cause| {
        invalid()
            .with_parameter("choices[].message")
            .with_diagnostics_from(cause)
    })?;
    if message
        .get("role")
        .is_some_and(|role| !role.is_null() && role.as_str() != Some("assistant"))
    {
        return Err(invalid());
    }
    let mut refusal = message
        .get("refusal")
        .filter(|refusal_value| !refusal_value.is_null())
        .map(|refusal_value| refusal_value.as_str().ok_or_else(invalid))
        .transpose()?;
    if refusal.is_some() && response.finish == Finish::Stop {
        response.finish = Finish::Refusal;
    }
    if let Some(reasoning) = chat_reasoning_text(
        message,
        invalid().with_parameter("choices[].message.reasoning"),
    )? {
        response.blocks.push(Block::Reasoning(reasoning.into()));
    }
    if let Some(content) = message
        .get("content")
        .filter(|content_value| !content_value.is_null())
    {
        let mut text = String::new();
        let mut has_text = false;
        if let Some(value) = content.as_str() {
            text.push_str(value);
            has_text = true;
        } else {
            for part in content.as_array().ok_or_else(invalid)? {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        checked(part, &["type", "text"]).map_err(|_| invalid())?;
                        text.push_str(
                            part.get("text")
                                .and_then(Value::as_str)
                                .ok_or_else(invalid)?,
                        );
                        has_text = true;
                    }
                    Some("refusal") => {
                        checked(part, &["type", "refusal"]).map_err(|_| invalid())?;
                        let text = part
                            .get("refusal")
                            .and_then(Value::as_str)
                            .ok_or_else(invalid)?;
                        if refusal.is_some_and(|existing| existing != text) {
                            return Err(invalid());
                        }
                        refusal = Some(text);
                        if response.finish == Finish::Stop {
                            response.finish = Finish::Refusal;
                        }
                    }
                    _ => return Err(invalid()),
                }
            }
        }
        if has_text {
            response.blocks.push(Block::Text(text));
        }
    }
    citations::set_chat_annotations(response, message.get("annotations"))?;
    if let Some(refusal) = refusal {
        response.blocks.push(Block::Refusal(refusal.into()));
    }
    if let Some(calls) = message.get("tool_calls").filter(|calls| !calls.is_null()) {
        for call in calls.as_array().ok_or_else(invalid)? {
            checked(call, &["id", "type", "function"]).map_err(|_| invalid())?;
            if call.get("type").and_then(Value::as_str) != Some("function") {
                return Err(invalid());
            }
            let function = call.get("function").ok_or_else(invalid)?;
            checked(function, &["name", "arguments"]).map_err(|_| invalid())?;
            response.blocks.push(Block::ToolCall {
                id: required_text(call, "id").map_err(|_| invalid())?.into(),
                name: required_text(function, "name")
                    .map_err(|_| invalid())?
                    .into(),
                arguments: function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?
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
        if matches!(
            response_item.get("type").and_then(Value::as_str),
            Some("function_call" | "custom_tool_call")
        ) {
            super::validate_responses_call_execution(response_item)?;
        }
        match response_item.get("type").and_then(Value::as_str) {
            Some("tool_search_call")
                if response_item.get("execution").and_then(Value::as_str) == Some("client") =>
            {
                response.blocks.push(
                    super::super::tool_search::history_block(response_item)
                        .map_err(|_| invalid())?,
                );
            }
            Some("message") => {
                if response_item
                    .get("role")
                    .is_some_and(|role| role.as_str() != Some("assistant"))
                {
                    return Err(invalid().with_parameter("output[].role"));
                }
                for response_content in response_item
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(invalid)?
                {
                    if response_content.get("type").and_then(Value::as_str) == Some("refusal") {
                        response.blocks.push(Block::Refusal(
                            response_content
                                .get("refusal")
                                .and_then(Value::as_str)
                                .ok_or_else(invalid)?
                                .into(),
                        ));
                        if response.finish == Finish::Stop {
                            response.finish = Finish::Refusal;
                        }
                        continue;
                    }
                    if response_content.get("type").and_then(Value::as_str) != Some("output_text") {
                        return Err(invalid());
                    }
                    let text = response_content
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid)?;
                    let citations =
                        citations::parse_annotations(response_content.get("annotations"), text)?;
                    if !citations.is_empty() {
                        response.citations.insert(response.blocks.len(), citations);
                    }
                    response.blocks.push(Block::Text(text.into()));
                }
            }
            Some("web_search_call")
                if response_item.get("status").and_then(Value::as_str) == Some("completed") => {}
            Some("function_call") => response.blocks.push(Block::ToolCall {
                id: required_text(response_item, "call_id")
                    .map_err(|_| invalid())?
                    .into(),
                name: required_text(response_item, "name")
                    .map_err(|_| invalid())?
                    .into(),
                arguments: response_item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?
                    .into(),
            }),
            Some("custom_tool_call") => response.blocks.push(Block::ToolCall {
                id: required_text(response_item, "call_id")
                    .map_err(|_| invalid())?
                    .into(),
                name: required_text(response_item, "name")
                    .map_err(|_| invalid())?
                    .into(),
                arguments: serde_json::json!({"input": response_item.get("input")
                    .and_then(Value::as_str).ok_or_else(invalid)?})
                .to_string(),
            }),
            Some("reasoning") => {
                if response_item
                    .get("encrypted_content")
                    .is_some_and(|value| !value.is_null() && !value.is_string())
                {
                    return Err(invalid());
                }
                let summaries = match response_item.get("summary") {
                    Some(summaries) => Some(summaries.as_array().ok_or_else(invalid)?),
                    None if response_item
                        .get("encrypted_content")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty()) =>
                    {
                        None
                    }
                    None => return Err(invalid()),
                };
                for reasoning_summary in summaries.into_iter().flatten() {
                    if reasoning_summary.get("type").and_then(Value::as_str) != Some("summary_text")
                    {
                        return Err(invalid());
                    }
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
    response.native_output = upstream_response["output"]
        .as_array()
        .ok_or_else(invalid)?
        .clone();
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
    if upstream_response
        .get("role")
        .is_some_and(|role| role.as_str() != Some("assistant"))
    {
        return Err(invalid().with_parameter("role"));
    }
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
        super::super::super::messages::validate_response_block(message_block)?;
        match message_block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let text = message_block
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?;
                let citations = web_search::citations(message_block, text)?;
                if !citations.is_empty() {
                    response.citations.insert(response.blocks.len(), citations);
                }
                response.blocks.push(Block::Text(
                    message_block
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid)?
                        .into(),
                ));
            }
            Some("thinking") => {
                required_text(message_block, "signature").map_err(|_| invalid())?;
                response.blocks.push(Block::Reasoning(
                    message_block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid)?
                        .into(),
                ));
            }
            Some("redacted_thinking") => {
                required_text(message_block, "data").map_err(|_| invalid())?;
            }
            Some("server_tool_use") => {
                let mut search = web_search::start(message_block)?;
                search.query = web_search::query(&message_block["input"])?;
                response.blocks.push(Block::WebSearch(search));
            }
            Some("web_search_tool_result") => {
                web_search::finish(&mut response.blocks, message_block)?;
            }
            Some("tool_use") => {
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
            _ => return Err(invalid()),
        }
    }
    if response.finish == Finish::Refusal {
        response.blocks.push(Block::Refusal(
            upstream_response
                .pointer("/stop_details/explanation")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
        ));
    }
    response.native_output = upstream_response["content"]
        .as_array()
        .ok_or_else(invalid)?
        .clone();

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
    if candidates.len() != 1
        || candidates[0]
            .get("index")
            .is_some_and(|index| index.as_u64() != Some(0))
    {
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
        let fields = super::super::super::gemini::validate_response_part(part)?;
        if super::super::super::gemini::is_noop_text_part(fields) {
            continue;
        }
        let mut native_part = part.clone();
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            response.blocks.push(
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    Block::Reasoning(text.into())
                } else {
                    Block::Text(text.into())
                },
            );
        } else if let Some(call) = part.get("functionCall") {
            checked(call, &["id", "name", "args", "partialArgs", "willContinue"])?;
            if call
                .get("id")
                .is_some_and(|id| id.as_str().is_none_or(|id| id.trim().is_empty()))
                || call.get("willContinue").is_some_and(|flag| flag != false)
            {
                return Err(invalid());
            }
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{seed}_call_{index}"));
            let arguments = super::super::super::gemini::function_call_args(
                call.as_object().ok_or_else(invalid)?,
            )
            .map_err(|_| invalid())?;
            response.blocks.push(Block::ToolCall {
                id,
                name: required_text(call, "name").map_err(|_| invalid())?.into(),
                arguments: arguments.to_string(),
            });
            native_part["functionCall"]["args"] = arguments;
            let fields = native_part["functionCall"]
                .as_object_mut()
                .ok_or_else(invalid)?;
            fields.remove("partialArgs");
            fields.remove("willContinue");
        } else if part.get("thoughtSignature").is_none() {
            return Err(invalid());
        }
        response.native_output.push(native_part);
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
