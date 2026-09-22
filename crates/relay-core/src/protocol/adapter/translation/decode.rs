use super::*;
use serde_json::json;
use std::collections::BTreeMap;

pub(super) fn request(protocol: WireApi, value: &Value) -> AdapterResult<Request> {
    let request = match protocol {
        WireApi::Responses => responses(value)?,
        WireApi::ChatCompletions => chat(value)?,
        WireApi::Messages => messages(value)?,
        WireApi::Gemini => gemini(value)?,
    };
    if request
        .messages
        .iter()
        .all(|message| message.role == Role::System)
    {
        return Err(AdapterError::invalid_request());
    }
    if let Some(ToolChoice::Function(name)) = &request.tool_choice {
        if !request.tools.iter().any(|tool| tool.name == *name) {
            return Err(AdapterError::unsupported_tool());
        }
    }
    Ok(request)
}

pub(super) fn resolve_tool_history(messages: &mut [Message]) -> AdapterResult<()> {
    // Gemini function results are linked by name; assign their original call
    // identifiers when the history supplies them, without dropping any result.
    let mut pending = Vec::<(String, String)>::new();
    let mut ids = messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| {
            if let Block::ToolCall { id, .. } = block {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect::<std::collections::HashSet<_>>();
    let mut generated = 0;
    for message in messages.iter_mut() {
        for block in &mut message.blocks {
            match block {
                Block::ToolCall { id, name, .. } => {
                    if id.is_empty() {
                        loop {
                            *id = format!("call_relay_{generated}");
                            generated += 1;
                            if ids.insert(id.clone()) {
                                break;
                            }
                        }
                    }
                    pending.push((id.clone(), name.clone()));
                }
                Block::ToolResult { id, name, .. } => {
                    let index = pending
                        .iter()
                        .position(|(call_id, call_name)| {
                            if id.is_empty() {
                                call_name == name
                            } else {
                                call_id == id
                            }
                        })
                        .ok_or_else(AdapterError::invalid_request)?;
                    let (call_id, call_name) = pending.remove(index);
                    *id = call_id;
                    *name = call_name;
                }
                _ => {}
            }
        }
    }
    validate_tool_history(messages)
}

fn validate_tool_history(messages: &[Message]) -> AdapterResult<()> {
    let mut pending = BTreeMap::new();
    for message in messages {
        for block in &message.blocks {
            match block {
                Block::ToolCall { id, name, .. } => {
                    if pending.insert(id, name).is_some() {
                        return Err(AdapterError::invalid_request());
                    }
                }
                Block::ToolResult { id, name, .. }
                    if pending
                        .remove(id)
                        .is_none_or(|call_name| !name.is_empty() && call_name != name) =>
                {
                    return Err(AdapterError::invalid_request());
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn role(value: &Value) -> AdapterResult<Role> {
    match value.get("role").and_then(Value::as_str) {
        Some("system" | "developer") => Ok(Role::System),
        Some("user") | None => Ok(Role::User),
        Some("assistant" | "model") => Ok(Role::Assistant),
        _ => Err(AdapterError::invalid_request()),
    }
}

fn text_blocks(value: &Value, protocol: WireApi) -> AdapterResult<Vec<Block>> {
    if let Some(text) = value.as_str() {
        return Ok(vec![Block::Text(text.to_owned())]);
    }
    if value.is_null() {
        return Ok(Vec::new());
    }
    value
        .as_array()
        .ok_or_else(AdapterError::invalid_request)?
        .iter()
        .enumerate()
        .map(|(index, part)| block(part, protocol, index))
        .collect()
}

fn block(part: &Value, protocol: WireApi, _index: usize) -> AdapterResult<Block> {
    if protocol == WireApi::Gemini {
        checked(
            part,
            &[
                "text",
                "inlineData",
                "fileData",
                "functionCall",
                "functionResponse",
                "thought",
            ],
        )?;
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            return Ok(
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    Block::Reasoning(text.into())
                } else {
                    Block::Text(text.into())
                },
            );
        }
        if let Some(data) = part.get("inlineData") {
            checked(data, &["mimeType", "data"])?;
            return image(
                format!(
                    "data:{};base64,{}",
                    required_text(data, "mimeType")?,
                    required_text(data, "data")?
                ),
                None,
            );
        }
        if let Some(data) = part.get("fileData") {
            checked(data, &["mimeType", "fileUri"])?;
            if data
                .get("mimeType")
                .and_then(Value::as_str)
                .is_none_or(|mime| !mime.starts_with("image/"))
            {
                return Err(AdapterError::parameter_unsupported());
            }
            return image(required_text(data, "fileUri")?.into(), None);
        }
        if let Some(call) = part.get("functionCall") {
            checked(call, &["id", "name", "args"])?;
            return Ok(Block::ToolCall {
                id: call
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_default(),
                name: required_text(call, "name")?.into(),
                arguments: call.get("args").unwrap_or(&json!({})).to_string(),
            });
        }
        if let Some(result) = part.get("functionResponse") {
            checked(result, &["id", "name", "response"])?;
            let response = result
                .get("response")
                .ok_or_else(AdapterError::invalid_request)?;
            return Ok(Block::ToolResult {
                id: result
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                name: required_text(result, "name")?.into(),
                content: response.to_string(),
                is_error: response.get("error").is_some(),
            });
        }
        return Err(AdapterError::parameter_unsupported());
    }
    match required_text(part, "type")? {
        "thinking" if protocol == WireApi::Messages => {
            checked(part, &["type", "thinking"])?;
            Ok(Block::Reasoning(
                part.get("thinking")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::invalid_request)?
                    .into(),
            ))
        }
        "text" | "input_text" | "output_text" => {
            checked(part, &["type", "text", "annotations"])?;
            if part
                .get("annotations")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
            {
                return Err(AdapterError::parameter_unsupported());
            }
            Ok(Block::Text(
                part.get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::invalid_request)?
                    .into(),
            ))
        }
        "image_url" | "input_image" => {
            checked(part, &["type", "image_url", "detail"])?;
            let image_value = part
                .get("image_url")
                .ok_or_else(AdapterError::invalid_request)?;
            let (url, detail) = if let Some(url) = image_value.as_str() {
                (url, part.get("detail").and_then(Value::as_str))
            } else {
                checked(image_value, &["url", "detail"])?;
                (
                    required_text(image_value, "url")?,
                    image_value.get("detail").and_then(Value::as_str),
                )
            };
            image(url.into(), detail.map(str::to_owned))
        }
        "image" => {
            checked(part, &["type", "source"])?;
            let source = part
                .get("source")
                .ok_or_else(AdapterError::invalid_request)?;
            match required_text(source, "type")? {
                "base64" => {
                    checked(source, &["type", "media_type", "data"])?;
                    image(
                        format!(
                            "data:{};base64,{}",
                            required_text(source, "media_type")?,
                            required_text(source, "data")?
                        ),
                        None,
                    )
                }
                "url" => {
                    checked(source, &["type", "url"])?;
                    image(required_text(source, "url")?.into(), None)
                }
                _ => Err(AdapterError::parameter_unsupported()),
            }
        }
        "tool_use" => {
            checked(part, &["type", "id", "name", "input"])?;
            Ok(Block::ToolCall {
                id: required_text(part, "id")?.into(),
                name: required_text(part, "name")?.into(),
                arguments: part
                    .get("input")
                    .filter(|input| input.is_object())
                    .ok_or_else(AdapterError::invalid_request)?
                    .to_string(),
            })
        }
        "tool_result" => {
            checked(part, &["type", "tool_use_id", "content", "is_error"])?;
            Ok(Block::ToolResult {
                id: required_text(part, "tool_use_id")?.into(),
                name: String::new(),
                content: plain_text(part.get("content").unwrap_or(&Value::Null), protocol)?,
                is_error: optional_bool(part, "is_error")?.unwrap_or(false),
            })
        }
        _ => Err(AdapterError::parameter_unsupported()),
    }
}

fn image(url: String, detail: Option<String>) -> AdapterResult<Block> {
    use base64::Engine;
    if let Some(data) = url.strip_prefix("data:") {
        let (mime, data) = data
            .split_once(";base64,")
            .ok_or_else(AdapterError::invalid_request)?;
        if !["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&mime)
            || data.len() > 28 * 1024 * 1024
        {
            return Err(AdapterError::parameter_unsupported());
        }
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| AdapterError::invalid_request())?;
    } else {
        let parsed = url::Url::parse(&url).map_err(|_| AdapterError::invalid_request())?;
        if !matches!(parsed.scheme(), "https" | "http")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(AdapterError::parameter_unsupported());
        }
    }
    Ok(Block::Image { url, detail })
}

fn plain_text(value: &Value, protocol: WireApi) -> AdapterResult<String> {
    let mut result = String::new();
    for block in text_blocks(value, protocol)? {
        let Block::Text(text) = block else {
            return Err(AdapterError::parameter_unsupported());
        };
        result.push_str(&text);
    }
    Ok(result)
}

fn tools(values: Option<&Value>, protocol: WireApi) -> AdapterResult<Vec<Function>> {
    let Some(values) = values.filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for tool in values
        .as_array()
        .ok_or_else(AdapterError::invalid_request)?
    {
        if protocol == WireApi::Gemini {
            checked(tool, &["functionDeclarations"])?;
            for declaration in tool
                .get("functionDeclarations")
                .and_then(Value::as_array)
                .ok_or_else(AdapterError::unsupported_tool)?
            {
                result.push(function(
                    declaration,
                    "parameters",
                    &["name", "description", "parameters", "parametersJsonSchema"],
                    true,
                )?);
            }
        } else if protocol == WireApi::Messages {
            result.push(function(
                tool,
                "input_schema",
                &["name", "description", "input_schema", "type"],
                false,
            )?);
        } else {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(AdapterError::unsupported_tool());
            }
            let declaration = if protocol == WireApi::ChatCompletions {
                checked(tool, &["type", "function"])?;
                tool.get("function")
                    .ok_or_else(AdapterError::unsupported_tool)?
            } else {
                tool
            };
            result.push(function(
                declaration,
                "parameters",
                &["type", "name", "description", "parameters", "strict"],
                false,
            )?);
        }
    }
    let mut names = std::collections::BTreeSet::new();
    if result.iter().any(|tool| !names.insert(&tool.name)) {
        return Err(AdapterError::invalid_request());
    }
    Ok(result)
}

fn function(
    value: &Value,
    schema_key: &str,
    allowed: &[&str],
    gemini: bool,
) -> AdapterResult<Function> {
    checked(value, allowed)?;
    if value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| !matches!(kind, "function" | "custom"))
    {
        return Err(AdapterError::unsupported_tool());
    }
    let schema = value
        .get(schema_key)
        .or_else(|| gemini.then(|| value.get("parametersJsonSchema")).flatten())
        .cloned()
        .unwrap_or_else(|| json!({"type":"object","properties":{}}));
    if !schema.is_object() {
        return Err(AdapterError::invalid_request());
    }
    Ok(Function {
        name: required_text(value, "name")?.into(),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        parameters: schema,
        strict: optional_bool(value, "strict")?,
    })
}

fn common(
    request: &mut Request,
    value: &Value,
    max: &str,
    top_p: &str,
    stop: &str,
) -> AdapterResult<()> {
    request.max_tokens = optional_u64(value, max)?;
    request.temperature = optional_f64(value, "temperature")?;
    request.top_p = optional_f64(value, top_p)?;
    if let Some(value) = value.get(stop).filter(|value| !value.is_null()) {
        request.stop = if let Some(text) = value.as_str() {
            vec![text.into()]
        } else {
            value
                .as_array()
                .ok_or_else(AdapterError::invalid_request)?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(AdapterError::invalid_request)
                })
                .collect::<AdapterResult<_>>()?
        };
    }
    Ok(())
}

fn choice(value: Option<&Value>, protocol: WireApi) -> AdapterResult<Option<ToolChoice>> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let kind = value
        .as_str()
        .or_else(|| value.get("type").and_then(Value::as_str))
        .ok_or_else(AdapterError::unsupported_tool)?;
    Ok(Some(match kind {
        "auto" => ToolChoice::Auto,
        "none" => ToolChoice::None,
        "required" | "any" => ToolChoice::Required,
        "function" | "tool" => {
            let target = if protocol == WireApi::ChatCompletions {
                value
                    .get("function")
                    .ok_or_else(AdapterError::unsupported_tool)?
            } else {
                value
            };
            ToolChoice::Function(required_text(target, "name")?.into())
        }
        _ => return Err(AdapterError::unsupported_tool()),
    }))
}

fn output_format(value: &Value) -> AdapterResult<Option<OutputFormat>> {
    if value.is_null() {
        return Ok(None);
    }
    checked(
        value,
        &[
            "type",
            "name",
            "schema",
            "strict",
            "description",
            "json_schema",
        ],
    )?;
    match value.get("type").and_then(Value::as_str) {
        Some("text") => Ok(None),
        Some("json_object") => Ok(Some(OutputFormat::JsonObject)),
        Some("json_schema") => {
            let schema = value.get("json_schema").unwrap_or(value);
            checked(schema, &["type", "name", "schema", "strict", "description"])?;
            Ok(Some(OutputFormat::JsonSchema {
                name: schema
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("response")
                    .into(),
                schema: schema
                    .get("schema")
                    .filter(|schema| schema.is_object())
                    .ok_or_else(AdapterError::invalid_request)?
                    .clone(),
                strict: optional_bool(schema, "strict")?,
            }))
        }
        _ => Err(AdapterError::parameter_unsupported()),
    }
}

fn responses(value: &Value) -> AdapterResult<Request> {
    super::super::contracts::validate_responses_bridge_request(value, WireApi::ChatCompletions)?;
    let mut request = Request::default();
    if let Some(instructions) = value.get("instructions").filter(|v| !v.is_null()) {
        request.instructions = Some(Message {
            role: Role::System,
            blocks: text_blocks(instructions, WireApi::Responses)?,
        });
    }
    let input = value
        .get("input")
        .ok_or_else(AdapterError::invalid_request)?;
    if let Some(text) = input.as_str() {
        request.messages.push(Message {
            role: Role::User,
            blocks: vec![Block::Text(text.into())],
        });
    } else {
        for item in input.as_array().ok_or_else(AdapterError::invalid_request)? {
            match item
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("message")
            {
                "message" => {
                    checked(item, &["type", "role", "content", "id", "status"])?;
                    request.messages.push(Message {
                        role: role(item)?,
                        blocks: text_blocks(
                            item.get("content")
                                .ok_or_else(AdapterError::invalid_request)?,
                            WireApi::Responses,
                        )?,
                    });
                }
                "function_call" => {
                    checked(
                        item,
                        &["type", "id", "status", "call_id", "name", "arguments"],
                    )?;
                    request.messages.push(Message {
                        role: Role::Assistant,
                        blocks: vec![Block::ToolCall {
                            id: required_text(item, "call_id")?.into(),
                            name: required_text(item, "name")?.into(),
                            arguments: required_text(item, "arguments")?.into(),
                        }],
                    });
                }
                "function_call_output" => {
                    checked(item, &["type", "id", "status", "call_id", "output"])?;
                    request.messages.push(Message {
                        role: Role::User,
                        blocks: vec![Block::ToolResult {
                            id: required_text(item, "call_id")?.into(),
                            name: String::new(),
                            content: plain_text(
                                item.get("output")
                                    .ok_or_else(AdapterError::invalid_request)?,
                                WireApi::Responses,
                            )?,
                            is_error: false,
                        }],
                    });
                }
                "reasoning" => {
                    // Public summaries are portable. Encrypted state is not:
                    // `checked` rejects it instead of dropping its ownership.
                    checked(item, &["type", "id", "status", "summary"])?;
                    let blocks = item
                        .get("summary")
                        .and_then(Value::as_array)
                        .ok_or_else(AdapterError::invalid_request)?
                        .iter()
                        .map(|summary| {
                            checked(summary, &["type", "text"])?;
                            if required_text(summary, "type")? != "summary_text" {
                                return Err(AdapterError::parameter_unsupported());
                            }
                            Ok(Block::Reasoning(
                                summary
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .ok_or_else(AdapterError::invalid_request)?
                                    .into(),
                            ))
                        })
                        .collect::<AdapterResult<Vec<_>>>()?;
                    request.messages.push(Message {
                        role: Role::Assistant,
                        blocks,
                    });
                }
                _ => return Err(AdapterError::parameter_unsupported()),
            }
        }
    }
    request.tools = tools(value.get("tools"), WireApi::Responses)?;
    request.tool_choice = choice(value.get("tool_choice"), WireApi::Responses)?;
    request.parallel_tools = optional_bool(value, "parallel_tool_calls")?;
    common(&mut request, value, "max_output_tokens", "top_p", "stop")?;
    if let Some(text) = value.get("text").filter(|v| !v.is_null()) {
        checked(text, &["format"])?;
        request.output_format = output_format(text.get("format").unwrap_or(&Value::Null))?;
    }
    if let Some(reasoning) = value.get("reasoning").filter(|v| !v.is_null()) {
        if let Some(effort) = reasoning.get("effort").filter(|value| !value.is_null()) {
            request.reasoning = Some(Reasoning::Effort(
                effort
                    .as_str()
                    .ok_or_else(AdapterError::invalid_request)?
                    .into(),
            ));
        }
    }
    Ok(request)
}

fn chat(value: &Value) -> AdapterResult<Request> {
    checked(
        value,
        &[
            "model",
            "stream",
            "messages",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
            "max_tokens",
            "max_completion_tokens",
            "temperature",
            "top_p",
            "stop",
            "response_format",
            "reasoning_effort",
            "stream_options",
            "n",
            "store",
        ],
    )?;
    if optional_u64(value, "n")?.is_some_and(|n| n != 1)
        || optional_bool(value, "store")? == Some(true)
    {
        return Err(AdapterError::parameter_unsupported());
    }
    if let Some(options) = value.get("stream_options").filter(|v| !v.is_null()) {
        checked(options, &["include_usage"])?;
    }
    let mut request = Request::default();
    for message in value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::invalid_request)?
    {
        checked(
            message,
            &[
                "role",
                "content",
                "tool_calls",
                "tool_call_id",
                "reasoning_content",
            ],
        )?;
        let reasoning = message
            .get("reasoning_content")
            .filter(|value| !value.is_null());
        if reasoning.is_some() && role(message)? != Role::Assistant {
            return Err(AdapterError::invalid_request());
        }
        if message.get("role").and_then(Value::as_str) == Some("tool") {
            request.messages.push(Message {
                role: Role::User,
                blocks: vec![Block::ToolResult {
                    id: required_text(message, "tool_call_id")?.into(),
                    name: String::new(),
                    content: plain_text(
                        message.get("content").unwrap_or(&Value::Null),
                        WireApi::ChatCompletions,
                    )?,
                    is_error: false,
                }],
            });
            continue;
        }
        let mut blocks = text_blocks(
            message.get("content").unwrap_or(&Value::Null),
            WireApi::ChatCompletions,
        )?;
        if let Some(reasoning) = reasoning {
            blocks.insert(
                0,
                Block::Reasoning(
                    reasoning
                        .as_str()
                        .ok_or_else(AdapterError::invalid_request)?
                        .into(),
                ),
            );
        }
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                checked(call, &["id", "type", "function"])?;
                if required_text(call, "type")? != "function" {
                    return Err(AdapterError::unsupported_tool());
                }
                let function = call
                    .get("function")
                    .ok_or_else(AdapterError::invalid_request)?;
                checked(function, &["name", "arguments"])?;
                blocks.push(Block::ToolCall {
                    id: required_text(call, "id")?.into(),
                    name: required_text(function, "name")?.into(),
                    arguments: required_text(function, "arguments")?.into(),
                });
            }
        }
        request.messages.push(Message {
            role: role(message)?,
            blocks,
        });
    }
    request.tools = tools(value.get("tools"), WireApi::ChatCompletions)?;
    request.tool_choice = choice(value.get("tool_choice"), WireApi::ChatCompletions)?;
    request.parallel_tools = optional_bool(value, "parallel_tool_calls")?;
    common(
        &mut request,
        value,
        if value.get("max_completion_tokens").is_some() {
            "max_completion_tokens"
        } else {
            "max_tokens"
        },
        "top_p",
        "stop",
    )?;
    request.output_format = output_format(value.get("response_format").unwrap_or(&Value::Null))?;
    request.reasoning = value
        .get("reasoning_effort")
        .filter(|v| !v.is_null())
        .map(|v| {
            v.as_str()
                .map(|effort| Reasoning::Effort(effort.into()))
                .ok_or_else(AdapterError::invalid_request)
        })
        .transpose()?;
    Ok(request)
}

fn messages(value: &Value) -> AdapterResult<Request> {
    checked(
        value,
        &[
            "model",
            "stream",
            "messages",
            "system",
            "max_tokens",
            "temperature",
            "top_p",
            "stop_sequences",
            "tools",
            "tool_choice",
            "thinking",
            "output_config",
        ],
    )?;
    let mut request = Request::default();
    if let Some(system) = value.get("system") {
        request.messages.push(Message {
            role: Role::System,
            blocks: text_blocks(system, WireApi::Messages)?,
        });
    }
    for message in value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::invalid_request)?
    {
        checked(message, &["role", "content"])?;
        request.messages.push(Message {
            role: role(message)?,
            blocks: text_blocks(
                message
                    .get("content")
                    .ok_or_else(AdapterError::invalid_request)?,
                WireApi::Messages,
            )?,
        });
    }
    request.tools = tools(value.get("tools"), WireApi::Messages)?;
    request.tool_choice = choice(value.get("tool_choice"), WireApi::Messages)?;
    if let Some(choice) = value.get("tool_choice") {
        checked(choice, &["type", "name", "disable_parallel_tool_use"])?;
        request.parallel_tools =
            optional_bool(choice, "disable_parallel_tool_use")?.map(|disabled| !disabled);
    }
    common(&mut request, value, "max_tokens", "top_p", "stop_sequences")?;
    if let Some(output) = value.get("output_config").filter(|v| !v.is_null()) {
        checked(output, &["format", "effort"])?;
        request.output_format = output_format(output.get("format").unwrap_or(&Value::Null))?;
        request.reasoning = output
            .get("effort")
            .and_then(Value::as_str)
            .map(|effort| Reasoning::Effort(effort.into()));
    }
    if let Some(thinking) = value.get("thinking").filter(|v| !v.is_null()) {
        checked(thinking, &["type", "budget_tokens"])?;
        match required_text(thinking, "type")? {
            "disabled" => request.reasoning = Some(Reasoning::Effort("none".into())),
            "adaptive" if request.reasoning.is_some() => {}
            "enabled" => {
                request.reasoning = Some(Reasoning::Budget(
                    optional_u64(thinking, "budget_tokens")?
                        .ok_or_else(AdapterError::invalid_request)?,
                ))
            }
            _ => return Err(AdapterError::reasoning_unsupported()),
        }
    }
    Ok(request)
}

fn gemini(value: &Value) -> AdapterResult<Request> {
    checked(
        value,
        &[
            "model",
            "stream",
            "contents",
            "systemInstruction",
            "tools",
            "toolConfig",
            "generationConfig",
        ],
    )?;
    let mut request = Request::default();
    if let Some(system) = value.get("systemInstruction") {
        checked(system, &["role", "parts"])?;
        request.messages.push(Message {
            role: Role::System,
            blocks: text_blocks(
                system
                    .get("parts")
                    .ok_or_else(AdapterError::invalid_request)?,
                WireApi::Gemini,
            )?,
        });
    }
    for content in value
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::invalid_request)?
    {
        checked(content, &["role", "parts"])?;
        request.messages.push(Message {
            role: role(content)?,
            blocks: text_blocks(
                content
                    .get("parts")
                    .ok_or_else(AdapterError::invalid_request)?,
                WireApi::Gemini,
            )?,
        });
    }
    request.tools = tools(value.get("tools"), WireApi::Gemini)?;
    if let Some(config) = value.get("toolConfig") {
        checked(config, &["functionCallingConfig"])?;
        let choice = config
            .get("functionCallingConfig")
            .ok_or_else(AdapterError::unsupported_tool)?;
        checked(choice, &["mode", "allowedFunctionNames"])?;
        request.tool_choice = Some(match required_text(choice, "mode")? {
            "AUTO" => ToolChoice::Auto,
            "NONE" => ToolChoice::None,
            "ANY" => {
                if let Some(names) = choice.get("allowedFunctionNames").and_then(Value::as_array) {
                    if names.len() != 1 {
                        return Err(AdapterError::unsupported_tool());
                    }
                    ToolChoice::Function(
                        names[0]
                            .as_str()
                            .ok_or_else(AdapterError::unsupported_tool)?
                            .into(),
                    )
                } else {
                    ToolChoice::Required
                }
            }
            _ => return Err(AdapterError::unsupported_tool()),
        });
    }
    if let Some(config) = value.get("generationConfig").filter(|v| !v.is_null()) {
        checked(
            config,
            &[
                "maxOutputTokens",
                "temperature",
                "topP",
                "stopSequences",
                "responseMimeType",
                "responseSchema",
                "responseJsonSchema",
                "thinkingConfig",
                "candidateCount",
            ],
        )?;
        if optional_u64(config, "candidateCount")?.is_some_and(|n| n != 1) {
            return Err(AdapterError::parameter_unsupported());
        }
        common(
            &mut request,
            config,
            "maxOutputTokens",
            "topP",
            "stopSequences",
        )?;
        if let Some(mime) = config.get("responseMimeType").and_then(Value::as_str) {
            request.output_format = match mime {
                "text/plain" => None,
                "application/json" => Some(
                    config
                        .get("responseJsonSchema")
                        .or_else(|| config.get("responseSchema"))
                        .map_or(OutputFormat::JsonObject, |schema| {
                            OutputFormat::JsonSchema {
                                name: "response".into(),
                                schema: schema.clone(),
                                strict: None,
                            }
                        }),
                ),
                _ => return Err(AdapterError::parameter_unsupported()),
            };
        }
        if let Some(thinking) = config.get("thinkingConfig") {
            checked(thinking, &["thinkingLevel", "thinkingBudget"])?;
            request.reasoning =
                if let Some(level) = thinking.get("thinkingLevel").and_then(Value::as_str) {
                    Some(Reasoning::Effort(level.to_ascii_lowercase()))
                } else {
                    optional_u64(thinking, "thinkingBudget")?.map(Reasoning::Budget)
                };
        }
    }
    Ok(request)
}
