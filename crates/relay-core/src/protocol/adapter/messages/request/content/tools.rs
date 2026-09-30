use super::super::{AdapterError, AdapterResult, MessagesBridgeState, ResponsesToolKind};
use super::{image_block_from_data_uri, text_block};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub(in crate::protocol::adapter::messages::request) fn append_assistant_tool_use(
    state: &mut MessagesBridgeState,
    item: &Map<String, Value>,
) -> AdapterResult<()> {
    let kind = ResponsesToolKind::from_call_item(item)?;
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    let namespace = match item.get("namespace") {
        None => None,
        Some(namespace) => Some(
            namespace
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(AdapterError::invalid_request)?,
        ),
    };
    let upstream_name = state
        .upstream_tool_name(namespace, name)
        .map(str::to_string)
        .ok_or_else(AdapterError::invalid_request)?;
    if state.client_tool_kind(&upstream_name) != Some(kind) {
        return Err(AdapterError::invalid_request());
    }
    let input = match kind {
        ResponsesToolKind::Function => {
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            serde_json::from_str::<Value>(arguments)
                .ok()
                .filter(Value::is_object)
                .ok_or_else(AdapterError::invalid_request)?
        }
        ResponsesToolKind::Custom => {
            let input = item
                .get("input")
                .and_then(Value::as_str)
                .ok_or_else(AdapterError::invalid_request)?;
            json!({"input": input})
        }
    };
    state.messages.push(json!({
        "role": "assistant",
        "content": [{"type": "tool_use", "id": call_id, "name": upstream_name, "input": input}],
    }));
    Ok(())
}

pub(in crate::protocol::adapter::messages::request) fn flush_tool_results(
    state: &mut MessagesBridgeState,
    results: &mut Vec<Value>,
) -> AdapterResult<()> {
    if results.is_empty() {
        return Ok(());
    }
    let Some(last) = state.messages.last() else {
        return Err(AdapterError::continuation_missing());
    };
    if last.get("role").and_then(Value::as_str) != Some("assistant")
        || !last
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            })
    {
        return Err(AdapterError::continuation_mismatch());
    }
    let known_call_ids = last
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| block.get("id").and_then(Value::as_str))
        .map(|id| (id.to_string(), ()))
        .collect::<BTreeMap<_, _>>();
    let mut returned_call_ids = BTreeMap::new();
    for result in results.iter() {
        let call_id = result
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(AdapterError::continuation_mismatch)?;
        if !known_call_ids.contains_key(call_id)
            || returned_call_ids.insert(call_id.to_string(), ()).is_some()
        {
            return Err(AdapterError::continuation_mismatch());
        }
    }
    let content = std::mem::take(results);
    state
        .messages
        .push(json!({"role": "user", "content": content}));
    Ok(())
}

pub(in crate::protocol::adapter::messages::request) fn tool_result_block(
    state: &MessagesBridgeState,
    item: &Map<String, Value>,
) -> AdapterResult<Value> {
    let kind = ResponsesToolKind::from_output_item(item)?;
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    let expected = state
        .messages
        .last()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks.iter().find(|block| {
                block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && block.get("id").and_then(Value::as_str) == Some(call_id)
            })
        })
        .and_then(|block| block.get("name"))
        .and_then(Value::as_str)
        .and_then(|name| state.client_tool(name))
        .ok_or_else(AdapterError::continuation_mismatch)?;
    if kind != expected.kind {
        return Err(AdapterError::continuation_mismatch());
    }
    if let Some(namespace) = item.get("namespace") {
        let namespace = namespace
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(AdapterError::invalid_request)?;
        if expected.namespace.as_deref() != Some(namespace) {
            return Err(AdapterError::continuation_mismatch());
        }
    }
    let content = match kind {
        ResponsesToolKind::Custom => match item.get("output") {
            Some(Value::String(output)) => Value::String(output.clone()),
            _ => return Err(AdapterError::invalid_request()),
        },
        ResponsesToolKind::Function => function_tool_result_content(item.get("output"))?,
    };
    Ok(json!({
        "type": "tool_result",
        "tool_use_id": call_id,
        "content": content,
    }))
}

fn function_tool_result_content(output: Option<&Value>) -> AdapterResult<Value> {
    match output {
        None => Ok(Value::String(String::new())),
        Some(Value::String(output)) => {
            let Some(parts) = serde_json::from_str::<Value>(output)
                .ok()
                .and_then(|value| value.as_array().cloned())
            else {
                return Ok(Value::String(output.clone()));
            };
            match function_tool_output_parts(&parts)? {
                Some(content) => Ok(content),
                None => Ok(Value::String(output.clone())),
            }
        }
        Some(Value::Array(parts)) => match function_tool_output_parts(parts)? {
            Some(content) => Ok(content),
            None => Ok(Value::String(
                serde_json::to_string(parts).map_err(|_| AdapterError::invalid_request())?,
            )),
        },
        Some(value) => Ok(Value::String(
            serde_json::to_string(value).map_err(|_| AdapterError::invalid_request())?,
        )),
    }
}

/// Converts a Responses tool output array only when it contains an image.
/// Ordinary JSON arrays keep their previous string representation.
fn function_tool_output_parts(parts: &[Value]) -> AdapterResult<Option<Value>> {
    let contains_image = parts.iter().any(|part| {
        part.get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "input_image")
    });
    if !contains_image {
        return Ok(None);
    }

    let mut blocks = Vec::with_capacity(parts.len());
    for part in parts {
        let Some(part) = part.as_object() else {
            return Err(AdapterError::invalid_request());
        };
        match part.get("type").and_then(Value::as_str) {
            Some("input_text" | "output_text" | "text") => {
                let text = part
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::invalid_request)?;
                if !text.is_empty() {
                    blocks.push(text_block(text));
                }
            }
            Some("input_image") => blocks.push(image_block_from_data_uri(
                part.get("image_url")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::invalid_request)?,
            )?),
            _ => return Err(AdapterError::invalid_request()),
        }
    }
    if blocks.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    Ok(Some(Value::Array(blocks)))
}
