//! Responses input items translated into Anthropic content blocks.

use super::{AdapterError, AdapterResult, MessagesBridgeState};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};

mod tools;

use tools::{append_assistant_tool_use, flush_tool_results, tool_result_block};

pub(super) fn append_system_value(
    state: &mut MessagesBridgeState,
    value: &Value,
) -> AdapterResult<()> {
    let text = content_to_messages_blocks(value)?;
    if text.is_empty() {
        return Ok(());
    }
    let next = Value::Array(text);
    match state.system.take() {
        None => state.system = Some(next),
        Some(Value::Array(mut current)) => {
            current.extend(next.as_array().into_iter().flatten().cloned());
            state.system = Some(Value::Array(current));
        }
        Some(_) => return Err(AdapterError::invalid_request()),
    }
    Ok(())
}

pub(super) fn append_responses_input(
    state: &mut MessagesBridgeState,
    input: &Value,
) -> AdapterResult<()> {
    match input {
        Value::String(text) => append_user_blocks(state, vec![text_block(text)]),
        Value::Array(items) => {
            let mut tool_results = Vec::new();
            for item in items {
                let item = item.as_object().ok_or_else(AdapterError::invalid_request)?;
                if matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("function_call_output" | "custom_tool_call_output")
                ) {
                    tool_results.push(tool_result_block(state, item)?);
                    continue;
                }
                flush_tool_results(state, &mut tool_results)?;
                if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                    // Tool definitions were collected before the Messages body was
                    // built. This Responses control item has no conversation
                    // equivalent and must not be emitted as a user message.
                    continue;
                }
                if let Some(role) = item.get("role").and_then(Value::as_str) {
                    match role {
                        "system" | "developer" => {
                            append_system_value(
                                state,
                                item.get("content")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?;
                        }
                        "user" => append_user_blocks(
                            state,
                            content_to_messages_blocks(
                                item.get("content")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?,
                        )?,
                        "assistant" => append_assistant_from_responses_item(state, item)?,
                        _ => return Err(AdapterError::invalid_request()),
                    }
                    continue;
                }
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call" | "custom_tool_call") => {
                        append_assistant_tool_use(state, item)?
                    }
                    Some("reasoning") => {
                        // A bridge continuation retains native thinking blocks locally. A
                        // standalone Responses reasoning item has no Anthropic signature and
                        // cannot be replayed safely.
                        if state.messages.is_empty() {
                            return Err(AdapterError::continuation_missing());
                        }
                    }
                    Some("message") => {
                        let role = item
                            .get("role")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        if role != "user" {
                            return Err(AdapterError::invalid_request());
                        }
                        append_user_blocks(
                            state,
                            content_to_messages_blocks(
                                item.get("content")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?,
                        )?;
                    }
                    _ => return Err(AdapterError::invalid_request()),
                }
            }
            flush_tool_results(state, &mut tool_results)
        }
        _ => Err(AdapterError::invalid_request()),
    }
}

fn append_assistant_from_responses_item(
    state: &mut MessagesBridgeState,
    item: &Map<String, Value>,
) -> AdapterResult<()> {
    let content = item
        .get("content")
        .map(content_to_messages_blocks)
        .transpose()?;
    if let Some(content) = content.filter(|content| !content.is_empty()) {
        state
            .messages
            .push(json!({"role": "assistant", "content": content}));
    }
    Ok(())
}

fn append_user_blocks(state: &mut MessagesBridgeState, blocks: Vec<Value>) -> AdapterResult<()> {
    if blocks.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    state
        .messages
        .push(json!({"role": "user", "content": blocks}));
    Ok(())
}

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

fn content_to_messages_blocks(content: &Value) -> AdapterResult<Vec<Value>> {
    match content {
        Value::String(text) if !text.is_empty() => Ok(vec![text_block(text)]),
        Value::String(_) => Ok(Vec::new()),
        Value::Array(parts) => parts
            .iter()
            .map(|part| {
                if let Some(text) = part.as_str() {
                    return Ok(text_block(text));
                }
                let part = part.as_object().ok_or_else(AdapterError::invalid_request)?;
                match part.get("type").and_then(Value::as_str) {
                    Some("input_text" | "output_text" | "text") => part
                        .get("text")
                        .and_then(Value::as_str)
                        .map(text_block)
                        .ok_or_else(AdapterError::invalid_request),
                    Some("input_image") => image_block_from_data_uri(
                        part.get("image_url")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?,
                    ),
                    _ => Err(AdapterError::invalid_request()),
                }
            })
            .collect(),
        _ => Err(AdapterError::invalid_request()),
    }
}

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/gif", "image/jpeg", "image/png", "image/webp"];

fn image_block_from_data_uri(data_uri: &str) -> AdapterResult<Value> {
    let data_uri = data_uri.trim();
    let Some(rest) = data_uri.strip_prefix("data:") else {
        return Err(AdapterError::invalid_request());
    };
    let Some((metadata, data)) = rest.split_once(',') else {
        return Err(AdapterError::invalid_request());
    };
    let mut metadata_parts = metadata.split(';');
    let media_type = metadata_parts
        .next()
        .map(str::trim)
        .filter(|value| {
            SUPPORTED_IMAGE_TYPES
                .iter()
                .any(|supported| value.eq_ignore_ascii_case(supported))
        })
        .ok_or_else(AdapterError::invalid_request)?
        .to_ascii_lowercase();
    if !metadata_parts.any(|part| part.trim().eq_ignore_ascii_case("base64")) {
        return Err(AdapterError::invalid_request());
    }
    let data = data.trim();
    if data.is_empty()
        || data.bytes().any(|byte| byte.is_ascii_whitespace())
        || data.len() > (MAX_IMAGE_BYTES * 4 / 3).saturating_add(4)
    {
        return Err(AdapterError::invalid_request());
    }
    let decoded = STANDARD
        .decode(data)
        .map_err(|_| AdapterError::invalid_request())?;
    if decoded.is_empty() || decoded.len() > MAX_IMAGE_BYTES {
        return Err(AdapterError::invalid_request());
    }

    Ok(json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": media_type,
            "data": data,
        }
    }))
}
