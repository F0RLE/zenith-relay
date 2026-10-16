use super::MessagesReasoningMode;
use crate::WireApi;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Volatile, bounded conversation state shared by all converted routes.
/// Native provider turns stay separate from portable content and are never
/// serialized into diagnostics, usage, or a foreign provider's request.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MessagesBridgeState {
    pub(in crate::protocol::adapter) client_wire_api: Option<WireApi>,
    pub(in crate::protocol::adapter) portable_history:
        Option<Vec<super::super::translation::Message>>,
    pub(in crate::protocol::adapter) response_tools: Option<Vec<Value>>,
    pub(in crate::protocol::adapter) response_call_targets:
        BTreeMap<String, super::ClientToolTarget>,
    pub(in crate::protocol::adapter) upstream_protocol: Option<WireApi>,
    pub(in crate::protocol::adapter) native_turns: BTreeMap<usize, Vec<Value>>,
    pub(in crate::protocol::adapter) history_key: Option<String>,
    pub(in crate::protocol::adapter) reusable: bool,
    pub(in crate::protocol::adapter) model: String,
    pub(in crate::protocol::adapter) reasoning_mode: MessagesReasoningMode,
}

impl MessagesBridgeState {
    pub(crate) fn upstream_wire_api(&self) -> WireApi {
        self.upstream_protocol.unwrap_or(WireApi::Messages)
    }

    pub(crate) fn storage_key<'a>(&'a self, response_id: &'a str) -> &'a str {
        if self.client_wire_api == Some(WireApi::Responses) {
            response_id
        } else {
            self.history_key.as_deref().unwrap_or(response_id)
        }
    }

    pub(crate) fn is_reusable(&self) -> bool {
        self.reusable
    }

    pub(in crate::protocol::adapter) fn new(
        model: &str,
        reasoning_mode: MessagesReasoningMode,
    ) -> Self {
        Self {
            client_wire_api: None,
            portable_history: None,
            response_tools: None,
            response_call_targets: BTreeMap::new(),
            upstream_protocol: None,
            native_turns: BTreeMap::new(),
            history_key: None,
            reusable: true,
            model: model.to_owned(),
            reasoning_mode,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

/// Source-compatible name; it is the shared request, not a pair-specific engine.
pub type MessagesBridgeRequest = super::super::translation::TranslationRequest;

#[derive(Clone, Debug)]
pub struct MessagesBridgeResponse {
    pub response_body: Value,
    pub response_id: String,
    pub continuation: MessagesBridgeState,
}
