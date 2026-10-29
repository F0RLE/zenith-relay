//! Convenience response entry point and shared Responses identity helpers.
use super::{AdapterError, AdapterResult, TranslatedResponse, TranslationRequest};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn translate_messages_response(
    request: TranslationRequest,
    upstream: &Value,
) -> AdapterResult<TranslatedResponse> {
    request.translate(upstream)
}

pub fn bridged_response_id(seed: &str) -> String {
    bridged_response_id_scoped("", seed)
}

/// Length-prefixed scope and request seed avoid cross-route ID collisions.
pub fn bridged_response_id_scoped(scope: &str, seed: &str) -> String {
    if scope.is_empty() {
        let digest = Sha256::digest(seed.as_bytes());
        return format!("resp_bridge_{}", hex::encode(&digest[..12]));
    }
    let mut hasher = Sha256::new();
    hasher.update((scope.len() as u64).to_le_bytes());
    hasher.update(scope.as_bytes());
    hasher.update((seed.len() as u64).to_le_bytes());
    hasher.update(seed.as_bytes());
    format!("resp_bridge_{}", hex::encode(&hasher.finalize()[..12]))
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
