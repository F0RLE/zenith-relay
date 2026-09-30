use super::*;
use serde_json::{json, Map};

pub(super) fn request(
    request: &Request,
    protocol: WireApi,
    model: &str,
    stream: bool,
) -> AdapterResult<Value> {
    let mut body = Map::new();
    if protocol != WireApi::Gemini {
        body.insert("model".into(), model.into());
        body.insert("stream".into(), stream.into());
    }
    let (history, system) = conversation(request, protocol)?;
    body.insert(
        match protocol {
            WireApi::Responses => "input",
            WireApi::Gemini => "contents",
            _ => "messages",
        }
        .into(),
        history.into(),
    );
    if !system.is_empty() {
        match protocol {
            WireApi::Messages => {
                body.insert("system".into(), system.into());
            }
            WireApi::Gemini => {
                body.insert("systemInstruction".into(), json!({"parts": system}));
            }
            _ => {}
        }
    }
    let mut generation = Map::new();
    {
        let controls = if protocol == WireApi::Gemini {
            &mut generation
        } else {
            &mut body
        };
        write_sampling(controls, request, protocol)?;
    }
    write_tools(&mut body, request, protocol)?;
    if let Some(format) = &request.output_format {
        parts::output_format(&mut body, &mut generation, format, protocol)?;
    }
    write_reasoning(&mut body, &mut generation, request, protocol)?;
    if protocol == WireApi::Gemini && !generation.is_empty() {
        body.insert("generationConfig".into(), generation.into());
    }
    if protocol == WireApi::ChatCompletions && stream {
        body.insert("stream_options".into(), json!({"include_usage": true}));
    }
    if protocol == WireApi::Responses {
        body.insert("store".into(), false.into());
    }
    Ok(body.into())
}

fn conversation(request: &Request, protocol: WireApi) -> AdapterResult<(Vec<Value>, Vec<Value>)> {
    let mut history = Vec::new();
    let mut system = Vec::new();
    for message in request.instructions.iter().chain(&request.messages) {
        if message.role == Role::System && matches!(protocol, WireApi::Messages | WireApi::Gemini) {
            for block in &message.blocks {
                let Block::Text(text) = block else {
                    return Err(AdapterError::parameter_unsupported());
                };
                system.push(if protocol == WireApi::Messages {
                    json!({"type": "text", "text": text})
                } else {
                    json!({"text": text})
                });
            }
            continue;
        }
        history.extend(parts::message_value(message, protocol)?);
    }
    Ok((history, system))
}

fn write_sampling(
    controls: &mut Map<String, Value>,
    request: &Request,
    protocol: WireApi,
) -> AdapterResult<()> {
    if let Some(tokens) = request
        .max_tokens
        .or((protocol == WireApi::Messages).then_some(8192))
    {
        controls.insert(
            match protocol {
                WireApi::Responses => "max_output_tokens",
                WireApi::ChatCompletions => "max_completion_tokens",
                WireApi::Messages => "max_tokens",
                WireApi::Gemini => "maxOutputTokens",
            }
            .into(),
            tokens.into(),
        );
    }
    if let Some(value) = request.temperature {
        controls.insert("temperature".into(), value.into());
    }
    if let Some(value) = request.top_p {
        controls.insert(
            if protocol == WireApi::Gemini {
                "topP"
            } else {
                "top_p"
            }
            .into(),
            value.into(),
        );
    }
    if !request.stop.is_empty() {
        if protocol == WireApi::Responses {
            return Err(AdapterError::parameter_unsupported());
        }
        controls.insert(
            match protocol {
                WireApi::Messages => "stop_sequences",
                WireApi::Gemini => "stopSequences",
                _ => "stop",
            }
            .into(),
            json!(request.stop),
        );
    }
    Ok(())
}

fn write_tools(
    body: &mut Map<String, Value>,
    request: &Request,
    protocol: WireApi,
) -> AdapterResult<()> {
    if !request.tools.is_empty() {
        let declarations = request
            .tools
            .iter()
            .map(|tool| parts::function(tool, protocol))
            .collect::<AdapterResult<Vec<_>>>()?;
        body.insert(
            "tools".into(),
            if protocol == WireApi::Gemini {
                json!([{"functionDeclarations": declarations}])
            } else {
                declarations.into()
            },
        );
    }
    if let Some(choice) = &request.tool_choice {
        body.insert(
            if protocol == WireApi::Gemini {
                "toolConfig"
            } else {
                "tool_choice"
            }
            .into(),
            parts::tool_choice(choice, protocol),
        );
    }
    if let Some(parallel) = request.parallel_tools {
        match protocol {
            WireApi::Responses | WireApi::ChatCompletions => {
                body.insert("parallel_tool_calls".into(), parallel.into());
            }
            WireApi::Messages => {
                let choice = body
                    .entry("tool_choice")
                    .or_insert_with(|| json!({"type": "auto"}));
                choice["disable_parallel_tool_use"] = (!parallel).into();
            }
            WireApi::Gemini if !parallel => return Err(AdapterError::parameter_unsupported()),
            WireApi::Gemini => {}
        }
    }
    Ok(())
}

fn write_reasoning(
    body: &mut Map<String, Value>,
    generation: &mut Map<String, Value>,
    request: &Request,
    protocol: WireApi,
) -> AdapterResult<()> {
    let Some(reasoning) = &request.reasoning else {
        return Ok(());
    };
    if protocol == WireApi::Messages
        && !matches!(reasoning, Reasoning::Effort(effort) if effort == "none")
    {
        if request.temperature.is_some() || request.top_p.is_some() {
            return Err(AdapterError::parameter_unsupported());
        }
        if let Reasoning::Budget(budget) = reasoning {
            if request.max_tokens.is_some_and(|limit| limit <= *budget) {
                return Err(AdapterError::parameter_unsupported());
            }
            if request.max_tokens.is_none() {
                body.insert("max_tokens".into(), budget.saturating_add(1024).into());
            }
        }
    }
    parts::thinking(body, generation, reasoning, protocol)
}

mod parts;
