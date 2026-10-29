use super::contracts::{
    AdapterContinuationState, AdapterError, AdapterResult, MessagesReasoningMode,
    TranslatedResponse,
};
use super::translation::TranslationRequest;
use serde_json::Value;

/// Content extensions understood by the Messages response codec.
/// A native Messages route keeps the full provider contract.
pub(in crate::protocol::adapter) fn validate_response_block(block: &Value) -> AdapterResult<()> {
    let invalid = AdapterError::upstream_response_invalid;
    let allowed: &[&str] = match block.get("type").and_then(Value::as_str) {
        Some("text") => &["type", "text", "citations"],
        Some("thinking") => &["type", "thinking", "signature"],
        Some("redacted_thinking") => &["type", "data"],
        Some("tool_use") => &["type", "id", "name", "input"],
        Some("server_tool_use") => {
            super::translation::web_search::start(block)?;
            &["type", "id", "name", "input", "caller"]
        }
        Some("web_search_tool_result") => &["type", "tool_use_id", "content", "caller"],
        _ => return Err(invalid()),
    };
    super::contracts::validate_bridge_fields(block, allowed).map_err(|_| invalid())?;
    for field in ["text", "thinking", "signature", "data", "id", "name"] {
        if block.get(field).is_some_and(|value| !value.is_string()) {
            return Err(invalid());
        }
    }
    if block.get("input").is_some_and(|input| !input.is_object()) {
        return Err(invalid());
    }
    if let Some(citations) = block.get("citations") {
        super::translation::web_search::validate_citations(citations)?;
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
        Some("citations_delta") => {
            super::contracts::validate_bridge_fields(delta, &["type", "citation"])
                .map_err(|_| invalid())?;
            let citation = delta.get("citation").ok_or_else(invalid)?;
            return super::translation::web_search::validate_citations(&serde_json::json!([
                citation
            ]))
            .map_err(|_| invalid());
        }
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

pub(crate) use request::apply_cache_write_ttl;
pub(in crate::protocol::adapter) use request::{
    custom_tool_input_schema, message_max_output_tokens,
};
pub use request::{prepare_responses_to_messages, prepare_responses_to_messages_scoped};
pub(in crate::protocol::adapter) use response::custom_tool_input;
pub use response::{bridged_response_id, bridged_response_id_scoped, translate_messages_response};
