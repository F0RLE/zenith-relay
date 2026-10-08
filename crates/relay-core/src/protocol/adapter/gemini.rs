use super::contracts::{
    bridged_namespace_tool_name, custom_tool_item_id, prepare_bridge_state, request_tool_catalog,
    AdapterError, AdapterResult, ClientToolTarget, MessagesBridgeState, MessagesReasoningMode,
    ResponsesToolKind,
};
#[cfg(test)]
use serde_json::json;
use serde_json::Value;

/// Complete Responses-to-Gemini request plus the local state needed by the
/// next `previous_response_id` turn.
#[derive(Clone, Debug)]
pub struct GeminiBridgeRequest {
    pub(super) upstream_body: Value,
    pub(super) model: String,
    pub(super) response_id: String,
    pub(super) bridge_state: MessagesBridgeState,
}

impl GeminiBridgeRequest {
    pub fn upstream_body(&self) -> &Value {
        &self.upstream_body
    }
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn response_id(&self) -> &str {
        &self.response_id
    }
    pub(super) fn bridge_state(&self) -> &MessagesBridgeState {
        &self.bridge_state
    }
}

#[derive(Clone, Debug)]
pub struct GeminiBridgeResponse {
    pub response_body: Value,
    pub response_id: String,
    pub continuation: MessagesBridgeState,
}

mod json_path;
mod request;
mod response;

pub(in crate::protocol::adapter) use json_path::{apply_partial_args, function_call_args};
#[cfg(test)]
pub use request::prepare_responses_to_gemini;
pub(crate) use request::prepare_responses_to_gemini_with_reasoning;
pub(crate) use response::gemini_incomplete;
pub use response::translate_gemini_response;
pub(in crate::protocol::adapter) use response::{candidate_incomplete_reason, prompt_blocked};
#[cfg(test)]
mod tests;
