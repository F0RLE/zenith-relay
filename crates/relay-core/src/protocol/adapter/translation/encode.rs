use super::*;
use serde_json::{json, Map};

pub(super) fn request(
    request: &Request,
    protocol: WireApi,
    model: &str,
    stream: bool,
    native_turns: &BTreeMap<usize, Vec<Value>>,
    reasoning_mode: MessagesReasoningMode,
    capabilities: Option<&crate::model_metadata::ModelCapabilities>,
) -> AdapterResult<Value> {
    let mut request_fields = Map::new();
    options::write_metadata(&mut request_fields, &request.metadata, protocol)?;
    if protocol != WireApi::Gemini {
        request_fields.insert("model".into(), model.into());
        request_fields.insert("stream".into(), stream.into());
    }
    let (history, system) = conversation(request, protocol, native_turns)?;
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
        write_sampling(controls, request, protocol, capabilities)?;
    }
    write_tools(&mut request_fields, request, protocol)?;
    if let Some(format) = &request.output_format {
        parts::output_format(&mut request_fields, &mut generation, format, protocol)?;
    }
    write_reasoning(
        &mut request_fields,
        &mut generation,
        request,
        protocol,
        reasoning_mode,
        capabilities,
    )?;
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

fn conversation(
    request: &Request,
    protocol: WireApi,
    native_turns: &BTreeMap<usize, Vec<Value>>,
) -> AdapterResult<(Vec<Value>, Vec<Value>)> {
    let mut history: Vec<Value> = Vec::new();
    let mut system = Vec::new();
    let mut conversation_started = false;
    let mut gemini_call_ids = BTreeMap::new();
    for (position, message) in request
        .instructions
        .iter()
        .chain(&request.messages)
        .enumerate()
    {
        if message.role == Role::System && matches!(protocol, WireApi::Messages | WireApi::Gemini) {
            if conversation_started {
                // Hoisting a mid-conversation instruction into the initial
                // prompt changes which earlier turns it governs.
                return Err(AdapterError::parameter_unsupported_for(
                    match request.origin {
                        Some(WireApi::Responses) => "input[].role",
                        Some(WireApi::Gemini) => "contents[].role",
                        _ => "messages[].role",
                    },
                ));
            }
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
        conversation_started = true;
        let native = position
            .checked_sub(usize::from(request.instructions.is_some()))
            .and_then(|index| native_turns.get(&index));
        let items = if let Some(native) = native {
            if message.role != Role::Assistant {
                return Err(AdapterError::continuation_mismatch());
            }
            if protocol == WireApi::Gemini {
                let client_calls = message
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        Block::ToolCall { id, name, .. } => Some((id, name)),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let provider_calls = native
                    .iter()
                    .filter_map(|part| part.get("functionCall"))
                    .collect::<Vec<_>>();
                if client_calls.len() != provider_calls.len() {
                    return Err(AdapterError::continuation_mismatch());
                }
                for ((id, name), call) in client_calls.into_iter().zip(provider_calls) {
                    if call.get("name").and_then(Value::as_str) != Some(name) {
                        return Err(AdapterError::continuation_mismatch());
                    }
                    gemini_call_ids.insert(
                        id.clone(),
                        call.get("id").and_then(Value::as_str).map(str::to_owned),
                    );
                }
            }
            continuation::replay_items(protocol, native)
        } else {
            let mut visible = message.clone();
            if request.origin == Some(WireApi::Responses)
                && matches!(protocol, WireApi::Messages | WireApi::Gemini)
            {
                // A public summary is visible history, not a fabricated
                // signature. Original signed turns take the native branch.
                for block in &mut visible.blocks {
                    if let Block::Reasoning(text) = block {
                        *block = Block::Text(std::mem::take(text));
                    }
                }
            }
            let items = parts::message_value(&visible, protocol)?;
            if protocol == WireApi::Gemini {
                let mut items = Value::Array(items);
                super::super::gemini::restore_provider_call_ids(&mut items, &gemini_call_ids)?;
                items.as_array().expect("encoded history").clone()
            } else {
                items
            }
        };
        for item in items {
            if matches!(protocol, WireApi::Messages | WireApi::Gemini) {
                let content_key = if protocol == WireApi::Messages {
                    "content"
                } else {
                    "parts"
                };
                if let Some(previous_content) = history
                    .last_mut()
                    .filter(|previous| previous.get("role") == item.get("role"))
                    .and_then(|previous| previous.get_mut(content_key))
                    .and_then(Value::as_array_mut)
                {
                    previous_content.extend(
                        item.get(content_key)
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .cloned(),
                    );
                    continue;
                }
            }
            history.push(item);
        }
    }
    Ok((history, system))
}

fn write_sampling(
    controls: &mut Map<String, Value>,
    request: &Request,
    protocol: WireApi,
    capabilities: Option<&crate::model_metadata::ModelCapabilities>,
) -> AdapterResult<()> {
    let max_tokens = if protocol == WireApi::Messages {
        Some(super::super::messages::message_max_output_tokens(
            request.max_tokens,
            capabilities,
        )?)
    } else {
        request.max_tokens
    };
    if let Some(tokens) = max_tokens {
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
    if let Some(top_k) = request.top_k {
        let field = match protocol {
            WireApi::Messages => "top_k",
            WireApi::Gemini => "topK",
            _ => return Err(AdapterError::parameter_unsupported_for("top_k")),
        };
        controls.insert(field.into(), top_k.into());
    }
    for (field, gemini_field, value) in [
        (
            "presence_penalty",
            "presencePenalty",
            request.presence_penalty,
        ),
        (
            "frequency_penalty",
            "frequencyPenalty",
            request.frequency_penalty,
        ),
    ] {
        if let Some(value) = value {
            if !matches!(protocol, WireApi::ChatCompletions | WireApi::Gemini) {
                return Err(AdapterError::parameter_unsupported_for(field));
            }
            controls.insert(
                if protocol == WireApi::Gemini {
                    gemini_field
                } else {
                    field
                }
                .into(),
                value.into(),
            );
        }
    }
    if let Some(seed) = request.seed {
        if !matches!(protocol, WireApi::ChatCompletions | WireApi::Gemini) {
            return Err(AdapterError::parameter_unsupported_for("seed"));
        }
        controls.insert("seed".into(), seed.into());
    }
    Ok(())
}

fn write_tools(
    request_fields: &mut Map<String, Value>,
    request: &Request,
    protocol: WireApi,
) -> AdapterResult<()> {
    if request.tools.is_empty() {
        return if matches!(
            request.tool_choice,
            Some(ToolChoice::Required | ToolChoice::Function(_))
        ) {
            Err(AdapterError::parameter_unsupported_for("tool_choice"))
        } else {
            Ok(())
        };
    }
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
            WireApi::Gemini => {
                // Gemini has no native toggle. JSON and stream translation
                // enforce the client's limit before forwarding tool calls.
            }
        }
    }
    Ok(())
}

fn write_reasoning(
    request_fields: &mut Map<String, Value>,
    generation: &mut Map<String, Value>,
    request: &Request,
    protocol: WireApi,
    mode: MessagesReasoningMode,
    capabilities: Option<&crate::model_metadata::ModelCapabilities>,
) -> AdapterResult<()> {
    let Some(reasoning) = &request.reasoning else {
        return Ok(());
    };
    parts::thinking(
        request_fields,
        generation,
        reasoning,
        protocol,
        mode,
        request.max_tokens.is_some(),
        capabilities,
    )
}

mod parts;
