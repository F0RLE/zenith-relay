use super::contracts::{
    custom_tool_item_id, prepare_bridge_state, request_tool_catalog, AdapterError, AdapterResult,
    ClientToolTarget, MessagesBridgeRequest, MessagesBridgeResponse, MessagesBridgeState,
    MessagesReasoningMode, ResponsesToolKind, TranslatedTools,
};
use serde_json::Value;

/// The content extensions understood by both Messages response converters.
/// A native Messages route keeps the full provider contract.
pub(in crate::protocol::adapter) fn validate_response_block(block: &Value) -> AdapterResult<()> {
    let invalid = AdapterError::upstream_response_invalid;
    let allowed: &[&str] = match block.get("type").and_then(Value::as_str) {
        Some("text") => &["type", "text", "citations"],
        Some("thinking") => &["type", "thinking", "signature"],
        Some("redacted_thinking") => &["type", "data"],
        Some("tool_use") => &["type", "id", "name", "input"],
        _ => return Err(invalid()),
    };
    super::contracts::validate_bridge_fields(block, allowed).map_err(|_| invalid())?;
    for field in ["text", "thinking", "signature", "data", "id", "name"] {
        if block.get(field).is_some_and(|value| !value.is_string()) {
            return Err(invalid());
        }
    }
    if block.get("input").is_some_and(|input| !input.is_object())
        || block.get("citations").is_some_and(|citations| {
            !citations.is_null() && !citations.as_array().is_some_and(Vec::is_empty)
        })
    {
        return Err(invalid());
    }
    Ok(())
}

pub(in crate::protocol::adapter) fn validate_response_delta(delta: &Value) -> AdapterResult<()> {
    let invalid = AdapterError::upstream_stream_invalid;
    let field = match delta.get("type").and_then(Value::as_str) {
        Some("text_delta") => "text",
        Some("thinking_delta") => "thinking",
        Some("signature_delta") => "signature",
        Some("input_json_delta") => "partial_json",
        _ => return Err(invalid()),
    };
    super::contracts::validate_bridge_fields(delta, &["type", field]).map_err(|_| invalid())?;
    if !delta.get(field).is_some_and(Value::is_string) {
        return Err(invalid());
    }
    Ok(())
}

mod request;
mod response;

pub(in crate::protocol::adapter) use request::custom_tool_input_schema;
pub(crate) use request::{apply_cache_write_ttl, prepare_responses_to_messages_with_capabilities};
pub use request::{prepare_responses_to_messages, prepare_responses_to_messages_scoped};
pub use response::{bridged_response_id, bridged_response_id_scoped, translate_messages_response};
pub(in crate::protocol::adapter) use response::{
    custom_tool_input, messages_response_terminal, refusal_message,
    responses_output_from_messages_content, responses_usage, set_message_output_id,
    validate_messages_tool_calls,
};
