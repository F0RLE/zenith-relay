//! Responses input items translated into Gemini content parts.

use super::{AdapterError, AdapterResult, MessagesBridgeState};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};

pub(super) fn append_responses_input(
    bridge_state: &mut MessagesBridgeState,
    input: &Value,
) -> AdapterResult<()> {
    match input {
        Value::String(text) => {
            if !text.is_empty() {
                append_message(bridge_state, "user", vec![json!({"text":text})]);
            }
            Ok(())
        }
        Value::Array(input_items) => {
            for input_value in input_items {
                let input_item = input_value
                    .as_object()
                    .ok_or_else(AdapterError::invalid_request)?;
                if input_item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                    continue;
                }
                match input_item.get("type").and_then(Value::as_str) {
                    Some("function_call_output" | "custom_tool_call_output") => {
                        let call_id = input_item
                            .get("call_id")
                            .or_else(|| input_item.get("id"))
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let tool_name = find_call_name(bridge_state, call_id)
                            .or_else(|| input_item.get("name").and_then(Value::as_str))
                            .ok_or_else(AdapterError::invalid_request)?;
                        let function_response = if input_item.get("type").and_then(Value::as_str)
                            == Some("custom_tool_call_output")
                        {
                            json!({"input": output_text(input_item.get("output").ok_or_else(AdapterError::invalid_request)?)?})
                        } else {
                            output_value(
                                input_item
                                    .get("output")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?
                        };
                        append_message(
                            bridge_state,
                            "user",
                            vec![
                                json!({"functionResponse":{"name":tool_name,"response":function_response,"id":call_id}}),
                            ],
                        );
                    }
                    Some("function_call" | "custom_tool_call") => {
                        let tool_name = input_item
                            .get("name")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let namespace = input_item
                            .get("namespace")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|namespace_candidate| !namespace_candidate.is_empty());
                        let upstream_name = bridge_state
                            .upstream_tool_name(namespace, tool_name)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let call_id = input_item
                            .get("call_id")
                            .or_else(|| input_item.get("id"))
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let function_arguments = if input_item.get("type").and_then(Value::as_str)
                            == Some("custom_tool_call")
                        {
                            json!({"input": output_text(input_item.get("input").ok_or_else(AdapterError::invalid_request)?)?})
                        } else {
                            serde_json::from_str::<Value>(
                                input_item
                                    .get("arguments")
                                    .and_then(Value::as_str)
                                    .unwrap_or("{}"),
                            )
                            .map_err(|_| AdapterError::invalid_request())?
                        };
                        if !function_arguments.is_object() {
                            return Err(AdapterError::invalid_request());
                        }
                        append_message(
                            bridge_state,
                            "model",
                            vec![
                                json!({"functionCall":{"name":upstream_name,"args":function_arguments,"id":call_id}}),
                            ],
                        );
                    }
                    Some("reasoning") => {
                        if bridge_state.messages.is_empty() {
                            return Err(AdapterError::continuation_missing());
                        }
                    }
                    Some("message") | None => {
                        let role = input_item
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
                            input_item
                                .get("content")
                                .ok_or_else(AdapterError::invalid_request)?,
                        )?;
                        if parts.is_empty() {
                            continue;
                        }
                        if role == "system" {
                            append_system_parts(bridge_state, parts)?;
                        } else {
                            append_message(bridge_state, role, parts);
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
    bridge_state: &mut MessagesBridgeState,
    role: &str,
    parts: Vec<Value>,
) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = bridge_state.messages.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some(role) {
            if let Some(existing) = last.get_mut("parts").and_then(Value::as_array_mut) {
                existing.extend(parts);
                return;
            }
        }
    }
    bridge_state
        .messages
        .push(json!({"role": role, "parts": parts}));
}

fn find_call_name<'a>(bridge_state: &'a MessagesBridgeState, call_id: &str) -> Option<&'a str> {
    bridge_state.messages.iter().rev().find_map(|message| {
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
    bridge_state: &mut MessagesBridgeState,
    parts: Vec<Value>,
) -> AdapterResult<()> {
    if parts.is_empty() {
        return Ok(());
    }
    match bridge_state.system.take() {
        None => bridge_state.system = Some(json!({"parts": parts})),
        Some(Value::Object(mut object)) => {
            let existing = object
                .entry("parts".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            let Some(existing) = existing.as_array_mut() else {
                return Err(AdapterError::invalid_request());
            };
            existing.extend(parts);
            bridge_state.system = Some(Value::Object(object));
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

fn content_part(input_part: &Value) -> AdapterResult<Value> {
    let part = input_part
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
    if let Some((header, encoded_data)) = url.split_once(',') {
        let mime = header
            .strip_prefix("data:")
            .and_then(|mime_header| mime_header.strip_suffix(";base64"))
            .filter(|mime_type| {
                matches!(
                    *mime_type,
                    "image/gif" | "image/jpeg" | "image/png" | "image/webp"
                )
            })
            .ok_or_else(AdapterError::invalid_request)?;
        STANDARD
            .decode(encoded_data)
            .map_err(|_| AdapterError::invalid_request())?;
        return Ok(json!({"inlineData":{"mimeType":mime,"data":encoded_data}}));
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
        .filter(|file_url| file_url.starts_with("https://") || file_url.starts_with("http://"))
        .ok_or_else(AdapterError::invalid_request)?;
    let mime = part
        .get("mime_type")
        .or_else(|| part.get("mimeType"))
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream");
    Ok(json!({"fileData":{"fileUri":url,"mimeType":mime}}))
}

fn output_text(output_value: &Value) -> AdapterResult<String> {
    match output_value {
        Value::String(text_value) => Ok(text_value.clone()),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|text_part| text_part.get("text").and_then(Value::as_str))
            .map(str::to_string)
            .reduce(|mut left, right| {
                left.push_str(&right);
                left
            })
            .ok_or_else(AdapterError::invalid_request),
        _ => Err(AdapterError::invalid_request()),
    }
}

fn output_value(output_value: &Value) -> AdapterResult<Value> {
    match output_value {
        Value::String(output_text) => {
            Ok(serde_json::from_str(output_text).unwrap_or_else(|_| json!({"output":output_text})))
        }
        Value::Object(_) => Ok(output_value.clone()),
        Value::Array(parts) => {
            let mut text_parts = Vec::new();
            let mut media_parts = Vec::new();
            for output_part in parts {
                let output_object = output_part
                    .as_object()
                    .ok_or_else(AdapterError::invalid_request)?;
                match output_object.get("type").and_then(Value::as_str) {
                    Some("input_text" | "output_text" | "text") => text_parts.push(
                        output_object
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?
                            .to_string(),
                    ),
                    Some("input_image" | "output_image" | "input_file" | "output_file") => {
                        media_parts.push(content_part(output_part)?);
                    }
                    _ => text_parts.push(
                        serde_json::to_string(output_part)
                            .map_err(|_| AdapterError::invalid_request())?,
                    ),
                }
            }
            let mut response_object = Map::new();
            if !text_parts.is_empty() {
                response_object.insert("output".to_string(), Value::String(text_parts.join("\n")));
            }
            if !media_parts.is_empty() {
                response_object.insert("parts".to_string(), Value::Array(media_parts));
            }
            if response_object.is_empty() {
                response_object.insert("output".to_string(), Value::String(String::new()));
            }
            Ok(Value::Object(response_object))
        }
        _ => Err(AdapterError::invalid_request()),
    }
}
