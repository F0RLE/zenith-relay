use crate::protocol::AdapterError;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub(super) fn as_input_items(input: Option<&Value>) -> Vec<Value> {
    match input {
        Some(Value::String(text)) => vec![json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": text}],
        })],
        Some(Value::Object(item)) => vec![Value::Object(item.clone())],
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

pub(super) fn text_message(role: &str, content_type: &str, text: String) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{"type": content_type, "text": text}],
    })
}

pub(super) fn json_text(value: &Value) -> Result<String, AdapterError> {
    serde_json::to_string(value).map_err(|_| AdapterError::invalid_request())
}

pub(super) fn parse_json_object(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::Object(object)) => Some(Value::Object(object.clone())),
        Some(Value::String(text)) => serde_json::from_str(text).ok(),
        _ => None,
    }
}

pub(super) fn parse_function_arguments(item: &Map<String, Value>) -> Result<Value, AdapterError> {
    let raw = item.get("arguments");
    let parsed = parse_json_object(raw).ok_or_else(AdapterError::invalid_request)?;
    if !parsed.is_object() {
        return Err(AdapterError::invalid_request().with_parameter("input.arguments"));
    }
    Ok(parsed)
}

/// Basis Points accepts the provider's effort vocabulary (`low`, `medium`,
/// `high`, `xhigh`, `ultra`). Relay's catalog also exposes the client-facing
/// `max` label, which is the same level as `xhigh` for this transport. Keep
/// that normalization local to the adapter so native account routes retain
/// their original request value and diagnostics.
pub(super) fn basis_points_reasoning_effort(request: &Map<String, Value>) -> String {
    let effort = request
        .get("reasoning")
        .and_then(Value::as_object)
        .and_then(|reasoning| reasoning.get("effort"))
        .or_else(|| request.get("reasoning_effort"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    match effort.as_deref() {
        Some("low") => "low",
        Some("medium") => "medium",
        Some("high") => "high",
        Some("xhigh" | "x-high" | "extra-high" | "extra_high" | "max") => "xhigh",
        Some("ultra") => "ultra",
        // The Basis Points plugin uses medium as its safe default for an
        // absent or unknown level. This keeps a stale client label from
        // producing the upstream 422 `Invalid request body` response.
        _ => "medium",
    }
    .to_string()
}

pub(super) fn sanitized_metadata(value: Option<&Value>) -> Option<Value> {
    let object = value?.as_object()?;
    let mut metadata = Map::new();
    for (key, value) in object {
        if matches!(key.as_str(), "task_id" | "turn_id" | "agent_iteration") {
            continue;
        }
        let safe_key = key.chars().take(64).collect::<String>();
        if safe_key.is_empty() {
            continue;
        }
        let safe_value = match value {
            Value::String(text) => text.chars().take(512).collect::<String>(),
            Value::Bool(value) => value.to_string(),
            Value::Number(value) => value.to_string(),
            _ => continue,
        };
        metadata.insert(safe_key, Value::String(safe_value));
    }
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

pub(super) fn explicit_conversation_key(object: &Map<String, Value>) -> Option<String> {
    for key in [
        "prompt_cache_key",
        "promptCacheKey",
        "session_id",
        "sessionId",
    ] {
        if let Some(value) = object
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value.to_string());
        }
    }
    object
        .get("client_metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| {
            ["session_id", "sessionId"].iter().find_map(|key| {
                metadata
                    .get(*key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
        })
}

pub(super) fn short_hash(value: &Value) -> String {
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(value).unwrap_or_default());
    hex::encode(digest.finalize())
}

pub(super) fn stable_uuid(namespace: &str, value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(namespace.as_bytes());
    digest.update([0]);
    digest.update(value.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.finalize()[..16]);
    // UUID version 5/variant bits keep the identifier accepted by the
    // Codex backend while the SHA-256 seed keeps it deterministic.
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

pub(super) fn request_iteration(input: &[Value]) -> String {
    let last_user = input
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            (item.get("role").and_then(Value::as_str) == Some("user")).then_some(index)
        })
        .next_back()
        .unwrap_or(0);
    let iteration = input
        .iter()
        .skip(last_user.saturating_add(1))
        .filter(|item| {
            matches!(
                item.get("type").and_then(Value::as_str),
                Some("function_call_output" | "custom_tool_call_output")
            )
        })
        .count()
        .saturating_add(1);
    iteration.to_string()
}

pub(super) fn basis_points_metadata(
    object: &Map<String, Value>,
    translated_input: &[Value],
) -> Value {
    let conversation = explicit_conversation_key(object).unwrap_or_else(|| {
        translated_input
            .first()
            .map(short_hash)
            .unwrap_or_else(|| "anonymous".to_string())
    });
    let last_user = translated_input
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            (item.get("role").and_then(Value::as_str) == Some("user")).then_some(index)
        })
        .next_back();
    let turn_prefix = last_user
        .map(|index| translated_input[..=index].to_vec())
        .unwrap_or_else(|| translated_input.to_vec());
    let turn_fingerprint = short_hash(&Value::Array(turn_prefix));
    json!({
        "task_id": stable_uuid("cpa-oai-basispoints", &conversation),
        "turn_id": stable_uuid(
            "cpa-oai-basispoints/turn",
            &format!("{conversation}/{turn_fingerprint}")
        ),
        "agent_iteration": request_iteration(translated_input),
    })
}
