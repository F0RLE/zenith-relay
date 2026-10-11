use super::super::{
    AdapterError, AdapterResult, ClientToolTarget, MessagesBridgeState, ResponsesToolKind,
};
use super::{image_block_from_data_uri, text_block};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub(in crate::protocol::adapter::messages::request) fn append_assistant_tool_use(
    state: &mut MessagesBridgeState,
    tool_call_item: &Map<String, Value>,
) -> AdapterResult<()> {
    let kind = ResponsesToolKind::from_call_item(tool_call_item)?;
    let call_id = tool_call_item
        .get("call_id")
        .or_else(|| tool_call_item.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|call_id_text| !call_id_text.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    // Historical calls can refer to tools outside this turn's active catalog.
    // Their identity comes from the call, never from the current allow-list.
    let target = ClientToolTarget::from_call(tool_call_item)?;
    let upstream_name = target.upstream_name();
    state.record_tool_call(call_id, target)?;
    let tool_input = match kind {
        ResponsesToolKind::Function => {
            let arguments = tool_call_item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            serde_json::from_str::<Value>(arguments)
                .ok()
                .filter(Value::is_object)
                .ok_or_else(AdapterError::invalid_request)?
        }
        ResponsesToolKind::Custom => {
            let custom_input = tool_call_item
                .get("input")
                .and_then(Value::as_str)
                .ok_or_else(AdapterError::invalid_request)?;
            json!({"input": custom_input})
        }
    };
    state.append_assistant_content(vec![
        json!({"type": "tool_use", "id": call_id, "name": upstream_name, "input": tool_input}),
    ]);
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
        .map(|call_id| (call_id.to_string(), ()))
        .collect::<BTreeMap<_, _>>();
    let mut returned_call_ids = BTreeMap::new();
    for tool_result_item in results.iter() {
        let call_id = tool_result_item
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|call_id| !call_id.is_empty())
            .ok_or_else(AdapterError::continuation_mismatch)?;
        if !known_call_ids.contains_key(call_id)
            || returned_call_ids.insert(call_id.to_string(), ()).is_some()
        {
            return Err(AdapterError::continuation_mismatch());
        }
    }
    if returned_call_ids.len() != known_call_ids.len() {
        return Err(AdapterError::continuation_mismatch());
    }
    let content = std::mem::take(results);
    state
        .messages
        .push(json!({"role": "user", "content": content}));
    Ok(())
}

pub(in crate::protocol::adapter::messages::request) fn tool_result_block(
    state: &MessagesBridgeState,
    tool_result_item: &Map<String, Value>,
) -> AdapterResult<Value> {
    let kind = ResponsesToolKind::from_output_item(tool_result_item)?;
    let call_id = tool_result_item
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|call_id_text| !call_id_text.is_empty())
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
        .and_then(|_| state.tool_call_targets.get(call_id))
        .ok_or_else(AdapterError::continuation_mismatch)?;
    if kind != expected.kind {
        return Err(AdapterError::continuation_mismatch());
    }
    if let Some(namespace) = tool_result_item.get("namespace") {
        let namespace = namespace
            .as_str()
            .map(str::trim)
            .filter(|namespace_text| !namespace_text.is_empty())
            .ok_or_else(AdapterError::invalid_request)?;
        if expected.namespace.as_deref() != Some(namespace) {
            return Err(AdapterError::continuation_mismatch());
        }
    }
    let content = match kind {
        ResponsesToolKind::Custom => match tool_result_item.get("output") {
            Some(Value::String(output_text)) => Value::String(output_text.clone()),
            _ => return Err(AdapterError::invalid_request()),
        },
        ResponsesToolKind::Function => {
            function_tool_result_content(tool_result_item.get("output"))?
        }
    };
    Ok(json!({
        "type": "tool_result",
        "tool_use_id": call_id,
        "content": content,
    }))
}

fn function_tool_result_content(tool_output: Option<&Value>) -> AdapterResult<Value> {
    match tool_output {
        None => Ok(Value::String(String::new())),
        Some(Value::String(output_text)) => {
            let Some(output_parts) = serde_json::from_str::<Value>(output_text)
                .ok()
                .and_then(|parsed_output| parsed_output.as_array().cloned())
            else {
                return Ok(Value::String(output_text.clone()));
            };
            if !output_parts
                .iter()
                .any(|part| part.get("type").and_then(Value::as_str) == Some("input_image"))
            {
                return Ok(Value::String(output_text.clone()));
            }
            match function_tool_output_parts(&output_parts)? {
                Some(content) => Ok(content),
                None => Ok(Value::String(output_text.clone())),
            }
        }
        Some(Value::Array(parts)) if parts.is_empty() => Ok(Value::Array(Vec::new())),
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

/// Recognizes explicit content blocks while leaving ordinary JSON arrays intact.
fn function_tool_output_parts(parts: &[Value]) -> AdapterResult<Option<Value>> {
    let contains_content = parts.iter().any(|part| {
        part.get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| {
                matches!(kind, "input_text" | "output_text" | "text" | "input_image")
            })
    });
    if !contains_content {
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
    Ok(Some(Value::Array(blocks)))
}
