use super::*;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

mod upstream;

pub(super) use upstream::decode;

pub(super) fn validate_responses_call_execution(call: &Value) -> AdapterResult<()> {
    validate_direct_tool_execution(
        call,
        AdapterError::upstream_response_invalid().with_parameter("tool_calls.async"),
        AdapterError::upstream_response_invalid().with_parameter("tool_calls.caller"),
    )?;
    // Converted requests declare flat tools. A namespace returned by the
    // upstream would address a different tool, not our flattened declaration.
    if call.get("namespace").is_some_and(|value| !value.is_null()) {
        return Err(
            AdapterError::upstream_response_invalid().with_parameter("tool_calls.namespace")
        );
    }
    Ok(())
}

pub(super) fn validate_calls(blocks: &[Block], finish: Finish) -> AdapterResult<()> {
    let mut call_ids = std::collections::BTreeSet::new();
    for block in blocks {
        if let Block::ToolCall {
            id,
            name,
            arguments,
        } = block
        {
            if id.trim().is_empty() || !call_ids.insert(id) {
                return Err(
                    AdapterError::upstream_response_invalid().with_parameter("tool_calls.id")
                );
            }
            if name.is_empty() {
                return Err(AdapterError::upstream_response_invalid()
                    .with_parameter("tool_calls.function.name"));
            }
            match serde_json::from_str::<Value>(arguments) {
                Ok(args) if args.is_object() => {}
                Err(error)
                    if finish == Finish::Length
                        && error.is_eof()
                        && (arguments.trim().is_empty()
                            || arguments.trim_start().starts_with('{')) => {}
                _ => {
                    return Err(AdapterError::upstream_response_invalid()
                        .with_parameter("tool_calls.function.arguments"));
                }
            }
        }
    }
    Ok(())
}

pub(super) fn finish(protocol: WireApi, finish_reason: &str) -> AdapterResult<Finish> {
    match (protocol, finish_reason) {
        (WireApi::ChatCompletions, "stop")
        | (WireApi::Messages, "end_turn" | "stop_sequence")
        | (WireApi::Gemini, "STOP") => Ok(Finish::Stop),
        (WireApi::ChatCompletions, "tool_calls") | (WireApi::Messages, "tool_use") => {
            Ok(Finish::Tools)
        }
        (WireApi::ChatCompletions, "length")
        | (WireApi::Messages, "max_tokens" | "model_context_window_exceeded")
        | (WireApi::Gemini, "MAX_TOKENS") => Ok(Finish::Length),
        (WireApi::Messages, "pause_turn") => {
            Err(AdapterError::upstream_response_invalid().with_parameter("stop_reason"))
        }
        (WireApi::Messages, "refusal") => Ok(Finish::Refusal),
        (WireApi::ChatCompletions, "content_filter")
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

pub(super) fn usage(protocol: WireApi, response_payload: &Value) -> Usage {
    let usage_payload = response_payload
        .get(if protocol == WireApi::Gemini {
            "usageMetadata"
        } else {
            "usage"
        })
        .unwrap_or(&Value::Null);
    let counter = |field_name: &str| usage_payload.get(field_name).and_then(Value::as_u64);
    let pointer = |json_pointer: &str| usage_payload.pointer(json_pointer).and_then(Value::as_u64);
    match protocol {
        WireApi::Responses => Usage {
            input: counter("input_tokens"),
            output: counter("output_tokens"),
            total: counter("total_tokens"),
            cached: pointer("/input_tokens_details/cached_tokens"),
            reasoning: pointer("/output_tokens_details/reasoning_tokens"),
            cache_write: pointer("/input_tokens_details/cache_write_tokens"),
            cache_write_5m: pointer(
                "/input_tokens_details/cache_creation/ephemeral_5m_input_tokens",
            ),
            cache_write_1h: pointer(
                "/input_tokens_details/cache_creation/ephemeral_1h_input_tokens",
            ),
        },
        WireApi::ChatCompletions => Usage {
            input: counter("prompt_tokens"),
            output: counter("completion_tokens"),
            total: counter("total_tokens"),
            cached: pointer("/prompt_tokens_details/cached_tokens")
                .or_else(|| counter("prompt_cache_hit_tokens")),
            reasoning: pointer("/completion_tokens_details/reasoning_tokens"),
            cache_write: pointer("/prompt_tokens_details/cache_write_tokens"),
            cache_write_5m: pointer(
                "/prompt_tokens_details/cache_creation/ephemeral_5m_input_tokens",
            ),
            cache_write_1h: pointer(
                "/prompt_tokens_details/cache_creation/ephemeral_1h_input_tokens",
            ),
        },
        WireApi::Messages => Usage {
            input: counter("input_tokens")
                .and_then(|input_tokens| {
                    input_tokens.checked_add(counter("cache_read_input_tokens").unwrap_or_default())
                })
                .and_then(|input_tokens| {
                    input_tokens
                        .checked_add(counter("cache_creation_input_tokens").unwrap_or_default())
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
            output: counter("candidatesTokenCount").and_then(|output_tokens| {
                output_tokens.checked_add(counter("thoughtsTokenCount").unwrap_or_default())
            }),
            total: counter("totalTokenCount"),
            cached: counter("cachedContentTokenCount"),
            reasoning: counter("thoughtsTokenCount"),
            ..Usage::default()
        },
    }
}

pub(super) fn usage_value(protocol: WireApi, usage: &Usage) -> Value {
    let mut usage_payload = Map::new();
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
        usage_payload.insert(input.into(), count.into());
    }
    let output_count = if protocol == WireApi::Gemini {
        usage
            .output
            .and_then(|count| count.checked_sub(usage.reasoning.unwrap_or_default()))
    } else {
        usage.output
    };
    if let Some(count) = output_count {
        usage_payload.insert(output.into(), count.into());
    }
    if !total.is_empty() {
        if let Some(count) = usage.total {
            usage_payload.insert(total.into(), count.into());
        }
    }
    if let Some(count) = usage.cached {
        usage_payload.insert(
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
            usage_payload.insert(
                reasoning.into(),
                if protocol == WireApi::Gemini {
                    count.into()
                } else {
                    json!({"reasoning_tokens":count})
                },
            );
        }
    }
    if matches!(
        protocol,
        WireApi::Messages | WireApi::ChatCompletions | WireApi::Responses
    ) {
        let mut details = if protocol == WireApi::Messages {
            Map::new()
        } else {
            usage_payload
                .remove(cached)
                .and_then(|value| value.as_object().cloned())
                .unwrap_or_default()
        };
        if let Some(count) = usage.cache_write {
            details.insert(
                if protocol == WireApi::Messages {
                    "cache_creation_input_tokens"
                } else {
                    "cache_write_tokens"
                }
                .into(),
                count.into(),
            );
        }
        let mut creation = Map::new();
        if let Some(count) = usage.cache_write_5m {
            creation.insert("ephemeral_5m_input_tokens".into(), count.into());
        }
        if let Some(count) = usage.cache_write_1h {
            creation.insert("ephemeral_1h_input_tokens".into(), count.into());
        }
        if !creation.is_empty() {
            details.insert("cache_creation".into(), creation.into());
        }
        if protocol == WireApi::Messages {
            usage_payload.extend(details);
        } else if !details.is_empty() {
            usage_payload.insert(cached.into(), details.into());
        }
    }
    usage_payload.into()
}

pub(super) fn encode(
    protocol: WireApi,
    response: &Response,
    model: &str,
    client_tools: &BTreeMap<String, ClientToolTarget>,
) -> AdapterResult<Value> {
    if !response.citations.is_empty()
        && !matches!(protocol, WireApi::ChatCompletions | WireApi::Responses)
    {
        return Err(AdapterError::upstream_response_invalid());
    }
    let mut usage = usage_value(protocol, &response.usage);
    if protocol == WireApi::Responses {
        // A Responses client needs input, output and total together or none.
        usage = match usage {
            Value::Object(counters) => super::super::responses_usage::complete(counters, None),
            other => other,
        };
    }
    let mut content = Vec::new();
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut refusal = String::new();
    let mut calls = Vec::new();
    for (index, block) in response.blocks.iter().enumerate() {
        match (protocol, block) {
            (WireApi::Responses, Block::Text(text)) => content.push(json!({"type":"message","id":format!("msg_{}_{index}",response.id),"role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":response.citations.get(&index).map(|items| items.iter().map(citations::UrlCitation::responses_value).collect::<Vec<_>>()).unwrap_or_default()}]})),
            (WireApi::Responses, Block::Refusal(text)) => content.push(json!({"type":"message","id":format!("msg_{}_{index}",response.id),"role":"assistant","status":"completed","content":[{"type":"refusal","refusal":text}]})),
            (WireApi::Responses, Block::ToolCall { id, name, arguments }) => {
                let target = client_tools.get(name);
                let client_name = target.map_or(name.as_str(), |target| target.name.as_str());
                let mut item = if target.is_some_and(|target| target.kind == ResponsesToolKind::ToolSearch) {
                    json!({"type":"tool_search_call","id":format!("ts_{}_{index}",response.id),"call_id":id,
                        "execution":"client","arguments":serde_json::from_str::<Value>(arguments).map_err(|_| AdapterError::upstream_response_invalid())?,"status":"completed"})
                } else if target.is_some_and(|target| target.kind == ResponsesToolKind::Custom) {
                    json!({
                        "type": "custom_tool_call",
                        "id": super::super::contracts::custom_tool_item_id(id),
                        "call_id": id,
                        "name": client_name,
                        "input": custom_tool_input(arguments)?,
                        "status": "completed"
                    })
                } else {
                    json!({"type":"function_call","id":format!("fc_{}_{index}",response.id),"call_id":id,"name":client_name,"arguments":arguments,"status":if serde_json::from_str::<Value>(arguments).is_err() { "incomplete" } else { "completed" }})
                };
                if let Some(namespace) = target.and_then(|target| target.namespace.as_deref()) {
                    item["namespace"] = namespace.into();
                }
                content.push(item);
            }
            (WireApi::Responses, Block::Reasoning(reasoning)) => content.push(json!({"type":"reasoning","id":format!("rs_{}_{index}",response.id),"summary":[{"type":"summary_text","text":reasoning}]})),
            (WireApi::ChatCompletions, Block::Text(text_value)) => text.push_str(text_value),
            (WireApi::ChatCompletions, Block::Reasoning(reasoning_value)) => {
                reasoning.push_str(reasoning_value)
            }
            (WireApi::ChatCompletions, Block::Refusal(text)) => refusal.push_str(text),
            (WireApi::ChatCompletions, Block::ToolCall { id, name, arguments }) => calls.push(json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments}})),
            (WireApi::Messages, Block::Text(text)) => content.push(json!({"type":"text","text":text})),
            (WireApi::Messages, Block::Refusal(text)) => content.push(json!({"type":"text","text":text})),
            // This is a public summary for the client, not signed Anthropic
            // state. Original upstream blocks stay in the scoped continuation.
            (WireApi::Messages, Block::Reasoning(text)) => content.push(json!({"type":"thinking","thinking":text})),
            (WireApi::Messages, Block::ToolCall { id, name, arguments }) => content.push(json!({"type":"tool_use","id":id,"name":name,"input":serde_json::from_str::<Value>(arguments).map_err(|_| AdapterError::upstream_response_invalid())?})),
            (WireApi::Gemini, Block::Text(text)) => content.push(json!({"text":text})),
            (WireApi::Gemini, Block::Refusal(text)) => content.push(json!({"text":text})),
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
            if response.finish == Finish::Refusal || !refusal.is_empty() {
                message["refusal"] = refusal.into();
            }
            let annotations = citations::chat_annotations(response)?;
            if !annotations.is_empty() {
                message["annotations"] = annotations.into();
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

fn custom_tool_input(arguments: &str) -> AdapterResult<String> {
    let input: Value =
        serde_json::from_str(arguments).map_err(|_| AdapterError::upstream_response_invalid())?;
    super::super::messages::custom_tool_input(&input).map(str::to_owned)
}

pub(super) fn finish_value(protocol: WireApi, finish: Finish) -> &'static str {
    match (protocol, finish) {
        (WireApi::ChatCompletions, Finish::Stop | Finish::Refusal) => "stop",
        (WireApi::ChatCompletions, Finish::Tools) => "tool_calls",
        (WireApi::ChatCompletions, Finish::Length) => "length",
        (WireApi::ChatCompletions, Finish::Filter) => "content_filter",
        (WireApi::Messages, Finish::Stop) => "end_turn",
        (WireApi::Messages, Finish::Tools) => "tool_use",
        (WireApi::Messages, Finish::Length) => "max_tokens",
        (WireApi::Messages, Finish::Filter | Finish::Refusal) => "refusal",
        (WireApi::Gemini, Finish::Stop | Finish::Tools) => "STOP",
        (WireApi::Gemini, Finish::Length) => "MAX_TOKENS",
        (WireApi::Gemini, Finish::Filter | Finish::Refusal) => "SAFETY",
        (WireApi::Responses, _) => "completed",
    }
}
