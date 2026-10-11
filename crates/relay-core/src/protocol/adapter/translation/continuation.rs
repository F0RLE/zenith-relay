use super::*;
use serde_json::json;
use sha2::{Digest, Sha256};

/// Stateless clients replay portable history. Its last assistant turn identifies
/// the bounded local state; signatures are never embedded in portable text.
pub(super) fn request_key(
    client: WireApi,
    request: &Value,
    model: &str,
) -> AdapterResult<Option<String>> {
    let mut decoded = decode::request(client, request)?;
    decode::resolve_tool_history(&mut decoded.messages)?;
    let Some(end) = decoded
        .messages
        .iter()
        .rposition(|message| message.role == Role::Assistant)
    else {
        return Ok(None);
    };
    history_key(client, model, &decoded.messages[..=end]).map(Some)
}

pub(super) fn history_key(
    client: WireApi,
    model: &str,
    history: &[Message],
) -> AdapterResult<String> {
    let mut messages = Vec::<Value>::new();
    for message in history {
        let mut blocks = Vec::new();
        for block in &message.blocks {
            let value = match block {
                Block::Reasoning(_) => continue,
                Block::Text(text) | Block::Refusal(text) if text.is_empty() => continue,
                Block::Text(text) | Block::Refusal(text) => json!({"text": text}),
                Block::Image { url, .. } => json!({"image": url}),
                Block::ToolCall {
                    id,
                    name,
                    arguments,
                } => json!({
                    "call": id, "name": name,
                    "arguments": serde_json::from_str::<Value>(arguments)
                        .map_err(|_| AdapterError::invalid_request())?
                }),
                Block::ToolResult {
                    id,
                    content,
                    is_error,
                    ..
                } => {
                    json!({"result": id, "content": content, "is_error": is_error})
                }
            };
            if let Some(previous) = blocks.last_mut().and_then(Value::as_object_mut) {
                if let (Some(previous_text), Some(text)) = (
                    previous
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    value.get("text").and_then(Value::as_str),
                ) {
                    previous.insert("text".into(), format!("{previous_text}{text}").into());
                    continue;
                }
            }
            blocks.push(value);
        }
        if blocks.is_empty() {
            continue;
        }
        let role =
            serde_json::to_value(message.role).map_err(|_| AdapterError::invalid_request())?;
        if let Some(previous) = messages
            .last_mut()
            .filter(|previous| previous["role"] == role)
        {
            previous["blocks"]
                .as_array_mut()
                .expect("history blocks")
                .extend(blocks);
        } else {
            messages.push(json!({"role": role, "blocks": blocks}));
        }
    }
    let bytes = serde_json::to_vec(&(client, model, messages))
        .map_err(|_| AdapterError::invalid_request())?;
    Ok(format!(
        "adapter-history:{}",
        hex::encode(Sha256::digest(bytes))
    ))
}

pub(super) fn restore(
    request: &mut Request,
    previous: MessagesBridgeState,
    client: WireApi,
    upstream: WireApi,
    model: &str,
    mode: MessagesReasoningMode,
) -> AdapterResult<BTreeMap<usize, Vec<Value>>> {
    if !previous.reusable
        || previous.model != model
        || previous.reasoning_mode != mode
        || previous.upstream_protocol != Some(upstream)
    {
        return Err(AdapterError::continuation_mismatch());
    }
    let mut history = previous
        .portable_history
        .ok_or_else(AdapterError::continuation_mismatch)?;
    if client != WireApi::Responses {
        let end = request
            .messages
            .iter()
            .rposition(|message| message.role == Role::Assistant)
            .ok_or_else(AdapterError::continuation_mismatch)?;
        if history_key(client, model, &request.messages[..=end])?
            != history_key(client, model, &history)?
        {
            return Err(AdapterError::continuation_mismatch());
        }
        request.messages = request.messages.split_off(end + 1);
    }
    history.append(&mut request.messages);
    request.messages = history;
    Ok(previous.native_turns)
}

pub(super) fn replay_items(protocol: WireApi, native: &[Value]) -> Vec<Value> {
    match protocol {
        WireApi::Responses => native.to_vec(),
        WireApi::Messages => vec![json!({"role":"assistant", "content":native})],
        WireApi::Gemini => vec![json!({"role":"model", "parts":native})],
        WireApi::ChatCompletions => Vec::new(),
    }
}
