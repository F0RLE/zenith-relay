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
    let mut format_value = match format {
        OutputFormat::JsonObject => json!({"type":"json_object"}),
        OutputFormat::JsonSchema {
            name,
            schema,
            strict,
        } => {
            let mut schema_value = json!({"type":"json_schema","name":name,"schema":schema});
            if let Some(strict) = strict {
                schema_value["strict"] = (*strict).into();
            }
            schema_value
        }
    };
    match protocol {
        WireApi::Responses => {
            request_fields.insert("text".into(), json!({"format":format_value}));
        }
        WireApi::ChatCompletions => {
            if matches!(format, OutputFormat::JsonSchema { .. }) {
                format_value.as_object_mut().unwrap().remove("type");
                format_value = json!({"type":"json_schema","json_schema":format_value});
            }
            request_fields.insert("response_format".into(), format_value);
        }
        WireApi::Messages => {
            let OutputFormat::JsonSchema { schema, .. } = format else {
                return Err(AdapterError::parameter_unsupported());
            };
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
            if let OutputFormat::JsonSchema { schema, .. } = format {
                generation.insert("responseJsonSchema".into(), schema.clone());
            }
        }
    }
    Ok(())
}

pub(super) fn thinking(
    request_fields: &mut Map<String, Value>,
    generation: &mut Map<String, Value>,
    reasoning: &Reasoning,
    protocol: WireApi,
) -> AdapterResult<()> {
    match (protocol, reasoning) {
        (WireApi::Responses, Reasoning::Effort(effort)) => {
            request_fields.insert("reasoning".into(), json!({"effort":effort}));
        }
        (WireApi::ChatCompletions, Reasoning::Effort(effort)) => {
            request_fields.insert("reasoning_effort".into(), effort.clone().into());
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
            request_fields.insert(
                "thinking".into(),
                json!({"type":"enabled","budget_tokens":budget}),
            );
        }
        (WireApi::Messages, Reasoning::Effort(effort)) if effort == "none" => {
            request_fields.insert("thinking".into(), json!({"type":"disabled"}));
        }
        (WireApi::Messages, Reasoning::Effort(effort))
            if matches!(effort.as_str(), "low" | "medium" | "high" | "max") =>
        {
            request_fields.insert("thinking".into(), json!({"type":"adaptive"}));
            request_fields
                .entry("output_config")
                .or_insert_with(|| json!({}))["effort"] = effort.clone().into();
        }
        _ => return Err(AdapterError::reasoning_unsupported()),
    }
    Ok(())
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
                encoded_items.push(json!({"type":"function_call_output","call_id":id,"output":tool_output(output, *is_error)}));
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
                encoded_items.push(json!({"role":"tool","tool_call_id":id,"content":tool_output(output, *is_error)}));
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

pub(super) fn tool_output(content: &str, is_error: bool) -> String {
    if is_error {
        json!({"error":content}).to_string()
    } else {
        content.into()
    }
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
        Block::Text(text) => match protocol {
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
        } => match protocol {
            WireApi::Messages => {
                json!({"type":"tool_result","tool_use_id":id,"content":content,"is_error":is_error})
            }
            WireApi::Gemini => {
                let tool_response = serde_json::from_str::<Value>(content)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| {
                        if *is_error {
                            json!({"error":content})
                        } else {
                            json!({"result":content})
                        }
                    });
                json!({"functionResponse":{"id":id,"name":name,"response":tool_response}})
            }
            _ => return Err(AdapterError::unsupported_binding()),
        },
        Block::Reasoning(text) if protocol == WireApi::Messages => {
            json!({"type":"thinking","thinking":text})
        }
        Block::Reasoning(text) if protocol == WireApi::Gemini => {
            json!({"text":text,"thought":true})
        }
        Block::Reasoning(_) => return Err(AdapterError::reasoning_unsupported()),
    })
}
