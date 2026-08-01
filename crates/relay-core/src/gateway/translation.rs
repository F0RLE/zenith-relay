use super::errors::AttemptFailure;
use axum::body::Bytes;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};
use std::collections::HashSet;

const CHAT_NAMESPACE_TOOL_PREFIX: &str = "zr_ns__";
const CHAT_CUSTOM_TOOL_PREFIX: &str = "zr_custom__";
const CHAT_ENCODED_FUNCTION_PREFIX: &str = "zr_fn__";
const CHAT_TOOL_SEARCH_NAME: &str = "zr_tool_search";
const MAX_CHAT_FUNCTION_NAME_BYTES: usize = 64;

pub(super) fn translate_responses_request(
    request: &Value,
    model: &str,
    stream: bool,
) -> Result<Vec<u8>, AttemptFailure> {
    let object = request
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    if object
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(AttemptFailure::invalid_request());
    }

    let mut messages = Vec::new();
    if let Some(instructions) = object.get("instructions") {
        let Some(instructions) = instructions.as_str() else {
            return Err(AttemptFailure::invalid_request());
        };
        if !instructions.trim().is_empty() {
            messages.push(json!({"role": "system", "content": instructions}));
        }
    }
    let input = object
        .get("input")
        .ok_or_else(AttemptFailure::invalid_request)?;
    messages.extend(translate_responses_input(input)?);
    let mut tools = translate_responses_tools(object)?;

    let mut translated = serde_json::Map::from_iter([
        ("model".to_string(), Value::String(model.to_string())),
        ("messages".to_string(), Value::Array(messages)),
        ("stream".to_string(), Value::Bool(stream)),
    ]);
    for field in ["temperature", "top_p", "stop", "service_tier"] {
        if let Some(value) = object.get(field) {
            translated.insert(field.to_string(), value.clone());
        }
    }
    if let Some(value) = object.get("max_output_tokens") {
        translated.insert("max_completion_tokens".to_string(), value.clone());
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        if let Some(choice) = translate_responses_tool_choice(tool_choice, &mut tools)? {
            translated.insert("tool_choice".to_string(), choice);
        }
    }
    if !tools.is_empty() {
        translated.insert("tools".to_string(), Value::Array(tools));
    }
    if object.get("parallel_tool_calls").is_some() {
        // Generic Chat Completions sources do not provide a reliable capability
        // contract for concurrent tool execution.  The translator still carries
        // all ordinary tools, but serializes calls safely on this wire.
        translated.insert("parallel_tool_calls".to_string(), Value::Bool(false));
    }
    if let Some(effort) = object
        .get("reasoning")
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(Value::as_str)
    {
        translated.insert(
            "reasoning_effort".to_string(),
            Value::String(effort.to_string()),
        );
    }
    serde_json::to_vec(&Value::Object(translated)).map_err(|_| AttemptFailure::invalid_request())
}

/// Builds the short-lived state required to replay a Responses continuation
/// through a stateless Chat Completions source.  The caller keeps this value
/// in memory only; it is never written to usage telemetry or persistent state.
pub(super) fn responses_replay_seed(
    request: &Value,
    response: &Value,
) -> Result<Value, AttemptFailure> {
    let mut replay = request
        .as_object()
        .cloned()
        .ok_or_else(AttemptFailure::invalid_request)?;
    let input = replay
        .remove("input")
        .ok_or_else(AttemptFailure::invalid_request)?;
    let mut history = normalize_responses_replay_input(&input)?;
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .filter(|output| !output.is_empty())
        .ok_or_else(AttemptFailure::translation)?;
    if !output.iter().all(response_replay_item_is_translatable) {
        return Err(AttemptFailure::translation());
    }
    history.extend(output.iter().cloned());
    replay.remove("previous_response_id");
    replay.insert("input".to_string(), Value::Array(history));
    Ok(Value::Object(replay))
}

/// Applies a client delta to an in-memory replay seed.  Responses clients send
/// only the next item plus `previous_response_id`; Chat Completions needs the
/// earlier user turn and assistant tool call in the same request.
pub(super) fn replay_responses_request(
    previous: &Value,
    next: &Value,
) -> Result<Value, AttemptFailure> {
    let mut replay = previous
        .as_object()
        .cloned()
        .ok_or_else(AttemptFailure::invalid_request)?;
    let next = next
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    let mut history = replay
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(AttemptFailure::invalid_request)?;
    let input = next
        .get("input")
        .ok_or_else(AttemptFailure::invalid_request)?;
    history.extend(normalize_responses_replay_input(input)?);
    for (field, value) in next {
        if field != "input" && field != "previous_response_id" {
            replay.insert(field.clone(), value.clone());
        }
    }
    replay.remove("previous_response_id");
    replay.insert("input".to_string(), Value::Array(history));
    Ok(Value::Object(replay))
}

fn normalize_responses_replay_input(input: &Value) -> Result<Vec<Value>, AttemptFailure> {
    match input {
        Value::String(text) => Ok(vec![json!({
            "role": "user",
            "content": [{"type": "input_text", "text": text}],
        })]),
        Value::Object(item) => Ok(vec![Value::Object(item.clone())]),
        Value::Array(items) if items.iter().all(is_message_content) => Ok(vec![json!({
            "role": "user",
            "content": items,
        })]),
        Value::Array(items) => Ok(items.clone()),
        _ => Err(AttemptFailure::invalid_request()),
    }
}

fn response_replay_item_is_translatable(item: &Value) -> bool {
    let Some(object) = item.as_object() else {
        return false;
    };
    if object.get("role").and_then(Value::as_str).is_some() {
        return true;
    }
    matches!(
        object.get("type").and_then(Value::as_str),
        Some(
            "additional_tools"
                | "function_call"
                | "custom_tool_call"
                | "tool_search_call"
                | "local_shell_call"
                | "function_call_output"
                | "custom_tool_call_output"
                | "tool_search_output"
                | "reasoning"
        )
    )
}

fn translate_responses_input(input: &Value) -> Result<Vec<Value>, AttemptFailure> {
    match input {
        Value::String(text) => Ok(vec![json!({"role": "user", "content": text})]),
        Value::Array(items) if !items.is_empty() && items.iter().all(is_message_content) => {
            Ok(vec![json!({
                "role": "user",
                "content": translate_message_content(input)?,
            })])
        }
        Value::Array(items) if !items.is_empty() => {
            let mut messages = Vec::new();
            let mut tool_calls = Vec::new();
            for item in items {
                let item_type = item.get("type").and_then(Value::as_str);
                // Responses Lite places Codex tools inside a developer input
                // item.  They are translated separately into Chat Completions
                // tool definitions and must never become a chat message.
                if item_type == Some("additional_tools") {
                    continue;
                }
                if let Some(role) = item.get("role").and_then(Value::as_str) {
                    flush_chat_tool_calls(&mut messages, &mut tool_calls);
                    if !matches!(role, "developer" | "system" | "user" | "assistant" | "tool") {
                        return Err(AttemptFailure::invalid_request());
                    }
                    if role == "tool" {
                        let call_id = item
                            .get("tool_call_id")
                            .or_else(|| item.get("call_id"))
                            .and_then(Value::as_str)
                            .ok_or_else(AttemptFailure::invalid_request)?;
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": response_tool_output_text(
                                item.get("content").ok_or_else(AttemptFailure::invalid_request)?
                            )?,
                        }));
                        continue;
                    }
                    let content = translate_message_content(
                        item.get("content")
                            .ok_or_else(AttemptFailure::invalid_request)?,
                    )?;
                    messages.push(json!({"role": role, "content": content}));
                    continue;
                }
                match item_type {
                    Some("function_call") => {
                        let call_id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .ok_or_else(AttemptFailure::invalid_request)?;
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .ok_or_else(AttemptFailure::invalid_request)?;
                        let wire_name = chat_function_wire_name(
                            item.get("namespace").and_then(Value::as_str),
                            name,
                        )?;
                        tool_calls.push(json!({
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "name": wire_name,
                                "arguments": item.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
                            }
                        }));
                    }
                    Some("custom_tool_call") => {
                        let call_id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .ok_or_else(AttemptFailure::invalid_request)?;
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .ok_or_else(AttemptFailure::invalid_request)?;
                        let input = item
                            .get("input")
                            .and_then(Value::as_str)
                            .ok_or_else(AttemptFailure::invalid_request)?;
                        tool_calls.push(json!({
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "name": chat_custom_tool_wire_name(name)?,
                                "arguments": serde_json::to_string(&json!({"input": input}))
                                    .map_err(|_| AttemptFailure::invalid_request())?,
                            }
                        }));
                    }
                    Some("tool_search_call") => {
                        let call_id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .ok_or_else(AttemptFailure::invalid_request)?;
                        tool_calls.push(json!({
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "name": CHAT_TOOL_SEARCH_NAME,
                                "arguments": response_tool_arguments(item.get("arguments"))?,
                            }
                        }));
                    }
                    Some("local_shell_call") => {
                        let call_id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .ok_or_else(AttemptFailure::invalid_request)?;
                        tool_calls.push(json!({
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "name": "shell",
                                "arguments": response_tool_arguments(item.get("action"))?,
                            }
                        }));
                    }
                    Some("function_call_output" | "custom_tool_call_output") => {
                        flush_chat_tool_calls(&mut messages, &mut tool_calls);
                        let call_id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .ok_or_else(AttemptFailure::invalid_request)?;
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": response_tool_output_text(
                                item.get("output").ok_or_else(AttemptFailure::invalid_request)?
                            )?,
                        }));
                    }
                    Some("tool_search_output") => {
                        flush_chat_tool_calls(&mut messages, &mut tool_calls);
                        let call_id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .ok_or_else(AttemptFailure::invalid_request)?;
                        let content = item
                            .get("output")
                            .map(response_tool_output_text)
                            .transpose()?
                            .unwrap_or_else(|| serde_json::to_string(item).unwrap_or_default());
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": content,
                        }));
                    }
                    Some("reasoning") => {}
                    _ => return Err(AttemptFailure::invalid_request()),
                }
            }
            flush_chat_tool_calls(&mut messages, &mut tool_calls);
            (!messages.is_empty())
                .then_some(messages)
                .ok_or_else(AttemptFailure::invalid_request)
        }
        _ => Err(AttemptFailure::invalid_request()),
    }
}

fn response_tool_arguments(value: Option<&Value>) -> Result<String, AttemptFailure> {
    match value {
        Some(Value::String(value)) => Ok(value.to_string()),
        Some(value) => serde_json::to_string(value).map_err(|_| AttemptFailure::invalid_request()),
        None => Ok("{}".to_string()),
    }
}

fn response_tool_output_text(value: &Value) -> Result<String, AttemptFailure> {
    match value {
        Value::String(value) => Ok(value.to_string()),
        Value::Array(parts) => {
            let mut output = String::new();
            for part in parts {
                if let Some(text) = part.as_str() {
                    output.push_str(text);
                    continue;
                }
                match part.get("type").and_then(Value::as_str) {
                    Some("output_text" | "input_text" | "text") => output.push_str(
                        part.get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(AttemptFailure::invalid_request)?,
                    ),
                    Some("refusal") => output.push_str(
                        part.get("refusal")
                            .and_then(Value::as_str)
                            .ok_or_else(AttemptFailure::invalid_request)?,
                    ),
                    // Chat Completions has no portable tool-result image or
                    // encrypted-content shape. Keep the turn structurally valid
                    // without exposing the opaque payload as plain text.
                    Some("input_image") => output.push_str("[image tool output omitted]"),
                    Some("encrypted_content") => output.push_str("[encrypted tool output omitted]"),
                    _ => return Err(AttemptFailure::invalid_request()),
                }
            }
            Ok(output)
        }
        _ => Err(AttemptFailure::invalid_request()),
    }
}

fn is_message_content(item: &Value) -> bool {
    item.as_str().is_some()
        || matches!(
            item.get("type").and_then(Value::as_str),
            Some("input_text" | "output_text" | "text" | "input_image")
        )
}

fn flush_chat_tool_calls(messages: &mut Vec<Value>, tool_calls: &mut Vec<Value>) {
    if !tool_calls.is_empty() {
        messages.push(json!({
            "role": "assistant",
            "content": Value::Null,
            "tool_calls": std::mem::take(tool_calls),
        }));
    }
}

pub(super) fn translate_chat_request(
    request: &Value,
    model: &str,
    stream: bool,
) -> Result<Vec<u8>, AttemptFailure> {
    let object = request
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(AttemptFailure::invalid_request)?;
    let mut input = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(AttemptFailure::invalid_request)?;
        if role == "tool" {
            let call_id = message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::invalid_request)?;
            let output = message
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::invalid_request)?;
            input.push(json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output,
            }));
            continue;
        }
        if !matches!(role, "developer" | "system" | "user" | "assistant") {
            return Err(AttemptFailure::invalid_request());
        }
        if let Some(content) = message.get("content").filter(|content| !content.is_null()) {
            input.push(json!({
                "role": role,
                "content": translate_chat_message_content(content)?,
            }));
        }
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in tool_calls {
                let function = call
                    .get("function")
                    .and_then(Value::as_object)
                    .ok_or_else(AttemptFailure::invalid_request)?;
                input.push(json!({
                    "type": "function_call",
                    "call_id": call.get("id").and_then(Value::as_str).ok_or_else(AttemptFailure::invalid_request)?,
                    "name": function.get("name").and_then(Value::as_str).ok_or_else(AttemptFailure::invalid_request)?,
                    "arguments": function.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
                }));
            }
        }
    }
    if input.is_empty() {
        return Err(AttemptFailure::invalid_request());
    }

    let mut translated = serde_json::Map::from_iter([
        ("model".to_string(), Value::String(model.to_string())),
        ("input".to_string(), Value::Array(input)),
        ("stream".to_string(), Value::Bool(stream)),
    ]);
    for field in [
        "temperature",
        "top_p",
        "parallel_tool_calls",
        "service_tier",
    ] {
        if let Some(value) = object.get(field) {
            translated.insert(field.to_string(), value.clone());
        }
    }
    if let Some(value) = object
        .get("max_completion_tokens")
        .or_else(|| object.get("max_tokens"))
    {
        translated.insert("max_output_tokens".to_string(), value.clone());
    }
    if let Some(tools) = object.get("tools") {
        translated.insert("tools".to_string(), translate_chat_tools(tools)?);
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        translated.insert(
            "tool_choice".to_string(),
            translate_chat_tool_choice(tool_choice)?,
        );
    }
    if let Some(effort) = object.get("reasoning_effort").and_then(Value::as_str) {
        translated.insert("reasoning".to_string(), json!({ "effort": effort }));
    }
    serde_json::to_vec(&Value::Object(translated)).map_err(|_| AttemptFailure::invalid_request())
}

fn translate_chat_message_content(content: &Value) -> Result<Value, AttemptFailure> {
    match content {
        Value::String(_) => Ok(content.clone()),
        Value::Array(items) => items
            .iter()
            .map(|item| match item.get("type").and_then(Value::as_str) {
                Some("text") => item
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| json!({"type": "input_text", "text": text}))
                    .ok_or_else(AttemptFailure::invalid_request),
                Some("image_url") => item
                    .get("image_url")
                    .and_then(|image| image.get("url"))
                    .and_then(Value::as_str)
                    .map(|url| json!({"type": "input_image", "image_url": url}))
                    .ok_or_else(AttemptFailure::invalid_request),
                _ => Err(AttemptFailure::invalid_request()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Err(AttemptFailure::invalid_request()),
    }
}

fn translate_chat_tools(tools: &Value) -> Result<Value, AttemptFailure> {
    let tools = tools
        .as_array()
        .ok_or_else(AttemptFailure::invalid_request)?;
    tools
        .iter()
        .map(|tool| {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(AttemptFailure::invalid_request());
            }
            let function = tool
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(AttemptFailure::invalid_request)?;
            let mut translated = serde_json::Map::from_iter([
                ("type".to_string(), Value::String("function".to_string())),
                (
                    "name".to_string(),
                    function
                        .get("name")
                        .cloned()
                        .ok_or_else(AttemptFailure::invalid_request)?,
                ),
            ]);
            for field in ["description", "parameters", "strict"] {
                if let Some(value) = function.get(field) {
                    translated.insert(field.to_string(), value.clone());
                }
            }
            Ok(Value::Object(translated))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn translate_chat_tool_choice(tool_choice: &Value) -> Result<Value, AttemptFailure> {
    if tool_choice.is_string() {
        return Ok(tool_choice.clone());
    }
    let name = tool_choice
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(AttemptFailure::invalid_request)?;
    Ok(json!({"type": "function", "name": name}))
}

fn translate_message_content(content: &Value) -> Result<Value, AttemptFailure> {
    match content {
        Value::String(_) => Ok(content.clone()),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if let Some(text) = item.as_str() {
                    return Ok(json!({"type": "text", "text": text}));
                }
                let kind = item
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(AttemptFailure::invalid_request)?;
                match kind {
                    "input_text" | "output_text" | "text" => item
                        .get("text")
                        .and_then(Value::as_str)
                        .map(|text| json!({"type": "text", "text": text}))
                        .ok_or_else(AttemptFailure::invalid_request),
                    "input_image" => item
                        .get("image_url")
                        .or_else(|| item.get("url"))
                        .and_then(Value::as_str)
                        .map(|url| json!({"type": "image_url", "image_url": {"url": url}}))
                        .ok_or_else(AttemptFailure::invalid_request),
                    _ => Err(AttemptFailure::invalid_request()),
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Err(AttemptFailure::invalid_request()),
    }
}

fn translate_responses_tools(
    object: &serde_json::Map<String, Value>,
) -> Result<Vec<Value>, AttemptFailure> {
    let mut source_tools = Vec::new();
    if let Some(value) = object.get("tools") {
        source_tools.extend(
            value
                .as_array()
                .ok_or_else(AttemptFailure::invalid_request)?
                .iter(),
        );
    }
    if let Some(input) = object.get("input").and_then(Value::as_array) {
        for item in input {
            if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
                continue;
            }
            source_tools.extend(
                item.get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(AttemptFailure::invalid_request)?
                    .iter(),
            );
        }
    }

    let mut translated = Vec::new();
    let mut seen_names = HashSet::new();
    for tool in source_tools {
        append_responses_tool(tool, None, &mut translated, &mut seen_names)?;
    }
    Ok(translated)
}

fn append_responses_tool(
    tool: &Value,
    namespace: Option<&str>,
    translated: &mut Vec<Value>,
    seen_names: &mut HashSet<String>,
) -> Result<(), AttemptFailure> {
    let object = tool
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    let tool_type = object.get("type").and_then(Value::as_str);
    if tool_type.is_some_and(chat_source_hosted_tool_type) || tool_is_server_executed(object) {
        return Ok(());
    }
    match tool_type {
        Some("function") => append_function_tool(tool, namespace, translated, seen_names),
        Some("namespace") => {
            let namespace = object
                .get("name")
                .or_else(|| object.get("namespace"))
                .and_then(Value::as_str)
                .filter(|value| valid_response_tool_identifier(value))
                .ok_or_else(AttemptFailure::invalid_request)?;
            let tools = object
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(AttemptFailure::invalid_request)?;
            for tool in tools {
                let child = tool
                    .as_object()
                    .ok_or_else(AttemptFailure::invalid_request)?;
                let child_type = child.get("type").and_then(Value::as_str);
                if child_type.is_some_and(chat_source_hosted_tool_type)
                    || tool_is_server_executed(child)
                {
                    continue;
                }
                match child_type {
                    // Codex has used both the full Responses function shape and
                    // the compact namespace-child shape without a redundant
                    // `type: "function"` field. Both describe a function.
                    None | Some("function") => {
                        append_function_tool(tool, Some(namespace), translated, seen_names)?;
                    }
                    // A Chat Completions namespace is represented by flattened
                    // functions; silently omitting another nested tool type
                    // would make the client believe a tool is available when it
                    // is not routable.
                    Some(_) => return Err(AttemptFailure::invalid_request()),
                }
            }
            Ok(())
        }
        Some("custom") => append_custom_tool(tool, translated, seen_names),
        Some("tool_search") => append_tool_search_tool(tool, translated, seen_names),
        // Preserve a future named client-side tool as a function instead of
        // silently removing it from the model's tool catalog.
        Some(_) if object.get("name").and_then(Value::as_str).is_some() => {
            append_function_tool(tool, namespace, translated, seen_names)
        }
        Some(_) | None => Ok(()),
    }
}

fn tool_is_server_executed(tool: &serde_json::Map<String, Value>) -> bool {
    tool.get("execution")
        .and_then(Value::as_str)
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("server"))
}

fn chat_source_hosted_tool_type(tool_type: &str) -> bool {
    let tool_type = tool_type.trim().to_ascii_lowercase();
    [
        "web_search",
        "web_search_preview",
        "image_generation",
        "image_gen",
        "file_search",
        "computer_use_preview",
        "code_interpreter",
        "mcp",
    ]
    .iter()
    .any(|hosted| tool_type == *hosted)
        || [
            "web_search_",
            "image_generation_",
            "file_search_",
            "computer_use_",
            "code_interpreter_",
            "mcp_",
        ]
        .iter()
        .any(|prefix| tool_type.starts_with(prefix))
}

fn append_function_tool(
    tool: &Value,
    namespace: Option<&str>,
    translated: &mut Vec<Value>,
    seen_names: &mut HashSet<String>,
) -> Result<(), AttemptFailure> {
    let source = tool
        .get("function")
        .and_then(Value::as_object)
        .or_else(|| tool.as_object())
        .ok_or_else(AttemptFailure::invalid_request)?;
    let name = source
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| valid_response_tool_identifier(value))
        .ok_or_else(AttemptFailure::invalid_request)?;
    let wire_name = chat_function_wire_name(namespace, name)?;
    let mut function =
        serde_json::Map::from_iter([("name".to_string(), Value::String(wire_name.clone()))]);
    copy_function_tool_fields(source, &mut function);
    push_chat_function_tool(wire_name, function, translated, seen_names)
}

fn append_custom_tool(
    tool: &Value,
    translated: &mut Vec<Value>,
    seen_names: &mut HashSet<String>,
) -> Result<(), AttemptFailure> {
    let object = tool
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| valid_response_tool_identifier(value))
        .ok_or_else(AttemptFailure::invalid_request)?;
    let wire_name = chat_custom_tool_wire_name(name)?;
    let mut function =
        serde_json::Map::from_iter([("name".to_string(), Value::String(wire_name.clone()))]);
    if let Some(description) = object.get("description").and_then(Value::as_str) {
        function.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
    }
    function.insert(
        "parameters".to_string(),
        json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "Raw input for the custom tool."
                }
            },
            "required": ["input"],
            "additionalProperties": false
        }),
    );
    push_chat_function_tool(wire_name, function, translated, seen_names)
}

fn append_tool_search_tool(
    tool: &Value,
    translated: &mut Vec<Value>,
    seen_names: &mut HashSet<String>,
) -> Result<(), AttemptFailure> {
    let object = tool
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    let mut function = serde_json::Map::from_iter([(
        "name".to_string(),
        Value::String(CHAT_TOOL_SEARCH_NAME.to_string()),
    )]);
    if let Some(description) = object.get("description").and_then(Value::as_str) {
        function.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
    }
    function.insert(
        "parameters".to_string(),
        object.get("parameters").cloned().unwrap_or_else(|| {
            json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            })
        }),
    );
    push_chat_function_tool(
        CHAT_TOOL_SEARCH_NAME.to_string(),
        function,
        translated,
        seen_names,
    )
}

fn copy_function_tool_fields(
    source: &serde_json::Map<String, Value>,
    target: &mut serde_json::Map<String, Value>,
) {
    if let Some(description) = source.get("description").and_then(Value::as_str) {
        target.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
    }
    if let Some(parameters) = source.get("parameters").filter(|value| value.is_object()) {
        target.insert("parameters".to_string(), parameters.clone());
    }
    if let Some(strict) = source.get("strict").filter(|value| value.is_boolean()) {
        target.insert("strict".to_string(), strict.clone());
    }
}

fn push_chat_function_tool(
    wire_name: String,
    function: serde_json::Map<String, Value>,
    translated: &mut Vec<Value>,
    seen_names: &mut HashSet<String>,
) -> Result<(), AttemptFailure> {
    if !seen_names.insert(wire_name) {
        return Err(AttemptFailure::invalid_request());
    }
    translated.push(json!({"type": "function", "function": function}));
    Ok(())
}

fn translate_responses_tool_choice(
    tool_choice: &Value,
    tools: &mut Vec<Value>,
) -> Result<Option<Value>, AttemptFailure> {
    if let Some(choice) = tool_choice.as_str() {
        return matches!(choice, "auto" | "none" | "required")
            .then(|| Some(Value::String(choice.to_string())))
            .ok_or_else(AttemptFailure::invalid_request);
    }
    let object = tool_choice
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    if object.get("type").and_then(Value::as_str) == Some("allowed_tools") {
        let mode = object
            .get("mode")
            .and_then(Value::as_str)
            .filter(|value| matches!(*value, "auto" | "required"))
            .ok_or_else(AttemptFailure::invalid_request)?;
        let allowed = object
            .get("tools")
            .or_else(|| object.get("allowed_tools"))
            .and_then(Value::as_array)
            .ok_or_else(AttemptFailure::invalid_request)?;
        let mut allowed_names = HashSet::new();
        for tool in allowed {
            // Codex can include server-hosted tools in this selector alongside
            // its local tools. Those tools cannot cross a stateless Chat
            // Completions source, but they must not disable the compatible
            // local entries in the same request.
            match response_tool_choice_wire_names(tool, tools) {
                Ok(names) => allowed_names.extend(names),
                Err(_) if response_tool_choice_is_hosted(tool) => {}
                Err(failure) => return Err(failure),
            }
        }
        tools.retain(|tool| {
            tool.get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .is_some_and(|name| allowed_names.contains(name))
        });
        return (!tools.is_empty())
            .then(|| Some(Value::String(mode.to_string())))
            .ok_or_else(AttemptFailure::invalid_request);
    }
    let choice_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(AttemptFailure::invalid_request)?;
    let wire_names = response_tool_choice_wire_names(tool_choice, tools)?;
    if choice_type == "namespace" {
        let allowed_names = wire_names.into_iter().collect::<HashSet<_>>();
        tools.retain(|tool| {
            tool.get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .is_some_and(|name| allowed_names.contains(name))
        });
        return (!tools.is_empty())
            .then(|| Some(Value::String("required".to_string())))
            .ok_or_else(AttemptFailure::invalid_request);
    }
    let wire_name = wire_names
        .into_iter()
        .find(|candidate| {
            tools.iter().any(|tool| {
                tool.get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    == Some(candidate.as_str())
            })
        })
        .ok_or_else(AttemptFailure::invalid_request)?;
    Ok(Some(
        json!({"type": "function", "function": {"name": wire_name}}),
    ))
}

fn response_tool_choice_is_hosted(tool: &Value) -> bool {
    tool.as_object().is_some_and(|tool| {
        tool.get("type")
            .and_then(Value::as_str)
            .is_some_and(chat_source_hosted_tool_type)
            || tool_is_server_executed(tool)
    })
}

fn response_tool_choice_wire_names(
    tool: &Value,
    available_tools: &[Value],
) -> Result<Vec<String>, AttemptFailure> {
    let object = tool
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(AttemptFailure::invalid_request)?;
    let name = object
        .get("name")
        .or_else(|| {
            object
                .get("function")
                .and_then(|function| function.get("name"))
        })
        .and_then(Value::as_str)
        .filter(|value| valid_response_tool_identifier(value));
    match kind {
        "function" => {
            let name = name.ok_or_else(AttemptFailure::invalid_request)?;
            let namespace = object.get("namespace").and_then(Value::as_str);
            let mut names = vec![chat_function_wire_name(namespace, name)?];
            if namespace.is_none() {
                if let Some((namespace, name)) = name.rsplit_once('.') {
                    if valid_response_tool_identifier(namespace)
                        && valid_response_tool_identifier(name)
                    {
                        names.push(chat_function_wire_name(Some(namespace), name)?);
                    }
                }
            }
            Ok(names)
        }
        "custom" => name
            .map(chat_custom_tool_wire_name)
            .transpose()?
            .map(|name| vec![name])
            .ok_or_else(AttemptFailure::invalid_request),
        "tool_search" => Ok(vec![CHAT_TOOL_SEARCH_NAME.to_string()]),
        "namespace" => {
            let namespace = name.ok_or_else(AttemptFailure::invalid_request)?;
            let matching = available_tools
                .iter()
                .filter_map(|tool| {
                    let wire_name = tool.pointer("/function/name").and_then(Value::as_str)?;
                    match decode_chat_tool_call_kind(wire_name) {
                        ChatToolCallKind::Function {
                            namespace: Some(tool_namespace),
                            ..
                        } if tool_namespace == namespace => Some(wire_name.to_string()),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>();
            (!matching.is_empty())
                .then_some(matching)
                .ok_or_else(AttemptFailure::invalid_request)
        }
        _ => Err(AttemptFailure::invalid_request()),
    }
}

fn valid_response_tool_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn chat_function_wire_name(namespace: Option<&str>, name: &str) -> Result<String, AttemptFailure> {
    if !valid_response_tool_identifier(name) {
        return Err(AttemptFailure::invalid_request());
    }
    let wire_name = if let Some(namespace) = namespace.filter(|value| !value.is_empty()) {
        if !valid_response_tool_identifier(namespace) {
            return Err(AttemptFailure::invalid_request());
        }
        let encoded_namespace = URL_SAFE_NO_PAD.encode(namespace.as_bytes());
        let encoded_name = URL_SAFE_NO_PAD.encode(name.as_bytes());
        format!(
            "{CHAT_NAMESPACE_TOOL_PREFIX}{:x}_{encoded_namespace}{encoded_name}",
            encoded_namespace.len()
        )
    } else if valid_chat_function_name(name) && !reserved_chat_tool_name(name) {
        name.to_string()
    } else {
        format!(
            "{CHAT_ENCODED_FUNCTION_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(name.as_bytes())
        )
    };
    valid_chat_function_name(&wire_name)
        .then_some(wire_name)
        .ok_or_else(AttemptFailure::invalid_request)
}

fn chat_custom_tool_wire_name(name: &str) -> Result<String, AttemptFailure> {
    if !valid_response_tool_identifier(name) {
        return Err(AttemptFailure::invalid_request());
    }
    let wire_name = format!(
        "{CHAT_CUSTOM_TOOL_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(name.as_bytes())
    );
    valid_chat_function_name(&wire_name)
        .then_some(wire_name)
        .ok_or_else(AttemptFailure::invalid_request)
}

fn valid_chat_function_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_CHAT_FUNCTION_NAME_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn reserved_chat_tool_name(name: &str) -> bool {
    name == CHAT_TOOL_SEARCH_NAME
        || name.starts_with(CHAT_NAMESPACE_TOOL_PREFIX)
        || name.starts_with(CHAT_CUSTOM_TOOL_PREFIX)
        || name.starts_with(CHAT_ENCODED_FUNCTION_PREFIX)
}

enum ChatToolCallKind {
    Function {
        name: String,
        namespace: Option<String>,
    },
    Custom {
        name: String,
    },
    ToolSearch,
}

fn decode_chat_tool_call_kind(wire_name: &str) -> ChatToolCallKind {
    if wire_name == CHAT_TOOL_SEARCH_NAME {
        return ChatToolCallKind::ToolSearch;
    }
    if let Some(encoded) = wire_name.strip_prefix(CHAT_CUSTOM_TOOL_PREFIX) {
        if let Some(name) = decode_chat_tool_component(encoded) {
            return ChatToolCallKind::Custom { name };
        }
    }
    if let Some(encoded) = wire_name.strip_prefix(CHAT_ENCODED_FUNCTION_PREFIX) {
        if let Some(name) = decode_chat_tool_component(encoded) {
            return ChatToolCallKind::Function {
                name,
                namespace: None,
            };
        }
    }
    if let Some(rest) = wire_name.strip_prefix(CHAT_NAMESPACE_TOOL_PREFIX) {
        if let Some((length, encoded)) = rest.split_once('_') {
            if let Ok(length) = usize::from_str_radix(length, 16) {
                if encoded.len() > length {
                    let (namespace, name) = encoded.split_at(length);
                    if let (Some(namespace), Some(name)) = (
                        decode_chat_tool_component(namespace),
                        decode_chat_tool_component(name),
                    ) {
                        return ChatToolCallKind::Function {
                            name,
                            namespace: Some(namespace),
                        };
                    }
                }
            }
        }
    }
    ChatToolCallKind::Function {
        name: wire_name.to_string(),
        namespace: None,
    }
}

fn decode_chat_tool_component(encoded: &str) -> Option<String> {
    let decoded = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    let value = String::from_utf8(decoded).ok()?;
    valid_response_tool_identifier(&value).then_some(value)
}

pub(super) fn translate_chat_response(
    body: &[u8],
    fallback_response_id: &str,
) -> Result<Vec<u8>, AttemptFailure> {
    let response: Value =
        serde_json::from_slice(body).map_err(|_| AttemptFailure::translation())?;
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(AttemptFailure::translation)?;
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(AttemptFailure::translation)?;
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or(fallback_response_id);
    let model = response
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut output = Vec::new();
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        output.push(json!({
            "id": format!("{id}-message"),
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "output_text", "text": content, "annotations": []}],
        }));
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        let choice_index = choice.get("index").and_then(Value::as_u64).unwrap_or(0);
        for (tool_index, call) in tool_calls.iter().enumerate() {
            let function = call
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(AttemptFailure::translation)?;
            let call_id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("call_{id}_{choice_index}_{tool_index}"));
            let wire_name = function
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::translation)?;
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            match decode_chat_tool_call_kind(wire_name) {
                ChatToolCallKind::Function { name, namespace } => {
                    let mut item = json!({
                        "id": call_id,
                        "type": "function_call",
                        "status": "completed",
                        "call_id": call_id,
                        "name": name,
                        "arguments": arguments,
                    });
                    if let Some(namespace) = namespace {
                        item["namespace"] = Value::String(namespace);
                    }
                    output.push(item);
                }
                ChatToolCallKind::Custom { name } => output.push(json!({
                    "id": call_id,
                    "type": "custom_tool_call",
                    "status": "completed",
                    "call_id": call_id,
                    "name": name,
                    "input": custom_tool_input(arguments),
                })),
                ChatToolCallKind::ToolSearch => output.push(json!({
                    "id": call_id,
                    "type": "tool_search_call",
                    "status": "completed",
                    "call_id": call_id,
                    "arguments": serde_json::from_str::<Value>(arguments)
                        .unwrap_or_else(|_| Value::String(arguments.to_string())),
                })),
            }
        }
    }
    if output.is_empty() {
        return Err(AttemptFailure::translation());
    }
    let usage = response.get("usage").map(|usage| {
        json!({
            "input_tokens": usage.get("prompt_tokens").or_else(|| usage.get("input_tokens")).and_then(Value::as_u64).unwrap_or(0),
            "output_tokens": usage.get("completion_tokens").or_else(|| usage.get("output_tokens")).and_then(Value::as_u64).unwrap_or(0),
            "total_tokens": usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        })
    });
    serde_json::to_vec(&json!({
        "id": id,
        "object": "response",
        "created_at": response.get("created").and_then(Value::as_u64).unwrap_or(0),
        "status": "completed",
        "model": model,
        "output": output,
        "usage": usage,
    }))
    .map_err(|_| AttemptFailure::translation())
}

fn custom_tool_input(arguments: &str) -> String {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|arguments| {
            arguments
                .get("input")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| arguments.to_string())
}

pub(super) fn translate_responses_response(body: &[u8]) -> Result<Vec<u8>, AttemptFailure> {
    let response: Value =
        serde_json::from_slice(body).map_err(|_| AttemptFailure::translation())?;
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(AttemptFailure::translation)?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if matches!(
                            part.get("type").and_then(Value::as_str),
                            Some("output_text" | "text")
                        ) {
                            if let Some(value) = part.get("text").and_then(Value::as_str) {
                                text.push_str(value);
                            }
                        }
                    }
                }
            }
            Some("function_call") => {
                let call_id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(AttemptFailure::translation)?;
                tool_calls.push(json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": item.get("name").and_then(Value::as_str).ok_or_else(AttemptFailure::translation)?,
                        "arguments": item.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
                    }
                }));
            }
            _ => {}
        }
    }
    if text.is_empty() && tool_calls.is_empty() {
        return Err(AttemptFailure::translation());
    }
    let mut message = serde_json::Map::from_iter([
        ("role".to_string(), Value::String("assistant".to_string())),
        (
            "content".to_string(),
            if text.is_empty() {
                Value::Null
            } else {
                Value::String(text)
            },
        ),
    ]);
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    let usage = response.get("usage").map(|usage| {
        json!({
            "prompt_tokens": usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            "completion_tokens": usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            "total_tokens": usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        })
    });
    serde_json::to_vec(&json!({
        "id": response.get("id").and_then(Value::as_str).unwrap_or("response"),
        "object": "chat.completion",
        "created": response.get("created_at").and_then(Value::as_u64).unwrap_or(0),
        "model": response.get("model").and_then(Value::as_str).unwrap_or("unknown"),
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": if response.get("status").and_then(Value::as_str) == Some("incomplete") { "length" } else if response.get("output").and_then(Value::as_array).is_some_and(|output| output.iter().any(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))) { "tool_calls" } else { "stop" },
        }],
        "usage": usage,
    }))
    .map_err(|_| AttemptFailure::translation())
}

pub(super) fn completed_sse(response: &[u8]) -> Bytes {
    let response = serde_json::from_slice::<Value>(response).unwrap_or(Value::Null);
    Bytes::from(format!(
        "data: {}\n\n",
        json!({"type": "response.completed", "response": response})
    ))
}

pub(super) fn completed_chat_sse(response: &[u8]) -> Bytes {
    let Ok(response) = serde_json::from_slice::<Value>(response) else {
        return Bytes::new();
    };
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .cloned()
        .unwrap_or(Value::Null);
    let common = json!({
        "id": response.get("id").cloned().unwrap_or(Value::Null),
        "object": "chat.completion.chunk",
        "created": response.get("created").cloned().unwrap_or(Value::from(0)),
        "model": response.get("model").cloned().unwrap_or(Value::Null),
    });
    let mut first = common.clone();
    first["choices"] = json!([{
        "index": 0,
        "delta": choice.get("message").cloned().unwrap_or(Value::Null),
        "finish_reason": Value::Null,
    }]);
    let mut terminal = common;
    terminal["choices"] = json!([{
        "index": 0,
        "delta": {},
        "finish_reason": choice.get("finish_reason").cloned().unwrap_or(Value::String("stop".to_string())),
    }]);
    terminal["usage"] = response.get("usage").cloned().unwrap_or(Value::Null);
    Bytes::from(format!(
        "data: {first}\n\ndata: {terminal}\n\ndata: [DONE]\n\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_translation_synthesizes_missing_tool_call_ids() {
        let translated = translate_chat_response(
            br#"{"id":"chatcmpl_1","model":"model","choices":[{"index":2,"message":{"tool_calls":[{"type":"function","function":{"name":"lookup","arguments":"{}"}}]}}]}"#,
            "relay-fallback",
        )
        .unwrap_or_else(|_| panic!("chat response should translate"));
        let response: Value = serde_json::from_slice(&translated).unwrap();

        assert_eq!(
            response.pointer("/output/0/id").unwrap(),
            "call_chatcmpl_1_2_0"
        );
        assert_eq!(
            response.pointer("/output/0/call_id").unwrap(),
            "call_chatcmpl_1_2_0"
        );
    }

    #[test]
    fn chat_translation_uses_request_scoped_response_fallback_id() {
        let translated = translate_chat_response(
            br#"{"model":"model","choices":[{"message":{"content":"done"}}]}"#,
            "relay-request-chat-2",
        )
        .unwrap_or_else(|_| panic!("Chat response should translate"));
        let response: Value = serde_json::from_slice(&translated).unwrap();

        assert_eq!(response["id"], "relay-request-chat-2");
    }

    #[test]
    fn request_translation_preserves_reasoning_effort() {
        let chat = translate_responses_request(
            &json!({ "input": "hello", "reasoning": { "effort": "high" } }),
            "model",
            false,
        )
        .unwrap_or_else(|_| panic!("Responses request should translate"));
        assert_eq!(
            serde_json::from_slice::<Value>(&chat).unwrap()["reasoning_effort"],
            "high"
        );

        let responses = translate_chat_request(
            &json!({
                "messages": [{ "role": "user", "content": "hello" }],
                "reasoning_effort": "low"
            }),
            "model",
            false,
        )
        .unwrap_or_else(|_| panic!("Chat request should translate"));
        assert_eq!(
            serde_json::from_slice::<Value>(&responses).unwrap()["reasoning"]["effort"],
            "low"
        );
    }

    #[test]
    fn responses_tool_history_translates_to_chat_messages() {
        let translated = translate_responses_request(
            &json!({
                "input": [
                    {"role": "user", "content": [{"type": "input_text", "text": "inspect"}]},
                    {"type": "reasoning", "encrypted_content": "opaque"},
                    {"type": "function_call", "call_id": "call_1", "name": "shell", "arguments": "{\"command\":\"pwd\"}"},
                    {"type": "function_call_output", "call_id": "call_1", "output": "C:/work"},
                    {"role": "user", "content": [{"type": "input_text", "text": "continue"}]}
                ],
                "tools": [{"type": "function", "name": "shell", "parameters": {"type": "object"}}],
                "tool_choice": "auto"
            }),
            "model",
            false,
        )
        .unwrap_or_else(|_| panic!("Responses tool history should translate"));
        let request: Value = serde_json::from_slice(&translated).unwrap();

        assert_eq!(request["messages"][1]["role"], "assistant");
        assert_eq!(
            request["messages"][1]["tool_calls"][0]["function"]["name"],
            "shell"
        );
        assert_eq!(request["messages"][2]["role"], "tool");
        assert_eq!(request["messages"][2]["tool_call_id"], "call_1");
        assert_eq!(request["messages"][3]["content"][0]["text"], "continue");
        assert_eq!(request["tools"][0]["function"]["name"], "shell");
        assert_eq!(request["tool_choice"], "auto");
    }

    #[test]
    fn responses_replay_restores_tool_call_context_for_chat_sources() {
        let initial = json!({
            "model": "vendor/model",
            "instructions": "Use the available tools.",
            "input": [{"role": "user", "content": [{"type": "input_text", "text": "inspect"}]}],
            "tools": [{"type": "function", "name": "shell", "parameters": {"type": "object"}}],
            "tool_choice": "auto",
        });
        let initial_response = json!({
            "id": "resp_tool_1",
            "output": [{
                "type": "function_call",
                "call_id": "call_shell",
                "name": "shell",
                "arguments": "{\"command\":\"pwd\"}"
            }]
        });
        let continuation = json!({
            "model": "vendor/model",
            "previous_response_id": "resp_tool_1",
            "input": [{
                "type": "function_call_output",
                "call_id": "call_shell",
                "output": "C:/work"
            }]
        });

        let replay = responses_replay_seed(&initial, &initial_response)
            .unwrap_or_else(|_| panic!("first tool response should be replayable"));
        let replay = replay_responses_request(&replay, &continuation)
            .unwrap_or_else(|_| panic!("tool output should attach to the replay state"));
        assert!(replay.get("previous_response_id").is_none());
        assert_eq!(replay["input"].as_array().unwrap().len(), 3);

        let translated = translate_responses_request(&replay, "vendor/model", false)
            .unwrap_or_else(|_| panic!("chat request"));
        let translated: Value = serde_json::from_slice(&translated).unwrap();
        let messages = translated["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["tool_calls"][0]["id"], "call_shell");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "call_shell");
        assert_eq!(messages[3]["content"], "C:/work");
    }

    #[test]
    fn codex_namespace_custom_and_tool_search_calls_round_trip_over_chat() {
        let translated = translate_responses_request(
            &json!({
                "input": [
                    {"role": "user", "content": [{"type": "input_text", "text": "work"}]},
                    {"type": "function_call", "call_id": "call_ns", "namespace": "mcp__repo", "name": "read_file", "arguments": "{\"path\":\"README.md\"}"},
                    {"type": "function_call_output", "call_id": "call_ns", "output": "contents"},
                    {"type": "custom_tool_call", "call_id": "call_patch", "name": "apply_patch", "input": "*** Begin Patch\n*** End Patch"},
                    {"type": "custom_tool_call_output", "call_id": "call_patch", "output": "Done"},
                    {"type": "tool_search_call", "call_id": "call_search", "arguments": {"query": "terminal"}},
                    {"type": "tool_search_output", "call_id": "call_search", "tools": []}
                ],
                "tools": [
                    {"type": "namespace", "name": "mcp__repo", "tools": [
                        {"name": "read_file", "parameters": {"type": "object"}},
                        {"type": "web_search_preview_2025_03_11"}
                    ]},
                    {"type": "custom", "name": "apply_patch"},
                    {"type": "tool_search", "description": "Find a tool"},
                    {"type": "tool_search", "execution": "server"},
                    {"type": "web_search_preview_2025_03_11"}
                ],
                "tool_choice": {
                    "type": "allowed_tools",
                    "mode": "required",
                    "tools": [
                        {"type": "function", "namespace": "mcp__repo", "name": "read_file"},
                        {"type": "custom", "name": "apply_patch"},
                        {"type": "tool_search"},
                        {"type": "tool_search", "execution": "server"},
                        {"type": "web_search_preview_2025_03_11"}
                    ]
                },
                "parallel_tool_calls": true
            }),
            "model",
            false,
        )
        .unwrap_or_else(|_| panic!("Codex tools should translate to Chat Completions"));
        let request: Value = serde_json::from_slice(&translated).unwrap();
        let tools = request["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        assert_eq!(request["tool_choice"], "required");
        assert_eq!(request["parallel_tool_calls"], false);
        let namespace_wire = tools
            .iter()
            .filter_map(|tool| tool.pointer("/function/name").and_then(Value::as_str))
            .find(|name| name.starts_with(CHAT_NAMESPACE_TOOL_PREFIX))
            .expect("namespace tool should have a reversible Chat wire name")
            .to_string();
        let custom_wire = tools
            .iter()
            .filter_map(|tool| tool.pointer("/function/name").and_then(Value::as_str))
            .find(|name| name.starts_with(CHAT_CUSTOM_TOOL_PREFIX))
            .expect("custom tool should have a reversible Chat wire name")
            .to_string();
        assert_eq!(
            request["messages"][1]["tool_calls"][0]["function"]["name"],
            namespace_wire
        );
        assert_eq!(
            request["messages"][3]["tool_calls"][0]["function"]["name"],
            custom_wire
        );
        assert_eq!(
            request["messages"][5]["tool_calls"][0]["function"]["name"],
            CHAT_TOOL_SEARCH_NAME
        );

        let response = translate_chat_response(
            serde_json::to_string(&json!({
                "id": "chatcmpl_tools",
                "model": "model",
                "choices": [{
                    "message": {"tool_calls": [
                        {"id": "call_ns", "type": "function", "function": {"name": namespace_wire, "arguments": "{\"path\":\"README.md\"}"}},
                        {"id": "call_patch", "type": "function", "function": {"name": custom_wire, "arguments": "{\"input\":\"*** Begin Patch\\n*** End Patch\"}"}},
                        {"id": "call_search", "type": "function", "function": {"name": CHAT_TOOL_SEARCH_NAME, "arguments": "{\"query\":\"terminal\"}"}}
                    ]}
                }]
            }))
            .unwrap()
            .as_bytes(),
            "relay-fallback",
        )
        .unwrap_or_else(|_| panic!("Chat tool calls should translate back to Responses"));
        let response: Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(response["output"][0]["type"], "function_call");
        assert_eq!(response["output"][0]["namespace"], "mcp__repo");
        assert_eq!(response["output"][0]["name"], "read_file");
        assert_eq!(response["output"][1]["type"], "custom_tool_call");
        assert_eq!(response["output"][1]["name"], "apply_patch");
        assert_eq!(
            response["output"][1]["input"],
            "*** Begin Patch\n*** End Patch"
        );
        assert_eq!(response["output"][2]["type"], "tool_search_call");
        assert_eq!(response["output"][2]["arguments"]["query"], "terminal");
    }

    #[test]
    fn namespace_tool_choice_limits_chat_tools_to_that_namespace() {
        let translated = translate_responses_request(
            &json!({
                "input": "work",
                "tools": [
                    {"type": "namespace", "name": "mcp__repo", "tools": [
                        {"name": "read_file", "parameters": {"type": "object"}}
                    ]},
                    {"type": "function", "name": "shell", "parameters": {"type": "object"}}
                ],
                "tool_choice": {"type": "namespace", "name": "mcp__repo"}
            }),
            "model",
            false,
        )
        .unwrap_or_else(|_| panic!("namespace tool choice should translate"));
        let request: Value = serde_json::from_slice(&translated).unwrap();

        assert_eq!(request["tool_choice"], "required");
        assert_eq!(request["tools"].as_array().unwrap().len(), 1);
        assert!(request["tools"][0]["function"]["name"]
            .as_str()
            .is_some_and(|name| name.starts_with(CHAT_NAMESPACE_TOOL_PREFIX)));
    }
}
