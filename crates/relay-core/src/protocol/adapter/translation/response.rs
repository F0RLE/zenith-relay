use super::*;
use serde_json::{json, Map};

pub(super) fn decode(protocol: WireApi, value: &Value, seed: &str) -> AdapterResult<Response> {
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
        }
        WireApi::Responses => {
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
        }
        WireApi::Messages => {
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
        }
        WireApi::Gemini => {
            if super::super::gemini::prompt_blocked(value).map_err(|()| invalid())? {
                response.finish = Finish::Filter;
                return Ok(response);
            }
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
        }
    }
    validate_calls(&response.blocks)?;
    Ok(response)
}

pub(super) fn validate_calls(blocks: &[Block]) -> AdapterResult<()> {
    let mut ids = std::collections::BTreeSet::new();
    for block in blocks {
        if let Block::ToolCall {
            id,
            name,
            arguments,
        } = block
        {
            if id.is_empty()
                || name.is_empty()
                || !ids.insert(id)
                || serde_json::from_str::<Value>(arguments)
                    .ok()
                    .is_none_or(|args| !args.is_object())
            {
                return Err(AdapterError::upstream_response_invalid());
            }
        }
    }
    Ok(())
}

pub(super) fn finish(protocol: WireApi, value: &str) -> AdapterResult<Finish> {
    match (protocol, value) {
        (WireApi::ChatCompletions, "stop")
        | (WireApi::Messages, "end_turn" | "stop_sequence")
        | (WireApi::Gemini, "STOP") => Ok(Finish::Stop),
        (WireApi::ChatCompletions, "tool_calls") | (WireApi::Messages, "tool_use") => {
            Ok(Finish::Tools)
        }
        (WireApi::ChatCompletions, "length")
        | (WireApi::Messages, "max_tokens")
        | (WireApi::Gemini, "MAX_TOKENS") => Ok(Finish::Length),
        (WireApi::ChatCompletions, "content_filter")
        | (WireApi::Messages, "refusal")
        | (
            WireApi::Gemini,
            "SAFETY"
            | "RECITATION"
            | "LANGUAGE"
            | "BLOCKLIST"
            | "PROHIBITED_CONTENT"
            | "SPII"
            | "IMAGE_SAFETY"
            | "IMAGE_PROHIBITED_CONTENT"
            | "IMAGE_RECITATION"
            | "ESCALATION",
        ) => Ok(Finish::Filter),
        _ => Err(AdapterError::upstream_response_invalid()),
    }
}

pub(super) fn usage(protocol: WireApi, value: &Value) -> Usage {
    let data = value
        .get(if protocol == WireApi::Gemini {
            "usageMetadata"
        } else {
            "usage"
        })
        .unwrap_or(&Value::Null);
    let counter = |name: &str| data.get(name).and_then(Value::as_u64);
    let pointer = |name: &str| data.pointer(name).and_then(Value::as_u64);
    match protocol {
        WireApi::Responses => Usage {
            input: counter("input_tokens"),
            output: counter("output_tokens"),
            total: counter("total_tokens"),
            cached: pointer("/input_tokens_details/cached_tokens"),
            reasoning: pointer("/output_tokens_details/reasoning_tokens"),
            ..Usage::default()
        },
        WireApi::ChatCompletions => Usage {
            input: counter("prompt_tokens"),
            output: counter("completion_tokens"),
            total: counter("total_tokens"),
            cached: pointer("/prompt_tokens_details/cached_tokens"),
            reasoning: pointer("/completion_tokens_details/reasoning_tokens"),
            ..Usage::default()
        },
        WireApi::Messages => Usage {
            input: counter("input_tokens")
                .and_then(|input| {
                    input.checked_add(counter("cache_read_input_tokens").unwrap_or_default())
                })
                .and_then(|input| {
                    input.checked_add(counter("cache_creation_input_tokens").unwrap_or_default())
                }),
            output: counter("output_tokens"),
            cached: counter("cache_read_input_tokens"),
            cache_write: counter("cache_creation_input_tokens"),
            cache_write_5m: pointer("/cache_creation/ephemeral_5m_input_tokens"),
            cache_write_1h: pointer("/cache_creation/ephemeral_1h_input_tokens"),
            ..Usage::default()
        },
        WireApi::Gemini => Usage {
            input: counter("promptTokenCount"),
            output: counter("candidatesTokenCount").and_then(|output| {
                output.checked_add(counter("thoughtsTokenCount").unwrap_or_default())
            }),
            total: counter("totalTokenCount"),
            cached: counter("cachedContentTokenCount"),
            reasoning: counter("thoughtsTokenCount"),
            ..Usage::default()
        },
    }
}

pub(super) fn usage_value(protocol: WireApi, usage: &Usage) -> Value {
    let mut value = Map::new();
    let (input, output, total, cached, reasoning) = match protocol {
        WireApi::Responses => (
            "input_tokens",
            "output_tokens",
            "total_tokens",
            "input_tokens_details",
            "output_tokens_details",
        ),
        WireApi::ChatCompletions => (
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
            "prompt_tokens_details",
            "completion_tokens_details",
        ),
        WireApi::Messages => (
            "input_tokens",
            "output_tokens",
            "",
            "cache_read_input_tokens",
            "",
        ),
        WireApi::Gemini => (
            "promptTokenCount",
            "candidatesTokenCount",
            "totalTokenCount",
            "cachedContentTokenCount",
            "thoughtsTokenCount",
        ),
    };
    let input_count = if protocol == WireApi::Messages {
        usage
            .input
            .and_then(|count| count.checked_sub(usage.cached.unwrap_or_default()))
            .and_then(|count| count.checked_sub(usage.cache_write.unwrap_or_default()))
    } else {
        usage.input
    };
    if let Some(count) = input_count {
        value.insert(input.into(), count.into());
    }
    let output_count = if protocol == WireApi::Gemini {
        usage
            .output
            .and_then(|count| count.checked_sub(usage.reasoning.unwrap_or_default()))
    } else {
        usage.output
    };
    if let Some(count) = output_count {
        value.insert(output.into(), count.into());
    }
    if !total.is_empty() {
        if let Some(count) = usage.total {
            value.insert(total.into(), count.into());
        }
    }
    if let Some(count) = usage.cached {
        value.insert(
            cached.into(),
            if matches!(protocol, WireApi::Messages | WireApi::Gemini) {
                count.into()
            } else {
                json!({"cached_tokens":count})
            },
        );
    }
    if let Some(count) = usage.reasoning {
        if !reasoning.is_empty() {
            value.insert(
                reasoning.into(),
                if protocol == WireApi::Gemini {
                    count.into()
                } else {
                    json!({"reasoning_tokens":count})
                },
            );
        }
    }
    if protocol == WireApi::Messages {
        if let Some(count) = usage.cache_write {
            value.insert("cache_creation_input_tokens".into(), count.into());
        }
        let mut creation = Map::new();
        if let Some(count) = usage.cache_write_5m {
            creation.insert("ephemeral_5m_input_tokens".into(), count.into());
        }
        if let Some(count) = usage.cache_write_1h {
            creation.insert("ephemeral_1h_input_tokens".into(), count.into());
        }
        if !creation.is_empty() {
            value.insert("cache_creation".into(), creation.into());
        }
    }
    value.into()
}

pub(super) fn encode(protocol: WireApi, response: &Response, model: &str) -> AdapterResult<Value> {
    let usage = usage_value(protocol, &response.usage);
    let mut content = Vec::new();
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = Vec::new();
    for (index, block) in response.blocks.iter().enumerate() {
        match (protocol, block) {
            (WireApi::Responses, Block::Text(text)) => content.push(json!({"type":"message","id":format!("msg_{}_{index}",response.id),"role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]})),
            (WireApi::Responses, Block::ToolCall { id, name, arguments }) => content.push(json!({"type":"function_call","id":format!("fc_{}_{index}",response.id),"call_id":id,"name":name,"arguments":arguments,"status":"completed"})),
            (WireApi::Responses, Block::Reasoning(reasoning)) => content.push(json!({"type":"reasoning","id":format!("rs_{}_{index}",response.id),"summary":[{"type":"summary_text","text":reasoning}]})),
            (WireApi::ChatCompletions, Block::Text(value)) => text.push_str(value),
            (WireApi::ChatCompletions, Block::Reasoning(value)) => reasoning.push_str(value),
            (WireApi::ChatCompletions, Block::ToolCall { id, name, arguments }) => calls.push(json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments}})),
            (WireApi::Messages, Block::Text(text)) => content.push(json!({"type":"text","text":text})),
            (WireApi::Messages, Block::Reasoning(text)) => content.push(json!({"type":"thinking","thinking":text})),
            (WireApi::Messages, Block::ToolCall { id, name, arguments }) => content.push(json!({"type":"tool_use","id":id,"name":name,"input":serde_json::from_str::<Value>(arguments).map_err(|_| AdapterError::upstream_response_invalid())?})),
            (WireApi::Gemini, Block::Text(text)) => content.push(json!({"text":text})),
            (WireApi::Gemini, Block::Reasoning(text)) => content.push(json!({"text":text,"thought":true})),
            (WireApi::Gemini, Block::ToolCall { id, name, arguments }) => content.push(json!({"functionCall":{"id":id,"name":name,"args":serde_json::from_str::<Value>(arguments).map_err(|_| AdapterError::upstream_response_invalid())?}})),
            _ => return Err(AdapterError::upstream_response_invalid()),
        }
    }
    Ok(match protocol {
        WireApi::Responses => {
            let incomplete = matches!(response.finish, Finish::Length | Finish::Filter);
            json!({"id":response.id,"object":"response","model":model,"status":if incomplete { "incomplete" } else { "completed" },
                "output":content,"usage":usage,"incomplete_details":if incomplete { json!({"reason":if response.finish == Finish::Length { "max_output_tokens" } else { "content_filter" }}) } else { Value::Null }})
        }
        WireApi::ChatCompletions => {
            let mut message = json!({"role":"assistant","content":if text.is_empty() && (!calls.is_empty() || response.finish == Finish::Filter) { Value::Null } else { text.into() }});
            if !calls.is_empty() {
                message["tool_calls"] = calls.into();
            }
            if !reasoning.is_empty() {
                message["reasoning_content"] = reasoning.into();
            }
            json!({"id":response.id,"object":"chat.completion","created":0,"model":model,"choices":[{"index":0,"message":message,"finish_reason":finish_value(protocol,response.finish)}],"usage":usage})
        }
        WireApi::Messages => {
            json!({"id":response.id,"type":"message","role":"assistant","model":model,"content":content,"stop_reason":finish_value(protocol,response.finish),"stop_sequence":null,"usage":usage})
        }
        WireApi::Gemini => {
            json!({"responseId":response.id,"modelVersion":model,"candidates":[{"index":0,"content":{"role":"model","parts":content},"finishReason":finish_value(protocol,response.finish)}],"usageMetadata":usage})
        }
    })
}

pub(super) fn finish_value(protocol: WireApi, finish: Finish) -> &'static str {
    match (protocol, finish) {
        (WireApi::ChatCompletions, Finish::Stop) => "stop",
        (WireApi::ChatCompletions, Finish::Tools) => "tool_calls",
        (WireApi::ChatCompletions, Finish::Length) => "length",
        (WireApi::ChatCompletions, Finish::Filter) => "content_filter",
        (WireApi::Messages, Finish::Stop) => "end_turn",
        (WireApi::Messages, Finish::Tools) => "tool_use",
        (WireApi::Messages, Finish::Length) => "max_tokens",
        (WireApi::Messages, Finish::Filter) => "refusal",
        (WireApi::Gemini, Finish::Stop | Finish::Tools) => "STOP",
        (WireApi::Gemini, Finish::Length) => "MAX_TOKENS",
        (WireApi::Gemini, Finish::Filter) => "SAFETY",
        (WireApi::Responses, _) => "completed",
    }
}
