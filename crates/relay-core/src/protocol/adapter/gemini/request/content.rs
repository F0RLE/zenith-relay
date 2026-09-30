//! Responses input items translated into Gemini content parts.

use super::{AdapterError, AdapterResult, MessagesBridgeState};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};

const MAX_INLINE_MEDIA_BYTES: usize = 20 * 1024 * 1024;

pub(super) fn append_responses_input(
    state: &mut MessagesBridgeState,
    input: &Value,
) -> AdapterResult<()> {
    match input {
        Value::String(text) => {
            if !text.is_empty() {
                append_message(state, "user", vec![json!({"text":text})]);
            }
            Ok(())
        }
        Value::Array(items) => {
            for value in items {
                let item = value
                    .as_object()
                    .ok_or_else(AdapterError::invalid_request)?;
                if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                    continue;
                }
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call_output" | "custom_tool_call_output") => {
                        let call_id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let name = find_call_name(state, call_id)
                            .or_else(|| item.get("name").and_then(Value::as_str))
                            .ok_or_else(AdapterError::invalid_request)?;
                        let response = if item.get("type").and_then(Value::as_str)
                            == Some("custom_tool_call_output")
                        {
                            json!({"input": output_text(item.get("output").ok_or_else(AdapterError::invalid_request)?)?})
                        } else {
                            output_value(
                                item.get("output")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?
                        };
                        append_message(
                            state,
                            "user",
                            vec![
                                json!({"functionResponse":{"name":name,"response":response,"id":call_id}}),
                            ],
                        );
                    }
                    Some("function_call" | "custom_tool_call") => {
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let namespace = item
                            .get("namespace")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty());
                        let upstream_name = state
                            .upstream_tool_name(namespace, name)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let call_id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let args = if item.get("type").and_then(Value::as_str)
                            == Some("custom_tool_call")
                        {
                            json!({"input": output_text(item.get("input").ok_or_else(AdapterError::invalid_request)?)?})
                        } else {
                            serde_json::from_str::<Value>(
                                item.get("arguments")
                                    .and_then(Value::as_str)
                                    .unwrap_or("{}"),
                            )
                            .map_err(|_| AdapterError::invalid_request())?
                        };
                        if !args.is_object() {
                            return Err(AdapterError::invalid_request());
                        }
                        append_message(
                            state,
                            "model",
                            vec![
                                json!({"functionCall":{"name":upstream_name,"args":args,"id":call_id}}),
                            ],
                        );
                    }
                    Some("reasoning") => {
                        if state.messages.is_empty() {
                            return Err(AdapterError::continuation_missing());
                        }
                    }
                    Some("message") | None => {
                        let role = item
                            .get("role")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let role = match role {
                            "user" => "user",
                            "assistant" => "model",
                            "developer" | "system" => "system",
                            _ => return Err(AdapterError::invalid_request()),
                        };
                        let parts = content_parts(
                            item.get("content")
                                .ok_or_else(AdapterError::invalid_request)?,
                        )?;
                        if parts.is_empty() {
                            continue;
                        }
                        if role == "system" {
                            append_system_parts(state, parts)?;
                        } else {
                            append_message(state, role, parts);
                        }
                    }
                    _ => return Err(AdapterError::invalid_request()),
                }
            }
            Ok(())
        }
        _ => Err(AdapterError::invalid_request()),
    }
}

pub(in crate::protocol::adapter) fn append_message(
    state: &mut MessagesBridgeState,
    role: &str,
    parts: Vec<Value>,
) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = state.messages.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some(role) {
            if let Some(existing) = last.get_mut("parts").and_then(Value::as_array_mut) {
                existing.extend(parts);
                return;
            }
        }
    }
    state.messages.push(json!({"role": role, "parts": parts}));
}

fn find_call_name<'a>(state: &'a MessagesBridgeState, call_id: &str) -> Option<&'a str> {
    state.messages.iter().rev().find_map(|message| {
        message
            .get("parts")
            .and_then(Value::as_array)
            .and_then(|parts| {
                parts.iter().rev().find_map(|part| {
                    let call = part.get("functionCall")?.as_object()?;
                    (call.get("id").and_then(Value::as_str) == Some(call_id))
                        .then(|| call.get("name").and_then(Value::as_str))
                        .flatten()
                })
            })
    })
}

pub(super) fn append_system_parts(
    state: &mut MessagesBridgeState,
    parts: Vec<Value>,
) -> AdapterResult<()> {
    if parts.is_empty() {
        return Ok(());
    }
    match state.system.take() {
        None => state.system = Some(json!({"parts": parts})),
        Some(Value::Object(mut object)) => {
            let existing = object
                .entry("parts".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            let Some(existing) = existing.as_array_mut() else {
                return Err(AdapterError::invalid_request());
            };
            existing.extend(parts);
            state.system = Some(Value::Object(object));
        }
        Some(_) => return Err(AdapterError::invalid_request()),
    }
    Ok(())
}

pub(super) fn content_parts(content: &Value) -> AdapterResult<Vec<Value>> {
    match content {
        Value::String(text) if !text.is_empty() => Ok(vec![json!({"text":text})]),
        Value::String(_) => Ok(Vec::new()),
        Value::Array(parts) => parts.iter().map(content_part).collect(),
        _ => Err(AdapterError::invalid_request()),
    }
}

fn content_part(value: &Value) -> AdapterResult<Value> {
    let part = value
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    match part.get("type").and_then(Value::as_str) {
        Some("input_text" | "output_text" | "text") => part
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(|text| json!({"text":text}))
            .ok_or_else(AdapterError::invalid_request),
        Some("input_image" | "output_image") => image_part(part),
        Some("input_file" | "output_file") => file_part(part),
        _ => Err(AdapterError::unsupported_binding()),
    }
}

fn image_part(part: &Map<String, Value>) -> AdapterResult<Value> {
    let url = part
        .get("image_url")
        .or_else(|| part.get("url"))
        .and_then(Value::as_str)
        .ok_or_else(AdapterError::invalid_request)?;
    if let Some((header, data)) = url.split_once(',') {
        let mime = header
            .strip_prefix("data:")
            .and_then(|value| value.strip_suffix(";base64"))
            .filter(|value| {
                matches!(
                    *value,
                    "image/gif" | "image/jpeg" | "image/png" | "image/webp"
                )
            })
            .ok_or_else(AdapterError::invalid_request)?;
        let decoded = STANDARD
            .decode(data)
            .map_err(|_| AdapterError::invalid_request())?;
        if decoded.len() > MAX_INLINE_MEDIA_BYTES {
            return Err(AdapterError::invalid_request());
        }
        return Ok(json!({"inlineData":{"mimeType":mime,"data":data}}));
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(AdapterError::invalid_request());
    }
    Ok(json!({"fileData":{"fileUri":url,"mimeType":"image/*"}}))
}

fn file_part(part: &Map<String, Value>) -> AdapterResult<Value> {
    let url = part
        .get("file_url")
        .or_else(|| part.get("file_uri"))
        .or_else(|| part.get("url"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("https://") || value.starts_with("http://"))
        .ok_or_else(AdapterError::invalid_request)?;
    let mime = part
        .get("mime_type")
        .or_else(|| part.get("mimeType"))
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream");
    Ok(json!({"fileData":{"fileUri":url,"mimeType":mime}}))
}

fn output_text(value: &Value) -> AdapterResult<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .map(str::to_string)
            .reduce(|mut left, right| {
                left.push_str(&right);
                left
            })
            .ok_or_else(AdapterError::invalid_request),
        _ => Err(AdapterError::invalid_request()),
    }
}

fn output_value(value: &Value) -> AdapterResult<Value> {
    match value {
        Value::String(text) => {
            Ok(serde_json::from_str(text).unwrap_or_else(|_| json!({"output":text})))
        }
        Value::Object(_) => Ok(value.clone()),
        Value::Array(parts) => {
            let mut text = Vec::new();
            let mut media = Vec::new();
            for part in parts {
                let object = part.as_object().ok_or_else(AdapterError::invalid_request)?;
                match object.get("type").and_then(Value::as_str) {
                    Some("input_text" | "output_text" | "text") => text.push(
                        object
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?
                            .to_string(),
                    ),
                    Some("input_image" | "output_image" | "input_file" | "output_file") => {
                        media.push(content_part(part)?);
                    }
                    _ => text.push(
                        serde_json::to_string(part).map_err(|_| AdapterError::invalid_request())?,
                    ),
                }
            }
            let mut response = Map::new();
            if !text.is_empty() {
                response.insert("output".to_string(), Value::String(text.join("\n")));
            }
            if !media.is_empty() {
                response.insert("parts".to_string(), Value::Array(media));
            }
            if response.is_empty() {
                response.insert("output".to_string(), Value::String(String::new()));
            }
            Ok(Value::Object(response))
        }
        _ => Err(AdapterError::invalid_request()),
    }
}
