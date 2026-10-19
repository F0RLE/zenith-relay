use super::*;

pub(super) fn function(tool: &Function, protocol: WireApi) -> AdapterResult<Value> {
    let mut definition = json!({"name":tool.name});
    if let Some(description) = &tool.description {
        definition["description"] = description.clone().into();
    }
    definition[match protocol {
        WireApi::Messages => "input_schema",
        WireApi::Gemini => "parametersJsonSchema",
        _ => "parameters",
    }] = if protocol == WireApi::Gemini {
        super::super::schema::to_gemini(&tool.parameters)?
    } else {
        tool.parameters.clone()
    };
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

pub(super) fn tool_choice(choice: &ToolChoice, protocol: WireApi) -> Value {
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

pub(super) fn output_format(
    request_fields: &mut Map<String, Value>,
    generation: &mut Map<String, Value>,
    format: &OutputFormat,
    protocol: WireApi,
) -> AdapterResult<()> {
    match protocol {
        WireApi::Responses | WireApi::ChatCompletions => {
            let mut format_value = match format {
                OutputFormat::JsonObject => json!({"type":"json_object"}),
                OutputFormat::JsonSchema {
                    name,
                    description,
                    schema,
                    strict,
                } => {
                    let mut schema_value =
                        json!({"type":"json_schema","name":name,"schema":schema});
                    if let Some(description) = description {
                        schema_value["description"] = description.clone().into();
                    }
                    if let Some(strict) = strict {
                        schema_value["strict"] = (*strict).into();
                    }
                    schema_value
                }
            };
            if protocol == WireApi::Responses {
                request_fields.insert("text".into(), json!({"format":format_value}));
            } else {
                if matches!(format, OutputFormat::JsonSchema { .. }) {
                    format_value.as_object_mut().unwrap().remove("type");
                    format_value = json!({"type":"json_schema","json_schema":format_value});
                }
                request_fields.insert("response_format".into(), format_value);
            }
        }
        WireApi::Messages => {
            let OutputFormat::JsonSchema {
                schema,
                description,
                ..
            } = format
            else {
                return Err(AdapterError::parameter_unsupported());
            };
            let schema = described_schema(schema, description.as_deref());
            request_fields.insert(
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
            if let OutputFormat::JsonSchema {
                schema,
                description,
                ..
            } = format
            {
                generation.insert(
                    "responseJsonSchema".into(),
                    super::super::schema::to_gemini(&described_schema(
                        schema,
                        description.as_deref(),
                    ))?,
                );
            }
        }
    }
    Ok(())
}

fn described_schema(schema: &Value, description: Option<&str>) -> Value {
    let mut schema = schema.clone();
    if let Some(description) = description {
        // Preserve both envelope and schema descriptions without changing validation.
        let text = schema
            .get("description")
            .and_then(Value::as_str)
            .map_or_else(
                || description.to_owned(),
                |existing| format!("{description}\n{existing}"),
            );
        schema["description"] = text.into();
    }
    schema
}

pub(super) fn thinking(
    request_fields: &mut Map<String, Value>,
    generation: &mut Map<String, Value>,
    reasoning: &Reasoning,
    protocol: WireApi,
    mode: MessagesReasoningMode,
    explicit_max_tokens: bool,
    capabilities: Option<&crate::model_metadata::ModelCapabilities>,
) -> AdapterResult<()> {
    let fields = if protocol == WireApi::Gemini {
        generation
    } else {
        request_fields
    };
    let policy = super::super::super::contracts::ReasoningPolicy::new(protocol, mode, capabilities);
    match reasoning {
        Reasoning::Effort(effort) => policy.apply_effort(fields, effort, explicit_max_tokens),
        Reasoning::Budget(budget) => policy.apply_budget(fields, *budget, explicit_max_tokens),
        Reasoning::DynamicBudget => policy.apply_dynamic_budget(fields),
        Reasoning::Adaptive => Err(AdapterError::reasoning_unsupported()),
    }
}

pub(super) fn message_value(message: &Message, protocol: WireApi) -> AdapterResult<Vec<Value>> {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    };
    let mut encoded_items = Vec::new();
    let mut content = Vec::new();
    let mut calls = Vec::new();
    let mut reasoning = String::new();
    for block in &message.blocks {
        match (protocol, block) {
            (WireApi::ChatCompletions, Block::Reasoning(text)) => reasoning.push_str(text),
            (
                WireApi::Responses,
                Block::ToolCall {
                    id,
                    name,
                    arguments,
                },
            ) => {
                flush_responses_content(&mut encoded_items, &mut content, role);
                encoded_items.push(
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
                flush_responses_content(&mut encoded_items, &mut content, role);
                encoded_items.push(json!({"type":"function_call_output","call_id":id,"output":tool_output(output, *is_error, protocol)?}));
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
                encoded_items.push(json!({"role":"tool","tool_call_id":id,"content":tool_output(output, *is_error, protocol)?}));
            }
            _ => content.push(content_block(block, protocol, message.role)?),
        }
    }
    match protocol {
        WireApi::Responses => flush_responses_content(&mut encoded_items, &mut content, role),
        WireApi::ChatCompletions if !content.is_empty() || !calls.is_empty() || !reasoning.is_empty() => {
            let mut message_value = json!({"role":role,"content":if content.is_empty() { Value::Null } else { content.into() }});
            if !calls.is_empty() { message_value["tool_calls"] = calls.into(); }
            if !reasoning.is_empty() { message_value["reasoning_content"] = reasoning.into(); }
            encoded_items.push(message_value);
        }
        WireApi::Messages => encoded_items.push(json!({"role":role,"content":content})),
        WireApi::Gemini => encoded_items.push(json!({"role":if message.role == Role::Assistant { "model" } else { "user" },"parts":content})),
        _ => {}
    }
    Ok(encoded_items)
}

fn tool_output(content: &[Block], is_error: bool, protocol: WireApi) -> AdapterResult<Value> {
    if let Some(text) = tool_output_text(content) {
        return Ok(Value::String(if is_error {
            json!({"error":text}).to_string()
        } else {
            text
        }));
    }
    if protocol == WireApi::ChatCompletions {
        return Err(AdapterError::parameter_unsupported_for("input.output"));
    }
    let mut parts = Vec::new();
    if is_error {
        parts.push(content_block(
            &Block::Text("Tool execution failed.".into()),
            protocol,
            Role::User,
        )?);
    }
    parts.extend(
        content
            .iter()
            .map(|block| content_block(block, protocol, Role::User))
            .collect::<AdapterResult<Vec<_>>>()?,
    );
    Ok(Value::Array(parts))
}

fn tool_output_text(content: &[Block]) -> Option<String> {
    let mut text = String::new();
    for block in content {
        let Block::Text(part) = block else {
            return None;
        };
        text.push_str(part);
    }
    Some(text)
}

pub(super) fn flush_responses_content(
    response_items: &mut Vec<Value>,
    content: &mut Vec<Value>,
    role: &str,
) {
    if !content.is_empty() {
        response_items.push(json!({"role":role,"content":std::mem::take(content)}));
    }
}

pub(super) fn content_block(block: &Block, protocol: WireApi, role: Role) -> AdapterResult<Value> {
    Ok(match block {
        Block::Refusal(text)
            if matches!(protocol, WireApi::Responses | WireApi::ChatCompletions) =>
        {
            json!({"type":"refusal","refusal":text})
        }
        Block::Text(text) | Block::Refusal(text) => match protocol {
            WireApi::Responses => {
                json!({"type":if role == Role::Assistant { "output_text" } else { "input_text" },"text":text})
            }
            WireApi::Gemini => json!({"text":text}),
            _ => json!({"type":"text","text":text}),
        },
        Block::Image { url, detail } => match protocol {
            WireApi::Responses => {
                let mut responses_image = json!({"type":"input_image","image_url":url});
                if let Some(detail) = detail {
                    responses_image["detail"] = detail.clone().into();
                }
                responses_image
            }
            WireApi::ChatCompletions => {
                let mut chat_image = json!({"type":"image_url","image_url":{"url":url}});
                if let Some(detail) = detail {
                    chat_image["image_url"]["detail"] = detail.clone().into();
                }
                chat_image
            }
            WireApi::Messages | WireApi::Gemini => {
                if detail.as_deref().is_some_and(|detail| detail != "auto") {
                    return Err(AdapterError::parameter_unsupported());
                }
                if let Some((mime, encoded_image)) = url
                    .strip_prefix("data:")
                    .and_then(|data_url| data_url.split_once(";base64,"))
                {
                    if protocol == WireApi::Messages {
                        json!({"type":"image","source":{"type":"base64","media_type":mime,"data":encoded_image}})
                    } else {
                        json!({"inlineData":{"mimeType":mime,"data":encoded_image}})
                    }
                } else if protocol == WireApi::Messages {
                    json!({"type":"image","source":{"type":"url","url":url}})
                } else {
                    return Err(AdapterError::parameter_unsupported());
                }
            }
        },
        Block::Document {
            url,
            filename,
            detail,
        } => {
            if protocol != WireApi::Responses
                && detail.as_deref().is_some_and(|detail| detail != "auto")
            {
                return Err(AdapterError::parameter_unsupported_for("document.detail"));
            }
            let data = url.strip_prefix("data:application/pdf;base64,");
            match protocol {
                WireApi::Responses => {
                    let mut file = json!({"type":"input_file"});
                    file[if data.is_some() {
                        "file_data"
                    } else {
                        "file_url"
                    }] = url.clone().into();
                    if let Some(filename) = filename {
                        file["filename"] = filename.clone().into();
                    }
                    if let Some(detail) = detail {
                        file["detail"] = detail.clone().into();
                    }
                    file
                }
                WireApi::ChatCompletions => {
                    if data.is_none() {
                        return Err(AdapterError::parameter_unsupported_for("document.file_url"));
                    }
                    let mut file = json!({"file_data":url});
                    if let Some(filename) = filename {
                        file["filename"] = filename.clone().into();
                    }
                    json!({"type":"file","file":file})
                }
                WireApi::Messages => {
                    let source = match data {
                        Some(data) => {
                            json!({"type":"base64","media_type":"application/pdf","data":data})
                        }
                        None => json!({"type":"url","url":url}),
                    };
                    let mut file = json!({"type":"document","source":source});
                    if let Some(filename) = filename {
                        file["title"] = filename.clone().into();
                    }
                    file
                }
                WireApi::Gemini => {
                    let data = data.ok_or_else(|| {
                        AdapterError::parameter_unsupported_for("document.file_url")
                    })?;
                    let mut blob = json!({"mimeType":"application/pdf","data":data});
                    if let Some(filename) = filename {
                        blob["displayName"] = filename.clone().into();
                    }
                    json!({"inlineData":blob})
                }
            }
        }
        Block::ToolCall {
            id,
            name,
            arguments,
        } => {
            let tool_input: Value =
                serde_json::from_str(arguments).map_err(|_| AdapterError::invalid_request())?;
            if !tool_input.is_object() {
                return Err(AdapterError::invalid_request());
            }
            match protocol {
                WireApi::Messages => {
                    json!({"type":"tool_use","id":id,"name":name,"input":tool_input})
                }
                WireApi::Gemini => json!({"functionCall":{"id":id,"name":name,"args":tool_input}}),
                _ => return Err(AdapterError::unsupported_binding()),
            }
        }
        Block::ToolResult {
            id,
            name,
            content,
            is_error,
            raw_text,
        } => match protocol {
            WireApi::Messages => {
                let mut result = json!({"type":"tool_result","tool_use_id":id,"content":tool_output(content, false, protocol)?});
                if *is_error {
                    result["is_error"] = true.into();
                }
                result
            }
            WireApi::Gemini => {
                let text = content
                    .iter()
                    .filter_map(|block| {
                        if let Block::Text(text) = block {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<String>();
                let parsed = (!raw_text)
                    .then(|| serde_json::from_str::<Value>(&text).ok())
                    .flatten()
                    .filter(Value::is_object);
                let response = if *is_error {
                    match parsed {
                        Some(value) if value.get("error").is_some() => value,
                        Some(value) => json!({"error":value}),
                        None => json!({"error":text}),
                    }
                } else {
                    parsed.unwrap_or_else(|| json!({"output":text}))
                };
                let mut result = json!({"id":id,"name":name,"response":response});
                let mut media = Vec::new();
                for block in content
                    .iter()
                    .filter(|block| !matches!(block, Block::Text(_)))
                {
                    let part = content_block(block, protocol, Role::User)?;
                    if !matches!(
                        part.pointer("/inlineData/mimeType").and_then(Value::as_str),
                        Some("image/png" | "image/jpeg" | "image/webp")
                    ) {
                        return Err(AdapterError::parameter_unsupported_for("input.output"));
                    }
                    media.push(part);
                }
                if !media.is_empty() {
                    result["parts"] = Value::Array(media);
                }
                json!({"functionResponse":result})
            }
            _ => return Err(AdapterError::unsupported_binding()),
        },
        // Provider-owned thinking can only be replayed through native_turns,
        // with its real signature and source/model owner intact.
        Block::Reasoning(_) => return Err(AdapterError::reasoning_unsupported()),
    })
}
