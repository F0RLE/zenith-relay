use super::{
    validate_responses_bridge_request, AdapterError, AdapterResult, ClientToolTarget,
    MessagesReasoningMode, ResponsesToolKind,
};
use crate::WireApi;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
pub(in crate::protocol::adapter) fn prepare_bridge_state<'a>(
    request: &'a Value,
    model: &str,
    reasoning_mode: MessagesReasoningMode,
    previous: Option<MessagesBridgeState>,
    wire_api: WireApi,
) -> AdapterResult<(&'a Map<String, Value>, MessagesBridgeState)> {
    let object = request
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    validate_responses_bridge_request(request, wire_api)?;
    let has_previous_response = object
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let mut state = match (has_previous_response, previous) {
        (true, Some(state)) if state.model == model => state,
        (true, Some(_)) => return Err(AdapterError::continuation_mismatch()),
        (true, None) => return Err(AdapterError::continuation_missing()),
        (false, _) => MessagesBridgeState::new(model, reasoning_mode),
    };
    if state.reasoning_mode != reasoning_mode {
        return Err(AdapterError::continuation_mismatch());
    }
    state.system = state.historical_system.take();
    Ok((object, state))
}

/// Volatile continuation state for a Responses-to-Messages bridge. It is
/// intentionally local-only and is never serialized into diagnostics or usage
/// records because it contains the user's conversation content.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MessagesBridgeState {
    pub(in crate::protocol::adapter) portable_history:
        Option<Vec<super::super::translation::Message>>,
    pub(in crate::protocol::adapter) model: String,
    pub(in crate::protocol::adapter) system: Option<Value>,
    pub(in crate::protocol::adapter) historical_system: Option<Value>,
    pub(in crate::protocol::adapter) messages: Vec<Value>,
    pub(in crate::protocol::adapter) tools: Option<Vec<Value>>,
    pub(in crate::protocol::adapter) tool_targets: BTreeMap<String, ClientToolTarget>,
    pub(in crate::protocol::adapter) tool_choice: Option<Value>,
    pub(in crate::protocol::adapter) tool_allow_list: Option<BTreeSet<String>>,
    pub(in crate::protocol::adapter) reasoning_mode: MessagesReasoningMode,
}

impl MessagesBridgeState {
    pub(in crate::protocol::adapter) fn new(
        model: &str,
        reasoning_mode: MessagesReasoningMode,
    ) -> Self {
        Self {
            model: model.to_string(),
            portable_history: None,
            system: None,
            historical_system: None,
            messages: Vec::new(),
            tools: None,
            tool_targets: BTreeMap::new(),
            tool_choice: None,
            tool_allow_list: None,
            reasoning_mode,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub(in crate::protocol::adapter) fn append_assistant_content(&mut self, content: Vec<Value>) {
        if !content.is_empty() {
            self.messages
                .push(json!({"role": "assistant", "content": content}));
        }
    }

    pub(in crate::protocol::adapter) fn upstream_tools(&self) -> Option<Vec<Value>> {
        let mut tools = self.tools.clone()?;
        if let Some(allowed) = self.tool_allow_list.as_ref() {
            tools.retain(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| allowed.contains(name))
            });
        }
        (!tools.is_empty()).then_some(tools)
    }

    pub(in crate::protocol::adapter) fn allows_tool_name(&self, name: &str) -> bool {
        self.upstream_tools().is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|candidate| candidate == name)
            })
        })
    }

    pub(in crate::protocol::adapter) fn client_tool(
        &self,
        upstream_name: &str,
    ) -> Option<&ClientToolTarget> {
        self.tool_targets
            .get(upstream_name)
            .filter(|_| self.allows_tool_name(upstream_name))
    }

    pub(in crate::protocol::adapter) fn client_tool_kind(
        &self,
        upstream_name: &str,
    ) -> Option<ResponsesToolKind> {
        self.client_tool(upstream_name).map(|tool| tool.kind)
    }

    pub(in crate::protocol::adapter) fn upstream_tool_name(
        &self,
        namespace: Option<&str>,
        name: &str,
    ) -> Option<&str> {
        self.tool_targets.iter().find_map(|(upstream_name, tool)| {
            (tool.namespace.as_deref() == namespace
                && tool.name == name
                && self.allows_tool_name(upstream_name))
            .then_some(upstream_name.as_str())
        })
    }

    /// Resolves a client tool choice to the upstream name already registered
    /// for this bridge. A present namespace must be usable text; an absent
    /// namespace selects the unnamed tool. A malformed namespace does not
    /// fall back to another tool.
    pub(in crate::protocol::adapter) fn selected_upstream_tool_name(
        &self,
        tool: &Map<String, Value>,
    ) -> Option<String> {
        let kind = ResponsesToolKind::from_definition(tool).ok()?;
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())?;
        let namespace = match tool.get("namespace") {
            None => None,
            Some(namespace) => Some(
                namespace
                    .as_str()
                    .map(str::trim)
                    .filter(|namespace| !namespace.is_empty())?,
            ),
        };
        let upstream_name = self.upstream_tool_name(namespace, name)?;
        (self.client_tool_kind(upstream_name) == Some(kind)).then(|| upstream_name.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct MessagesBridgeRequest {
    pub(in crate::protocol::adapter) upstream_body: Value,
    pub(in crate::protocol::adapter) state: MessagesBridgeState,
    /// Stable local route scope used when deriving the client-facing
    /// response id. Keeping the scope in the request makes JSON and SSE
    /// translation use the exact same identity rule.
    pub(in crate::protocol::adapter) response_scope: String,
}

impl MessagesBridgeRequest {
    pub fn upstream_body(&self) -> &Value {
        &self.upstream_body
    }

    pub fn state(&self) -> &MessagesBridgeState {
        &self.state
    }

    pub fn response_scope(&self) -> &str {
        &self.response_scope
    }
}

#[derive(Clone, Debug)]
pub struct MessagesBridgeResponse {
    pub response_body: Value,
    pub response_id: String,
    pub continuation: MessagesBridgeState,
}
