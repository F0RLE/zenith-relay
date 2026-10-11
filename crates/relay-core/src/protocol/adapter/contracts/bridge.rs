use super::{
    validate_responses_bridge_request, AdapterError, AdapterResult, ClientToolTarget,
    MessagesReasoningMode,
};
use crate::WireApi;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
pub(in crate::protocol::adapter) fn prepare_bridge_state<'a>(
    request: &'a Value,
    model: &str,
    reasoning_mode: MessagesReasoningMode,
    previous_bridge_state: Option<MessagesBridgeState>,
    wire_api: WireApi,
) -> AdapterResult<(&'a Map<String, Value>, MessagesBridgeState)> {
    let request_object = request
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    validate_responses_bridge_request(request, wire_api)?;
    let has_previous_response = request_object
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|previous_response_id| !previous_response_id.trim().is_empty());
    let mut bridge_state = match (has_previous_response, previous_bridge_state) {
        (true, Some(previous_state)) if previous_state.model == model => previous_state,
        (true, Some(_)) => return Err(AdapterError::continuation_mismatch()),
        (true, None) => return Err(AdapterError::continuation_missing()),
        (false, _) => MessagesBridgeState::new(model, reasoning_mode),
    };
    if bridge_state.reasoning_mode != reasoning_mode {
        return Err(AdapterError::continuation_mismatch());
    }
    bridge_state.system = bridge_state.historical_system.take();
    bridge_state.tool_choice = None;
    bridge_state.tool_allow_list = None;
    Ok((request_object, bridge_state))
}

/// Volatile continuation state for converted protocol routes. It is
/// intentionally local-only and is never serialized into diagnostics or usage
/// records because it contains the user's conversation content.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MessagesBridgeState {
    pub(in crate::protocol::adapter) portable_history:
        Option<Vec<super::super::translation::Message>>,
    pub(in crate::protocol::adapter) response_tools: Option<Vec<Value>>,
    pub(in crate::protocol::adapter) upstream_protocol: Option<WireApi>,
    pub(in crate::protocol::adapter) native_turns: BTreeMap<usize, Vec<Value>>,
    pub(in crate::protocol::adapter) history_key: Option<String>,
    pub(in crate::protocol::adapter) reusable: bool,
    pub(in crate::protocol::adapter) model: String,
    pub(in crate::protocol::adapter) system: Option<Value>,
    pub(in crate::protocol::adapter) historical_system: Option<Value>,
    pub(in crate::protocol::adapter) messages: Vec<Value>,
    pub(in crate::protocol::adapter) tools: Option<Vec<Value>>,
    pub(in crate::protocol::adapter) tool_targets: BTreeMap<String, ClientToolTarget>,
    pub(in crate::protocol::adapter) tool_call_targets: BTreeMap<String, ClientToolTarget>,
    /// Client call IDs are stable even when Gemini supplies its ID later.
    pub(in crate::protocol::adapter) gemini_tool_call_ids: BTreeMap<String, Option<String>>,
    pub(in crate::protocol::adapter) tool_choice: Option<Value>,
    pub(in crate::protocol::adapter) tool_allow_list: Option<BTreeSet<String>>,
    pub(in crate::protocol::adapter) reasoning_mode: MessagesReasoningMode,
}

impl MessagesBridgeState {
    pub(crate) fn storage_key<'a>(&'a self, response_id: &'a str) -> &'a str {
        self.history_key.as_deref().unwrap_or(response_id)
    }

    pub(crate) fn is_reusable(&self) -> bool {
        self.reusable
    }

    pub(in crate::protocol::adapter) fn new(
        model: &str,
        reasoning_mode: MessagesReasoningMode,
    ) -> Self {
        Self {
            model: model.to_string(),
            portable_history: None,
            response_tools: None,
            upstream_protocol: None,
            native_turns: BTreeMap::new(),
            history_key: None,
            reusable: true,
            system: None,
            historical_system: None,
            messages: Vec::new(),
            tools: None,
            tool_targets: BTreeMap::new(),
            tool_call_targets: BTreeMap::new(),
            gemini_tool_call_ids: BTreeMap::new(),
            tool_choice: None,
            tool_allow_list: None,
            reasoning_mode,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub(in crate::protocol::adapter) fn record_tool_call(
        &mut self,
        call_id: &str,
        target: ClientToolTarget,
    ) -> AdapterResult<()> {
        if call_id.trim().is_empty() || self.tool_call_targets.contains_key(call_id) {
            return Err(AdapterError::continuation_mismatch());
        }
        self.tool_call_targets.insert(call_id.to_owned(), target);
        Ok(())
    }

    pub(in crate::protocol::adapter) fn record_response_tool_calls(
        &mut self,
        output: &[Value],
    ) -> AdapterResult<()> {
        for item in output {
            if !matches!(
                item.get("type").and_then(Value::as_str),
                Some("function_call" | "custom_tool_call")
            ) {
                continue;
            }
            let fields = item
                .as_object()
                .ok_or_else(AdapterError::upstream_response_invalid)?;
            let call_id = fields
                .get("call_id")
                .and_then(Value::as_str)
                .ok_or_else(AdapterError::upstream_response_invalid)?;
            let target = ClientToolTarget::from_call(fields)
                .map_err(|_| AdapterError::upstream_response_invalid())?;
            self.record_tool_call(call_id, target)
                .map_err(|_| AdapterError::upstream_response_invalid())?;
        }
        Ok(())
    }

    pub(in crate::protocol::adapter) fn validate_tool_history(
        &self,
        protocol: WireApi,
    ) -> AdapterResult<()> {
        let (assistant_role, content_key, call_key, result_key, result_id_key) = match protocol {
            WireApi::Messages => (
                "assistant",
                "content",
                "tool_use",
                "tool_result",
                "tool_use_id",
            ),
            WireApi::Gemini => ("model", "parts", "functionCall", "functionResponse", "id"),
            _ => return Err(AdapterError::unsupported_binding()),
        };
        let mut seen = BTreeSet::new();
        let mut pending = BTreeMap::new();
        for message in &self.messages {
            let role = message.get("role").and_then(Value::as_str);
            for block in message
                .get(content_key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let (call, result) = if protocol == WireApi::Messages {
                    let kind = block.get("type").and_then(Value::as_str);
                    (
                        (kind == Some(call_key)).then_some(block),
                        (kind == Some(result_key)).then_some(block),
                    )
                } else {
                    (block.get(call_key), block.get(result_key))
                };
                if let Some(call) = call {
                    let id = call
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        .ok_or_else(AdapterError::continuation_mismatch)?;
                    let name = call
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(AdapterError::continuation_mismatch)?;
                    if role != Some(assistant_role) || !seen.insert(id) {
                        return Err(AdapterError::continuation_mismatch());
                    }
                    pending.insert(id, name);
                } else if let Some(result) = result {
                    let id = result
                        .get(result_id_key)
                        .and_then(Value::as_str)
                        .ok_or_else(AdapterError::continuation_mismatch)?;
                    let name = pending
                        .remove(id)
                        .ok_or_else(AdapterError::continuation_mismatch)?;
                    if role != Some("user")
                        || (protocol == WireApi::Gemini
                            && result.get("name").and_then(Value::as_str) != Some(name))
                    {
                        return Err(AdapterError::continuation_mismatch());
                    }
                } else if role != Some(assistant_role) && !pending.is_empty() {
                    return Err(AdapterError::continuation_mismatch());
                }
            }
            if role != Some(assistant_role) && !pending.is_empty() {
                return Err(AdapterError::continuation_mismatch());
            }
        }
        if !pending.is_empty() {
            return Err(AdapterError::continuation_mismatch());
        }
        Ok(())
    }

    pub(in crate::protocol::adapter) fn append_assistant_content(&mut self, content: Vec<Value>) {
        if content.is_empty() {
            return;
        }
        if let Some(previous) = self
            .messages
            .last_mut()
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
            .and_then(|message| message.get_mut("content"))
            .and_then(Value::as_array_mut)
        {
            previous.extend(content);
            return;
        }
        self.messages
            .push(json!({"role": "assistant", "content": content}));
    }

    pub(in crate::protocol::adapter) fn upstream_tools(&self) -> Option<Vec<Value>> {
        let mut tools = self.tools.clone()?;
        if let Some(allowed) = self.tool_allow_list.as_ref() {
            tools.retain(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|allowed_tool_name| allowed.contains(allowed_tool_name))
            });
        }
        (!tools.is_empty()).then_some(tools)
    }

    pub(in crate::protocol::adapter) fn allows_tool_name(&self, upstream_tool_name: &str) -> bool {
        self.tool_allow_list
            .as_ref()
            .is_none_or(|allowed| allowed.contains(upstream_tool_name))
            && self.tools.as_ref().is_some_and(|tools| {
                tools.iter().any(|tool| {
                    tool.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|candidate_tool_name| {
                            candidate_tool_name == upstream_tool_name
                        })
                })
            })
    }

    pub(in crate::protocol::adapter) fn allows_parallel_tool_calls(&self) -> bool {
        self.tool_choice
            .as_ref()
            .and_then(|choice| choice.get("disable_parallel_tool_use"))
            .and_then(Value::as_bool)
            != Some(true)
    }

    pub(in crate::protocol::adapter) fn available_tool_targets(
        &self,
    ) -> BTreeMap<String, ClientToolTarget> {
        self.tool_targets
            .iter()
            .filter(|(name, _)| self.allows_tool_name(name))
            .map(|(name, target)| (name.clone(), target.clone()))
            .collect()
    }

    pub(in crate::protocol::adapter) fn client_tool(
        &self,
        upstream_name: &str,
    ) -> Option<&ClientToolTarget> {
        self.tool_targets
            .get(upstream_name)
            .filter(|_| self.allows_tool_name(upstream_name))
    }
}

#[derive(Clone, Debug)]
pub struct MessagesBridgeRequest {
    pub(in crate::protocol::adapter) upstream_body: Value,
    pub(in crate::protocol::adapter) bridge_state: MessagesBridgeState,
    /// Stable local route scope used when deriving the client-facing
    /// response id. Keeping the scope in the request makes JSON and SSE
    /// translation use the exact same identity rule.
    pub(in crate::protocol::adapter) response_scope: String,
}

impl MessagesBridgeRequest {
    pub fn upstream_body(&self) -> &Value {
        &self.upstream_body
    }

    pub fn bridge_state(&self) -> &MessagesBridgeState {
        &self.bridge_state
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
