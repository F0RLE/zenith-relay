//! Responses input items translated into Anthropic content blocks.

use super::{AdapterError, AdapterResult, MessagesBridgeState};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};

mod tools;

use tools::{append_assistant_tool_use, flush_tool_results, tool_result_block};

pub(super) fn append_system_value(
    bridge_state: &mut MessagesBridgeState,
    system_value: &Value,
) -> AdapterResult<()> {
    let system_blocks = content_to_messages_blocks(system_value)?;
    if system_blocks.is_empty() {
        return Ok(());
    }
    let system_content = Value::Array(system_blocks);
    match bridge_state.system.take() {
        None => bridge_state.system = Some(system_content),
        Some(Value::Array(mut existing_system_blocks)) => {
            existing_system_blocks.extend(system_content.as_array().into_iter().flatten().cloned());
            bridge_state.system = Some(Value::Array(existing_system_blocks));
        }
        Some(_) => return Err(AdapterError::invalid_request()),
    }
    Ok(())
}

pub(super) fn append_responses_input(
    bridge_state: &mut MessagesBridgeState,
    responses_input: &Value,
) -> AdapterResult<()> {
    match responses_input {
        Value::String(text) => append_user_blocks(bridge_state, vec![text_block(text)]),
        Value::Array(input_items) => {
            let mut tool_results = Vec::new();
            for input_value in input_items {
                let response_item = input_value
                    .as_object()
                    .ok_or_else(AdapterError::invalid_request)?;
                if matches!(
                    response_item.get("type").and_then(Value::as_str),
                    Some("function_call_output" | "custom_tool_call_output")
                ) {
                    tool_results.push(tool_result_block(bridge_state, response_item)?);
                    continue;
                }
                flush_tool_results(bridge_state, &mut tool_results)?;
                if response_item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                    // Tool definitions were collected before the Messages body was
                    // built. This Responses control item has no conversation
                    // equivalent and must not be emitted as a user message.
                    continue;
                }
                if let Some(role) = response_item.get("role").and_then(Value::as_str) {
                    match role {
                        "system" | "developer" => {
                            append_system_value(
                                bridge_state,
                                response_item
                                    .get("content")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?;
                        }
                        "user" => append_user_blocks(
                            bridge_state,
                            content_to_messages_blocks(
                                response_item
                                    .get("content")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?,
                        )?,
                        "assistant" => {
                            append_assistant_from_responses_item(bridge_state, response_item)?
                        }
                        _ => return Err(AdapterError::invalid_request()),
                    }
                    continue;
                }
                match response_item.get("type").and_then(Value::as_str) {
                    Some("function_call" | "custom_tool_call") => {
                        append_assistant_tool_use(bridge_state, response_item)?
                    }
                    Some("reasoning") => {
                        // A bridge continuation retains native thinking blocks locally. A
                        // standalone Responses reasoning item has no Anthropic signature and
                        // cannot be replayed safely.
                        if bridge_state.messages.is_empty() {
                            return Err(AdapterError::continuation_missing());
                        }
                    }
                    Some("message") => {
                        let role = response_item
                            .get("role")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        if role != "user" {
                            return Err(AdapterError::invalid_request());
                        }
                        append_user_blocks(
                            bridge_state,
                            content_to_messages_blocks(
                                response_item
                                    .get("content")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?,
                        )?;
                    }
                    _ => return Err(AdapterError::invalid_request()),
                }
            }
            flush_tool_results(bridge_state, &mut tool_results)
        }
        _ => Err(AdapterError::invalid_request()),
    }
}

fn append_assistant_from_responses_item(
    bridge_state: &mut MessagesBridgeState,
    response_item: &Map<String, Value>,
) -> AdapterResult<()> {
    let content = response_item
        .get("content")
        .map(content_to_messages_blocks)
        .transpose()?;
    if let Some(content) = content.filter(|content| !content.is_empty()) {
        bridge_state
            .messages
            .push(json!({"role": "assistant", "content": content}));
    }
    Ok(())
}

fn append_user_blocks(
    bridge_state: &mut MessagesBridgeState,
    blocks: Vec<Value>,
) -> AdapterResult<()> {
    if blocks.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    bridge_state
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
            .map(|content_part| {
                if let Some(text_value) = content_part.as_str() {
                    return Ok(text_block(text_value));
                }
                let content_object = content_part
                    .as_object()
                    .ok_or_else(AdapterError::invalid_request)?;
                match content_object.get("type").and_then(Value::as_str) {
                    Some("input_text" | "output_text" | "text") => content_object
                        .get("text")
                        .and_then(Value::as_str)
                        .map(text_block)
                        .ok_or_else(AdapterError::invalid_request),
                    Some("input_image") => image_block_from_data_uri(
                        content_object
                            .get("image_url")
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

const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/gif", "image/jpeg", "image/png", "image/webp"];

fn image_block_from_data_uri(data_uri: &str) -> AdapterResult<Value> {
    let data_uri = data_uri.trim();
    let Some(rest) = data_uri.strip_prefix("data:") else {
        return Err(AdapterError::invalid_request());
    };
    let Some((metadata, encoded_data)) = rest.split_once(',') else {
        return Err(AdapterError::invalid_request());
    };
    let mut metadata_parts = metadata.split(';');
    let media_type = metadata_parts
        .next()
        .map(str::trim)
        .filter(|media_type| {
            SUPPORTED_IMAGE_TYPES
                .iter()
                .any(|supported| media_type.eq_ignore_ascii_case(supported))
        })
        .ok_or_else(AdapterError::invalid_request)?
        .to_ascii_lowercase();
    if !metadata_parts.any(|part| part.trim().eq_ignore_ascii_case("base64")) {
        return Err(AdapterError::invalid_request());
    }
    let encoded_data = encoded_data.trim();
    if encoded_data.is_empty() || encoded_data.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(AdapterError::invalid_request());
    }
    let decoded = STANDARD
        .decode(encoded_data)
        .map_err(|_| AdapterError::invalid_request())?;
    if decoded.is_empty() {
        return Err(AdapterError::invalid_request());
    }

    Ok(json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": media_type,
            "data": encoded_data,
        }
    }))
}
