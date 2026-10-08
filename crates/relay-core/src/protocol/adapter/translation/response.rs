use super::*;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

mod upstream;

pub(super) use upstream::decode;

pub(super) fn validate_calls(blocks: &[Block]) -> AdapterResult<()> {
    let mut call_ids = std::collections::BTreeSet::new();
    for block in blocks {
        if let Block::ToolCall {
            id,
            name,
            arguments,
        } = block
        {
            if id.is_empty()
                || name.is_empty()
                || !call_ids.insert(id)
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

pub(super) fn finish(protocol: WireApi, finish_reason: &str) -> AdapterResult<Finish> {
    match (protocol, finish_reason) {
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
    if protocol == WireApi::Messages {
        if let Some(count) = usage.cache_write {
            usage_payload.insert("cache_creation_input_tokens".into(), count.into());
        }
        let mut creation = Map::new();
        if let Some(count) = usage.cache_write_5m {
            creation.insert("ephemeral_5m_input_tokens".into(), count.into());
        }
        if let Some(count) = usage.cache_write_1h {
            creation.insert("ephemeral_1h_input_tokens".into(), count.into());
        }
        if !creation.is_empty() {
            usage_payload.insert("cache_creation".into(), creation.into());
        }
    }
    usage_payload.into()
}

pub(super) fn encode(
    protocol: WireApi,
    response: &Response,
    model: &str,
    custom_tools: &BTreeSet<String>,
) -> AdapterResult<Value> {
    let usage = usage_value(protocol, &response.usage);
    let mut content = Vec::new();
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = Vec::new();
    for (index, block) in response.blocks.iter().enumerate() {
        match (protocol, block) {
            (WireApi::Responses, Block::Text(text)) => content.push(json!({"type":"message","id":format!("msg_{}_{index}",response.id),"role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]})),
            (WireApi::Responses, Block::ToolCall { id, name, arguments }) if custom_tools.contains(name) => {
                content.push(json!({
                    "type": "custom_tool_call",
                    "id": super::super::contracts::custom_tool_item_id(id),
                    "call_id": id,
                    "name": name,
                    "input": custom_tool_input(arguments)?,
                    "status": "completed"
                }));
            }
            (WireApi::Responses, Block::ToolCall { id, name, arguments }) => content.push(json!({"type":"function_call","id":format!("fc_{}_{index}",response.id),"call_id":id,"name":name,"arguments":arguments,"status":"completed"})),
            (WireApi::Responses, Block::Reasoning(reasoning)) => content.push(json!({"type":"reasoning","id":format!("rs_{}_{index}",response.id),"summary":[{"type":"summary_text","text":reasoning}]})),
            (WireApi::ChatCompletions, Block::Text(text_value)) => text.push_str(text_value),
            (WireApi::ChatCompletions, Block::Reasoning(reasoning_value)) => {
                reasoning.push_str(reasoning_value)
            }
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

fn custom_tool_input(arguments: &str) -> AdapterResult<String> {
    if let Ok(parsed_arguments) = serde_json::from_str::<Value>(arguments) {
        if let Some(input) = parsed_arguments.get("input").and_then(Value::as_str) {
            return Ok(input.to_string());
        }
        if parsed_arguments.is_object() {
            return Err(AdapterError::upstream_response_invalid());
        }
    }
    if arguments.is_empty() {
        return Err(AdapterError::upstream_response_invalid());
    }
    Ok(arguments.to_string())
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
