use super::*;
use serde_json::{json, Map};

pub(super) fn request(
    request: &Request,
    protocol: WireApi,
    model: &str,
    stream: bool,
) -> AdapterResult<Value> {
    let mut request_fields = Map::new();
    if protocol != WireApi::Gemini {
        request_fields.insert("model".into(), model.into());
        request_fields.insert("stream".into(), stream.into());
    }
    let (history, system) = conversation(request, protocol)?;
    request_fields.insert(
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
                request_fields.insert("system".into(), system.into());
            }
            WireApi::Gemini => {
                request_fields.insert("systemInstruction".into(), json!({"parts": system}));
            }
            _ => {}
        }
    }
    let mut generation = Map::new();
    {
        let controls = if protocol == WireApi::Gemini {
            &mut generation
        } else {
            &mut request_fields
        };
        write_sampling(controls, request, protocol)?;
    }
    write_tools(&mut request_fields, request, protocol)?;
    if let Some(format) = &request.output_format {
        parts::output_format(&mut request_fields, &mut generation, format, protocol)?;
    }
    write_reasoning(&mut request_fields, &mut generation, request, protocol)?;
    if protocol == WireApi::Gemini && !generation.is_empty() {
        request_fields.insert("generationConfig".into(), generation.into());
    }
    if protocol == WireApi::ChatCompletions && stream {
        request_fields.insert("stream_options".into(), json!({"include_usage": true}));
    }
    if protocol == WireApi::Responses {
        request_fields.insert("store".into(), false.into());
    }
    Ok(request_fields.into())
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
    if let Some(temperature_value) = request.temperature {
        controls.insert("temperature".into(), temperature_value.into());
    }
    if let Some(top_p_value) = request.top_p {
        controls.insert(
            if protocol == WireApi::Gemini {
                "topP"
            } else {
                "top_p"
            }
            .into(),
            top_p_value.into(),
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
    request_fields: &mut Map<String, Value>,
    request: &Request,
    protocol: WireApi,
) -> AdapterResult<()> {
    if !request.tools.is_empty() {
        let declarations = request
            .tools
            .iter()
            .map(|tool| parts::function(tool, protocol))
            .collect::<AdapterResult<Vec<_>>>()?;
        request_fields.insert(
            "tools".into(),
            if protocol == WireApi::Gemini {
                json!([{"functionDeclarations": declarations}])
            } else {
                declarations.into()
            },
        );
    }
    if let Some(choice) = &request.tool_choice {
        request_fields.insert(
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
                request_fields.insert("parallel_tool_calls".into(), parallel.into());
            }
            WireApi::Messages => {
                let choice = request_fields
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
    request_fields: &mut Map<String, Value>,
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
                request_fields.insert("max_tokens".into(), budget.saturating_add(1024).into());
            }
        }
    }
    parts::thinking(request_fields, generation, reasoning, protocol)
}

mod parts;
