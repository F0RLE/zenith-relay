//! Anthropic Messages responses translated back into Responses bodies.

use super::{
    custom_tool_item_id, AdapterError, AdapterResult, MessagesBridgeRequest,
    MessagesBridgeResponse, MessagesBridgeState, ResponsesToolKind,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Converts a complete Anthropic Messages response to a complete Responses
/// object and captures the exact native assistant blocks for the next turn.
pub fn translate_messages_response(
    request: MessagesBridgeRequest,
    upstream: &Value,
) -> AdapterResult<MessagesBridgeResponse> {
    let (status, incomplete_reason) =
        messages_response_terminal(upstream.get("stop_reason").and_then(Value::as_str))?;
    let upstream_id = upstream
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(AdapterError::upstream_response_invalid)?;
    let content = upstream
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::upstream_response_invalid)?
        .clone();
    validate_messages_tool_calls(&request.state, &content)?;
    let (mut output, _) =
        responses_output_from_messages_content(&content, &request.state, status == "incomplete")?;
    let response_id = bridged_response_id_scoped(request.response_scope(), upstream_id);
    set_message_output_id(&mut output, &response_id);
    let usage = responses_usage(upstream.get("usage"));
    let response_body = json!({
        "id": response_id,
        "object": "response",
        "created_at": upstream
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        "status": status,
        "incomplete_details": incomplete_reason.map(|reason| json!({"reason": reason})),
        "model": request.state.model,
        "output": output,
        "usage": usage,
    });
    let mut continuation = request.state;
    continuation.append_assistant_content(content);
    Ok(MessagesBridgeResponse {
        response_body,
        response_id,
        continuation,
    })
}

/// Map only terminal Messages reasons that a Responses client can represent.
/// A missing or unknown reason must not be presented as a completed turn.
pub(in crate::protocol::adapter) fn messages_response_terminal(
    stop_reason: Option<&str>,
) -> AdapterResult<(&'static str, Option<&'static str>)> {
    match stop_reason {
        Some("end_turn" | "stop_sequence" | "tool_use") => Ok(("completed", None)),
        Some("max_tokens") => Ok(("incomplete", Some("max_output_tokens"))),
        Some("refusal") => Ok(("incomplete", Some("content_filter"))),
        _ => Err(AdapterError::upstream_response_invalid()),
    }
}

pub fn bridged_response_id(upstream_id: &str) -> String {
    bridged_response_id_scoped("", upstream_id)
}

/// Derives a deterministic client-facing id from both the upstream id and the
/// connector route that produced it. Length-prefixing the scope prevents
/// ambiguous concatenations such as `ab` + `c` versus `a` + `bc`.
pub fn bridged_response_id_scoped(scope: &str, upstream_id: &str) -> String {
    if scope.is_empty() {
        let digest = Sha256::digest(upstream_id.as_bytes());
        return format!("resp_bridge_{}", hex::encode(&digest[..12]));
    }
    let mut hasher = Sha256::new();
    hasher.update((scope.len() as u64).to_le_bytes());
    hasher.update(scope.as_bytes());
    hasher.update((upstream_id.len() as u64).to_le_bytes());
    hasher.update(upstream_id.as_bytes());
    let digest = hasher.finalize();
    format!("resp_bridge_{}", hex::encode(&digest[..12]))
}

pub(in crate::protocol::adapter) fn set_message_output_id(output: &mut [Value], response_id: &str) {
    let mut message_index = 0_usize;
    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        if let Some(object) = item.as_object_mut() {
            let id = if message_index == 0 {
                format!("msg_{response_id}")
            } else {
                format!("msg_{response_id}_{message_index}")
            };
            object.insert("id".to_string(), Value::String(id));
            message_index = message_index.saturating_add(1);
        }
    }
}

/// An upstream Messages response may only invoke a function that Relay sent in
/// the translated client catalog. This protects the client tool router from an
/// upstream-invented tool name while preserving the exact name supplied by the
/// client when the call is valid.
pub(in crate::protocol::adapter) fn validate_messages_tool_calls(
    state: &MessagesBridgeState,
    content: &[Value],
) -> AdapterResult<()> {
    let mut call_ids = BTreeSet::new();
    for block in content {
        let block = block
            .as_object()
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let id = nonempty_block_str(block, "id")?;
        let name = nonempty_block_str(block, "name")?;
        if !block.get("input").is_some_and(Value::is_object)
            || !state.allows_tool_name(name)
            || !call_ids.insert(id.to_string())
        {
            return Err(AdapterError::upstream_response_invalid());
        }
    }
    Ok(())
}

fn nonempty_block_str<'a>(block: &'a Map<String, Value>, key: &str) -> AdapterResult<&'a str> {
    block
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(AdapterError::upstream_response_invalid)
}

pub(in crate::protocol::adapter) fn responses_output_from_messages_content(
    content: &[Value],
    state: &MessagesBridgeState,
    allow_empty: bool,
) -> AdapterResult<(Vec<Value>, Vec<Value>)> {
    let mut output = Vec::new();
    let mut preserved = Vec::new();
    let mut text = Vec::new();
    let mut text_message_index = 0_usize;
    let flush_text =
        |output: &mut Vec<Value>, text: &mut Vec<Value>, text_message_index: &mut usize| {
            if text.is_empty() {
                return;
            }
            let index = *text_message_index;
            *text_message_index = (*text_message_index).saturating_add(1);
            output.push(json!({
                "id": if index == 0 {
                    "msg_bridge_output".to_string()
                } else {
                    format!("msg_bridge_output_{index}")
                },
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": std::mem::take(text),
            }));
        };
    for block in content {
        let block = block
            .as_object()
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let value = block
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                if !value.is_empty() {
                    text.push(json!({"type": "output_text", "text": value, "annotations": []}));
                }
                preserved.push(Value::Object(block.clone()));
            }
            Some("tool_use") => {
                flush_text(&mut output, &mut text, &mut text_message_index);
                let call_id = nonempty_block_str(block, "id")?;
                let name = nonempty_block_str(block, "name")?;
                let target = state
                    .client_tool(name)
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                let kind = target.kind;
                let client_name = target.name.clone();
                let client_namespace = target.namespace.clone();
                let input = block
                    .get("input")
                    .filter(|value| value.is_object())
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                let mut item = match kind {
                    ResponsesToolKind::Function => json!({
                        "id": call_id,
                        "type": kind.response_item_type(),
                        "status": "completed",
                        "call_id": call_id,
                        "name": client_name.clone(),
                        "arguments": serde_json::to_string(input).map_err(|_| AdapterError::upstream_response_invalid())?,
                    }),
                    ResponsesToolKind::Custom => json!({
                        "id": custom_tool_item_id(call_id),
                        "type": kind.response_item_type(),
                        "status": "completed",
                        "call_id": call_id,
                        "name": client_name.clone(),
                        "input": custom_tool_input(input)?,
                    }),
                };
                if let Some(namespace) = client_namespace {
                    item.as_object_mut()
                        .expect("Responses output item is an object")
                        .insert("namespace".to_string(), Value::String(namespace));
                }
                output.push(item);
                preserved.push(Value::Object(block.clone()));
            }
            Some("thinking" | "redacted_thinking") => {
                // The native block (including its signature) must survive in bridge
                // state, but it is intentionally not exposed as fake Responses
                // encrypted content.
                preserved.push(Value::Object(block.clone()));
            }
            _ => return Err(AdapterError::upstream_response_invalid()),
        }
    }
    flush_text(&mut output, &mut text, &mut text_message_index);
    if output.is_empty() && !allow_empty {
        return Err(AdapterError::upstream_response_invalid());
    }
    Ok((output, preserved))
}

pub(in crate::protocol::adapter) fn custom_tool_input(input: &Value) -> AdapterResult<&str> {
    let input = input
        .as_object()
        .ok_or_else(AdapterError::upstream_response_invalid)?;
    if input.len() != 1 {
        return Err(AdapterError::upstream_response_invalid());
    }
    input
        .get("input")
        .and_then(Value::as_str)
        .ok_or_else(AdapterError::upstream_response_invalid)
}

pub(in crate::protocol::adapter) fn responses_usage(usage: Option<&Value>) -> Value {
    let mut result = Map::new();
    if let Some(input_tokens) = usage
        .and_then(|usage| usage.get("input_tokens"))
        .and_then(Value::as_u64)
    {
        result.insert("input_tokens".to_string(), Value::from(input_tokens));
    }
    if let Some(output_tokens) = usage
        .and_then(|usage| usage.get("output_tokens"))
        .and_then(Value::as_u64)
    {
        result.insert("output_tokens".to_string(), Value::from(output_tokens));
    }
    if let Some(total_tokens) = usage
        .and_then(|usage| usage.get("total_tokens"))
        .and_then(Value::as_u64)
    {
        result.insert("total_tokens".to_string(), Value::from(total_tokens));
    }
    if let Some(cache_read) = usage
        .and_then(|usage| usage.get("cache_read_input_tokens"))
        .and_then(Value::as_u64)
    {
        result.insert(
            "input_tokens_details".to_string(),
            json!({"cached_tokens": cache_read}),
        );
    }
    if let Some(cache_write) = usage
        .and_then(|usage| usage.get("cache_creation_input_tokens"))
        .and_then(Value::as_u64)
    {
        result
            .entry("input_tokens_details".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("usage details is an object")
            .insert("cache_write_tokens".to_string(), Value::from(cache_write));
    }
    if let Some(cache_write_ttl) = usage.and_then(cache_write_ttl_from_usage) {
        result
            .entry("input_tokens_details".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("usage details is an object")
            .insert(
                "cache_write_ttl".to_string(),
                Value::String(cache_write_ttl.to_string()),
            );
    }
    Value::Object(result)
}

fn cache_write_ttl_from_usage(usage: &Value) -> Option<&'static str> {
    let creation = usage.get("cache_creation")?;
    if creation
        .get("ephemeral_1h_input_tokens")
        .and_then(Value::as_u64)
        .is_some_and(|tokens| tokens > 0)
    {
        return Some("1h");
    }
    creation
        .get("ephemeral_5m_input_tokens")
        .and_then(Value::as_u64)
        .is_some_and(|tokens| tokens > 0)
        .then_some("5m")
}
