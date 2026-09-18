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
    let mut history = Vec::new();
    let mut system = Vec::new();
    for message in request.instructions.iter().chain(&request.messages) {
        if message.role == Role::System && matches!(protocol, WireApi::Messages | WireApi::Gemini) {
            for block in &message.blocks {
                let Block::Text(text) = block else {
                    return Err(AdapterError::parameter_unsupported());
                };
                system.push(if protocol == WireApi::Messages {
                    json!({"type":"text","text":text})
                } else {
                    json!({"text":text})
                });
            }
            continue;
        }
        history.extend(message_value(message, protocol)?);
    }
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
                body.insert("systemInstruction".into(), json!({"parts":system}));
            }
            _ => {}
        }
    }
    let mut generation = Map::new();
    let controls = if protocol == WireApi::Gemini {
        &mut generation
    } else {
        &mut body
    };
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
    if !request.tools.is_empty() {
        let declarations = request
            .tools
            .iter()
            .map(|tool| function(tool, protocol))
            .collect::<AdapterResult<Vec<_>>>()?;
        body.insert(
            "tools".into(),
            if protocol == WireApi::Gemini {
                json!([{"functionDeclarations":declarations}])
            } else {
                declarations.into()
            },
        );
    }
    if let Some(choice) = &request.tool_choice {
        let value = tool_choice(choice, protocol);
        body.insert(
            if protocol == WireApi::Gemini {
                "toolConfig"
            } else {
                "tool_choice"
            }
            .into(),
            value,
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
                    .or_insert_with(|| json!({"type":"auto"}));
                choice["disable_parallel_tool_use"] = (!parallel).into();
            }
            WireApi::Gemini if !parallel => return Err(AdapterError::parameter_unsupported()),
            WireApi::Gemini => {}
        }
    }
    if let Some(format) = &request.output_format {
        output_format(&mut body, &mut generation, format, protocol)?;
    }
    if let Some(reasoning) = &request.reasoning {
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
        thinking(&mut body, &mut generation, reasoning, protocol)?;
    }
    if protocol == WireApi::Gemini && !generation.is_empty() {
        body.insert("generationConfig".into(), generation.into());
    }
    if protocol == WireApi::ChatCompletions && stream {
        body.insert("stream_options".into(), json!({"include_usage":true}));
    }
    if protocol == WireApi::Responses {
        body.insert("store".into(), false.into());
    }
    Ok(body.into())
}

fn function(tool: &Function, protocol: WireApi) -> AdapterResult<Value> {
    let mut definition = json!({"name":tool.name});
    if let Some(description) = &tool.description {
        definition["description"] = description.clone().into();
    }
    definition[match protocol {
        WireApi::Messages => "input_schema",
        WireApi::Gemini => "parametersJsonSchema",
        _ => "parameters",
    }] = tool.parameters.clone();
    if let Some(strict) = tool.strict {
        if protocol == WireApi::Gemini && strict {
            return Err(AdapterError::parameter_unsupported());
        }
        if protocol != WireApi::Gemini {
            definition["strict"] = strict.into();
        }
    }
    Ok(match protocol {
        WireApi::ChatCompletions => json!({"type":"function","function":definition}),
        WireApi::Responses => {
            definition["type"] = "function".into();
            definition
        }
        _ => definition,
    })
}

fn tool_choice(choice: &ToolChoice, protocol: WireApi) -> Value {
    match protocol {
        WireApi::Responses | WireApi::ChatCompletions => match choice {
            ToolChoice::Auto => "auto".into(),
            ToolChoice::None => "none".into(),
            ToolChoice::Required => "required".into(),
            ToolChoice::Function(name) if protocol == WireApi::Responses => {
                json!({"type":"function","name":name})
            }
            ToolChoice::Function(name) => json!({"type":"function","function":{"name":name}}),
        },
        WireApi::Messages => match choice {
            ToolChoice::Auto => json!({"type":"auto"}),
            ToolChoice::None => json!({"type":"none"}),
            ToolChoice::Required => json!({"type":"any"}),
            ToolChoice::Function(name) => json!({"type":"tool","name":name}),
        },
        WireApi::Gemini => json!({"functionCallingConfig":match choice {
            ToolChoice::Auto => json!({"mode":"AUTO"}), ToolChoice::None => json!({"mode":"NONE"}), ToolChoice::Required => json!({"mode":"ANY"}),
            ToolChoice::Function(name) => json!({"mode":"ANY","allowedFunctionNames":[name]}),
        }}),
    }
}

fn output_format(
    body: &mut Map<String, Value>,
    generation: &mut Map<String, Value>,
    format: &OutputFormat,
    protocol: WireApi,
) -> AdapterResult<()> {
    let mut value = match format {
        OutputFormat::JsonObject => json!({"type":"json_object"}),
        OutputFormat::JsonSchema {
            name,
            schema,
            strict,
        } => {
            let mut value = json!({"type":"json_schema","name":name,"schema":schema});
            if let Some(strict) = strict {
                value["strict"] = (*strict).into();
            }
            value
        }
    };
    match protocol {
        WireApi::Responses => {
            body.insert("text".into(), json!({"format":value}));
        }
        WireApi::ChatCompletions => {
            if matches!(format, OutputFormat::JsonSchema { .. }) {
                value.as_object_mut().unwrap().remove("type");
                value = json!({"type":"json_schema","json_schema":value});
            }
            body.insert("response_format".into(), value);
        }
        WireApi::Messages => {
            let OutputFormat::JsonSchema { schema, .. } = format else {
                return Err(AdapterError::parameter_unsupported());
            };
            body.insert(
                "output_config".into(),
                json!({"format":{"type":"json_schema","schema":schema}}),
            );
        }
        WireApi::Gemini => {
            if matches!(
                format,
                OutputFormat::JsonSchema {
                    strict: Some(true),
                    ..
                }
            ) {
                return Err(AdapterError::parameter_unsupported());
            }
            generation.insert("responseMimeType".into(), "application/json".into());
            if let OutputFormat::JsonSchema { schema, .. } = format {
                generation.insert("responseJsonSchema".into(), schema.clone());
            }
        }
    }
    Ok(())
}

fn thinking(
    body: &mut Map<String, Value>,
    generation: &mut Map<String, Value>,
    reasoning: &Reasoning,
    protocol: WireApi,
) -> AdapterResult<()> {
    match (protocol, reasoning) {
        (WireApi::Responses, Reasoning::Effort(effort)) => {
            body.insert("reasoning".into(), json!({"effort":effort}));
        }
        (WireApi::ChatCompletions, Reasoning::Effort(effort)) => {
            body.insert("reasoning_effort".into(), effort.clone().into());
        }
        (WireApi::Gemini, Reasoning::Effort(effort))
            if matches!(effort.as_str(), "minimal" | "low" | "medium" | "high") =>
        {
            generation.insert("thinkingConfig".into(), json!({"thinkingLevel":effort}));
        }
        (WireApi::Gemini, Reasoning::Budget(budget)) => {
            generation.insert("thinkingConfig".into(), json!({"thinkingBudget":budget}));
        }
        (WireApi::Messages, Reasoning::Budget(budget)) if *budget >= 1024 => {
            body.insert(
                "thinking".into(),
                json!({"type":"enabled","budget_tokens":budget}),
            );
        }
        (WireApi::Messages, Reasoning::Effort(effort)) if effort == "none" => {
            body.insert("thinking".into(), json!({"type":"disabled"}));
        }
        (WireApi::Messages, Reasoning::Effort(effort))
            if matches!(effort.as_str(), "low" | "medium" | "high" | "max") =>
        {
            body.insert("thinking".into(), json!({"type":"adaptive"}));
            body.entry("output_config").or_insert_with(|| json!({}))["effort"] =
                effort.clone().into();
        }
        _ => return Err(AdapterError::reasoning_unsupported()),
    }
    Ok(())
}

fn message_value(message: &Message, protocol: WireApi) -> AdapterResult<Vec<Value>> {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    };
    let mut values = Vec::new();
    let mut content = Vec::new();
    let mut calls = Vec::new();
    for block in &message.blocks {
        match (protocol, block) {
            (
                WireApi::Responses,
                Block::ToolCall {
                    id,
                    name,
                    arguments,
                },
            ) => {
                flush_responses_content(&mut values, &mut content, role);
                values.push(
                    json!({"type":"function_call","call_id":id,"name":name,"arguments":arguments}),
                );
            }
            (
                WireApi::Responses,
                Block::ToolResult {
                    id,
                    content: output,
                    is_error,
                    ..
                },
            ) => {
                flush_responses_content(&mut values, &mut content, role);
                values.push(json!({"type":"function_call_output","call_id":id,"output":tool_output(output, *is_error)}));
            }
            (
                WireApi::ChatCompletions,
                Block::ToolCall {
                    id,
                    name,
                    arguments,
                },
            ) => calls.push(
                json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments}}),
            ),
            (
                WireApi::ChatCompletions,
                Block::ToolResult {
                    id,
                    content: output,
                    is_error,
                    ..
                },
            ) => {
                if !content.is_empty() || !calls.is_empty() {
                    return Err(AdapterError::parameter_unsupported());
                }
                values.push(json!({"role":"tool","tool_call_id":id,"content":tool_output(output, *is_error)}));
            }
            _ => content.push(content_block(block, protocol, message.role)?),
        }
    }
    match protocol {
        WireApi::Responses => flush_responses_content(&mut values, &mut content, role),
        WireApi::ChatCompletions if !content.is_empty() || !calls.is_empty() => {
            let mut value = json!({"role":role,"content":if content.is_empty() { Value::Null } else { content.into() }});
            if !calls.is_empty() { value["tool_calls"] = calls.into(); }
            values.push(value);
        }
        WireApi::Messages => values.push(json!({"role":role,"content":content})),
        WireApi::Gemini => values.push(json!({"role":if message.role == Role::Assistant { "model" } else { "user" },"parts":content})),
        _ => {}
    }
    Ok(values)
}

fn tool_output(content: &str, is_error: bool) -> String {
    if is_error {
        json!({"error":content}).to_string()
    } else {
        content.into()
    }
}

fn flush_responses_content(values: &mut Vec<Value>, content: &mut Vec<Value>, role: &str) {
    if !content.is_empty() {
        values.push(json!({"role":role,"content":std::mem::take(content)}));
    }
}

fn content_block(block: &Block, protocol: WireApi, role: Role) -> AdapterResult<Value> {
    Ok(match block {
        Block::Text(text) => match protocol {
            WireApi::Responses => {
                json!({"type":if role == Role::Assistant { "output_text" } else { "input_text" },"text":text})
            }
            WireApi::Gemini => json!({"text":text}),
            _ => json!({"type":"text","text":text}),
        },
        Block::Image { url, detail } => match protocol {
            WireApi::Responses => {
                let mut value = json!({"type":"input_image","image_url":url});
                if let Some(detail) = detail {
                    value["detail"] = detail.clone().into();
                }
                value
            }
            WireApi::ChatCompletions => {
                let mut value = json!({"type":"image_url","image_url":{"url":url}});
                if let Some(detail) = detail {
                    value["image_url"]["detail"] = detail.clone().into();
                }
                value
            }
            WireApi::Messages | WireApi::Gemini => {
                if detail.as_deref().is_some_and(|detail| detail != "auto") {
                    return Err(AdapterError::parameter_unsupported());
                }
                if let Some((mime, data)) = url
                    .strip_prefix("data:")
                    .and_then(|data| data.split_once(";base64,"))
                {
                    if protocol == WireApi::Messages {
                        json!({"type":"image","source":{"type":"base64","media_type":mime,"data":data}})
                    } else {
                        json!({"inlineData":{"mimeType":mime,"data":data}})
                    }
                } else if protocol == WireApi::Messages {
                    json!({"type":"image","source":{"type":"url","url":url}})
                } else {
                    return Err(AdapterError::parameter_unsupported());
                }
            }
        },
        Block::ToolCall {
            id,
            name,
            arguments,
        } => {
            let input: Value =
                serde_json::from_str(arguments).map_err(|_| AdapterError::invalid_request())?;
            if !input.is_object() {
                return Err(AdapterError::invalid_request());
            }
            match protocol {
                WireApi::Messages => json!({"type":"tool_use","id":id,"name":name,"input":input}),
                WireApi::Gemini => json!({"functionCall":{"id":id,"name":name,"args":input}}),
                _ => return Err(AdapterError::unsupported_binding()),
            }
        }
        Block::ToolResult {
            id,
            name,
            content,
            is_error,
        } => match protocol {
            WireApi::Messages => {
                json!({"type":"tool_result","tool_use_id":id,"content":content,"is_error":is_error})
            }
            WireApi::Gemini => {
                let response = serde_json::from_str::<Value>(content)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| {
                        if *is_error {
                            json!({"error":content})
                        } else {
                            json!({"result":content})
                        }
                    });
                json!({"functionResponse":{"id":id,"name":name,"response":response}})
            }
            _ => return Err(AdapterError::unsupported_binding()),
        },
        Block::Reasoning(_) => return Err(AdapterError::reasoning_unsupported()),
    })
}
