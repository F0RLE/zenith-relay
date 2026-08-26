use super::{
    gemini::{self, GeminiBridgeRequest, GeminiBridgeResponse},
    messages,
    stream::{AdapterStreamBridge, MessagesStreamBridge},
};
use crate::{CacheWriteTtl, WireApi};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
/// Describes how a client-facing source binding reaches its upstream endpoint.
///
/// `Native` keeps one wire contract end-to-end. Bridges are explicit because a
/// model name is never enough evidence that an upstream accepts a different
/// request or response format.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceAdapter {
    #[default]
    Native,
    ResponsesToMessages,
    ResponsesToGemini,
}

/// The actual upstream HTTP contract selected by a source binding.
///
/// This stays separate from [`WireApi`], which describes the API Relay
/// presents to its client. An adapter is the only place allowed to change one
/// into another.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum UpstreamProtocol {
    Responses,
    ChatCompletions,
    Messages,
    GeminiGenerateContent,
}

/// Inputs needed to turn one client request into an upstream request.
///
/// The adapter owns the protocol conversion, while the caller resolves the
/// selected source route and any prior bridge state.
pub struct AdapterRequestContext<'a> {
    pub client_wire_api: WireApi,
    pub request: &'a Value,
    pub model: &'a str,
    pub stream: bool,
    pub reasoning_mode: MessagesReasoningMode,
    pub cache_write_ttl: CacheWriteTtl,
    pub previous: Option<MessagesBridgeState>,
    pub response_scope: &'a str,
    pub response_id_seed: &'a str,
}

impl SourceAdapter {
    pub const fn upstream_protocol(self, client_wire_api: WireApi) -> UpstreamProtocol {
        match self {
            Self::Native => match client_wire_api {
                WireApi::Responses => UpstreamProtocol::Responses,
                WireApi::ChatCompletions => UpstreamProtocol::ChatCompletions,
                WireApi::Messages => UpstreamProtocol::Messages,
                WireApi::Gemini => UpstreamProtocol::GeminiGenerateContent,
            },
            Self::ResponsesToMessages => UpstreamProtocol::Messages,
            Self::ResponsesToGemini => UpstreamProtocol::GeminiGenerateContent,
        }
    }

    pub const fn is_passthrough(self) -> bool {
        matches!(self, Self::Native)
    }

    pub const fn uses_local_continuation_state(self) -> bool {
        matches!(self, Self::ResponsesToMessages | Self::ResponsesToGemini)
    }

    pub const fn route_suffix(self, client_wire_api: WireApi) -> &'static str {
        match (client_wire_api, self) {
            (WireApi::Responses, Self::ResponsesToMessages) => "responses_to_messages",
            (WireApi::Responses, Self::ResponsesToGemini) => "responses_to_gemini",
            (WireApi::Responses, Self::Native) => "responses",
            (WireApi::ChatCompletions, Self::Native) => "chat_completions",
            (WireApi::Messages, Self::Native) => "messages",
            (WireApi::Gemini, Self::Native) => "gemini",
            // Validation rejects this combination today. Keeping a stable
            // fallback makes candidate identity forward-compatible with a
            // future adapter that targets another upstream contract.
            (_, Self::ResponsesToMessages | Self::ResponsesToGemini) => "bridge",
        }
    }

    pub fn validate(
        self,
        client_wire_api: WireApi,
        reasoning_mode: MessagesReasoningMode,
    ) -> AdapterResult<()> {
        match self {
            Self::Native if reasoning_mode == MessagesReasoningMode::Disabled => Ok(()),
            Self::Native => Err(AdapterError::reasoning_unsupported()),
            Self::ResponsesToMessages if client_wire_api == WireApi::Responses => Ok(()),
            Self::ResponsesToGemini if client_wire_api == WireApi::Responses => Ok(()),
            Self::ResponsesToMessages => Err(AdapterError::unsupported_binding()),
            Self::ResponsesToGemini => Err(AdapterError::unsupported_binding()),
        }
    }

    /// Builds the upstream request contract without making a network call.
    ///
    /// Native routes preserve the client body apart from the resolved source
    /// model. Bridges own all request conversion and later own the matching
    /// response conversion, so the gateway never needs to infer behavior from
    /// a provider name or model family.
    pub fn prepare_request(
        self,
        context: AdapterRequestContext<'_>,
    ) -> AdapterResult<PreparedAdapterRequest> {
        let AdapterRequestContext {
            client_wire_api,
            request,
            model,
            stream,
            reasoning_mode,
            cache_write_ttl,
            previous,
            response_scope,
            response_id_seed,
        } = context;
        self.validate(client_wire_api, reasoning_mode)?;
        match self {
            Self::Native => {
                let mut upstream_body = request.clone();
                let object = upstream_body
                    .as_object_mut()
                    .ok_or_else(AdapterError::invalid_request)?;
                if client_wire_api == WireApi::Gemini {
                    // Gemini places the model in the endpoint path. A model
                    // field is not part of the generateContent contract and
                    // some providers reject it as an unknown field.
                    object.remove("model");
                } else {
                    object.insert("model".to_string(), Value::String(model.to_string()));
                }
                if client_wire_api == WireApi::Messages {
                    messages::apply_cache_write_ttl(&mut upstream_body, cache_write_ttl)?;
                }
                Ok(PreparedAdapterRequest::Native { upstream_body })
            }
            Self::ResponsesToMessages => {
                messages::prepare_responses_to_messages_scoped_with_cache_ttl(
                    request,
                    model,
                    stream,
                    reasoning_mode,
                    cache_write_ttl,
                    previous,
                    response_scope,
                )
                .map(|request| PreparedAdapterRequest::ResponsesToMessages {
                    request: Box::new(request),
                })
            }
            Self::ResponsesToGemini => gemini::prepare_responses_to_gemini_with_reasoning(
                request,
                model,
                stream,
                reasoning_mode,
                previous,
                response_scope,
                response_id_seed,
            )
            .map(|request| PreparedAdapterRequest::ResponsesToGemini {
                request: Box::new(request),
            }),
        }
    }
}

/// The internal upstream thinking contract used by a Messages bridge.
/// Persisted source bindings normalize to `Adaptive`; the enum remains part of
/// the adapter contract and focused protocol tests.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessagesReasoningMode {
    #[default]
    Disabled,
    Budget,
    Adaptive,
}

impl MessagesReasoningMode {
    /// Returns whether the Responses-to-Messages bridge can represent the
    /// requested Codex effort on this upstream route.
    ///
    /// The bridge may advertise only efforts it can actually translate. Native
    /// Responses routes do not use this list: they preserve a provider's
    /// confirmed effort value verbatim.
    pub(crate) fn supports_effort(self, effort: &str) -> bool {
        let effort = effort.trim().to_ascii_lowercase();
        match self {
            Self::Disabled => false,
            Self::Budget | Self::Adaptive => matches!(
                effort.as_str(),
                "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdapterError {
    code: &'static str,
    message: &'static str,
}

impl AdapterError {
    pub const fn code(self) -> &'static str {
        self.code
    }

    pub const fn message(self) -> &'static str {
        self.message
    }

    pub fn is_upstream_failure(self) -> bool {
        matches!(
            self.code,
            "adapter_upstream_response_invalid" | "adapter_upstream_stream_invalid"
        )
    }

    pub(crate) fn is_route_incompatible(self) -> bool {
        matches!(
            self.code,
            "adapter_tool_unsupported" | "adapter_reasoning_unsupported"
        )
    }

    pub(super) const fn invalid_request() -> Self {
        Self {
            code: "adapter_invalid_request",
            message: "request cannot be represented by the selected source adapter",
        }
    }

    pub(super) const fn continuation_missing() -> Self {
        Self {
            code: "adapter_continuation_missing",
            message: "the adapter no longer has the prior response needed for this continuation",
        }
    }

    pub(super) const fn continuation_mismatch() -> Self {
        Self {
            code: "adapter_continuation_mismatch",
            message: "the continuation belongs to a different model or source route",
        }
    }

    pub(crate) const fn unsupported_binding() -> Self {
        Self {
            code: "adapter_binding_unsupported",
            message: "the selected adapter cannot serve this client protocol",
        }
    }

    pub(super) const fn unsupported_tool() -> Self {
        Self {
            code: "adapter_tool_unsupported",
            message: "the selected source adapter supports JSON-schema function and direct custom text tools only",
        }
    }

    pub(super) const fn reasoning_unsupported() -> Self {
        Self {
            code: "adapter_reasoning_unsupported",
            message: "the selected source adapter does not expose reasoning for this binding",
        }
    }

    pub(crate) const fn upstream_response_invalid() -> Self {
        Self {
            code: "adapter_upstream_response_invalid",
            message: "the upstream response cannot be represented as a Responses response",
        }
    }

    pub(super) const fn upstream_stream_invalid() -> Self {
        Self {
            code: "adapter_upstream_stream_invalid",
            message: "the upstream stream cannot be represented as a Responses stream",
        }
    }
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for AdapterError {}

pub type AdapterResult<T> = std::result::Result<T, AdapterError>;

/// The original Responses contract expected by the client for one tool name.
///
/// Anthropic always represents a tool invocation as an object, so a direct
/// custom tool uses the internal `input` string field until it is translated
/// back at the client boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ResponsesToolKind {
    Function,
    Custom,
}

/// One client-visible tool represented by an upstream Messages tool name.
///
/// Namespace functions have only a local name inside the Responses contract,
/// while Messages requires one flat, globally unique tool name. The bridge
/// therefore records both identities and never asks the client to execute an
/// opaque generated upstream name.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct ClientToolTarget {
    pub(super) kind: ResponsesToolKind,
    pub(super) name: String,
    pub(super) namespace: Option<String>,
}

impl ResponsesToolKind {
    pub(super) fn from_definition(tool: &Map<String, Value>) -> AdapterResult<Self> {
        match tool.get("type").and_then(Value::as_str) {
            Some("function") => Ok(Self::Function),
            Some("custom") => Ok(Self::Custom),
            // Responses namespace children have historically omitted `type`
            // for ordinary client functions. A named, untyped definition is
            // still representable as a JSON-schema function for Messages.
            None if tool
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| !name.trim().is_empty()) =>
            {
                Ok(Self::Function)
            }
            _ => Err(AdapterError::unsupported_tool()),
        }
    }

    pub(super) fn from_call_item(item: &Map<String, Value>) -> AdapterResult<Self> {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => Ok(Self::Function),
            Some("custom_tool_call") => Ok(Self::Custom),
            _ => Err(AdapterError::invalid_request()),
        }
    }

    pub(super) fn from_output_item(item: &Map<String, Value>) -> AdapterResult<Self> {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call_output") => Ok(Self::Function),
            Some("custom_tool_call_output") => Ok(Self::Custom),
            _ => Err(AdapterError::invalid_request()),
        }
    }

    pub(super) const fn response_item_type(self) -> &'static str {
        match self {
            Self::Function => "function_call",
            Self::Custom => "custom_tool_call",
        }
    }
}

#[derive(Debug)]
pub(super) struct TranslatedTools {
    pub(super) upstream: Vec<Value>,
    pub(super) client_tools: BTreeMap<String, ClientToolTarget>,
}

/// Volatile continuation state for a Responses-to-Messages bridge. It is
/// intentionally local-only and is never serialized into diagnostics or usage
/// records because it contains the user's conversation content.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MessagesBridgeState {
    pub(super) model: String,
    pub(super) system: Option<Value>,
    pub(super) messages: Vec<Value>,
    pub(super) tools: Option<Vec<Value>>,
    pub(super) tool_targets: BTreeMap<String, ClientToolTarget>,
    pub(super) tool_choice: Option<Value>,
    pub(super) tool_allow_list: Option<BTreeSet<String>>,
    pub(super) reasoning_mode: MessagesReasoningMode,
}

impl MessagesBridgeState {
    pub(super) fn new(model: &str, reasoning_mode: MessagesReasoningMode) -> Self {
        Self {
            model: model.to_string(),
            system: None,
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

    pub(super) fn append_assistant_content(&mut self, content: Vec<Value>) {
        if !content.is_empty() {
            self.messages
                .push(json!({"role": "assistant", "content": content}));
        }
    }

    pub(super) fn upstream_tools(&self) -> Option<Vec<Value>> {
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

    pub(super) fn allows_tool_name(&self, name: &str) -> bool {
        self.upstream_tools().is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|candidate| candidate == name)
            })
        })
    }

    pub(super) fn client_tool(&self, upstream_name: &str) -> Option<&ClientToolTarget> {
        self.tool_targets
            .get(upstream_name)
            .filter(|_| self.allows_tool_name(upstream_name))
    }

    pub(super) fn client_tool_kind(&self, upstream_name: &str) -> Option<ResponsesToolKind> {
        self.client_tool(upstream_name).map(|tool| tool.kind)
    }

    pub(super) fn upstream_tool_name(&self, namespace: Option<&str>, name: &str) -> Option<&str> {
        self.tool_targets.iter().find_map(|(upstream_name, tool)| {
            (tool.namespace.as_deref() == namespace
                && tool.name == name
                && self.allows_tool_name(upstream_name))
            .then_some(upstream_name.as_str())
        })
    }
}

/// Local fallback state for a native Responses route whose upstream only
/// supports `previous_response_id` over WebSocket v2.
///
/// The initial request and completed output are kept in memory so the next
/// request can replay the conversation without pretending that the upstream
/// response id is portable across transports. This is deliberately separate
/// from `ResponsesToMessages`: no protocol conversion happens here.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NativeResponsesReplayState {
    pub(super) model: String,
    request: Value,
    output: Vec<Value>,
}

impl NativeResponsesReplayState {
    pub fn from_response(request: &Value, model: &str, upstream: &Value) -> Option<(String, Self)> {
        let response = upstream
            .pointer("/response/response")
            .or_else(|| upstream.get("response"))
            .unwrap_or(upstream);
        let response_id = response
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())?
            .to_string();
        let request = request.as_object()?.clone();
        if !request.contains_key("input") {
            return None;
        }
        let output = response
            .get("output")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Some((
            response_id,
            Self {
                model: model.to_string(),
                request: Value::Object(request),
                output,
            },
        ))
    }

    /// Builds a new native Responses request with the prior turn materialized
    /// in `input`. The caller invokes this only after the upstream explicitly
    /// rejected `previous_response_id` before the stream is committed.
    pub fn replay_request(
        &self,
        continuation: &Value,
        model: &str,
        stream: bool,
    ) -> AdapterResult<Value> {
        if !self.model.eq_ignore_ascii_case(model) {
            return Err(AdapterError::continuation_mismatch());
        }
        let continuation = continuation
            .as_object()
            .ok_or_else(AdapterError::invalid_request)?;
        let mut request = self
            .request
            .as_object()
            .cloned()
            .ok_or_else(AdapterError::invalid_request)?;
        let initial_input = request
            .remove("input")
            .ok_or_else(AdapterError::invalid_request)?;
        let current_input = continuation
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?;
        let mut input = Vec::new();
        append_replay_input(&mut input, &initial_input)?;
        input.extend(self.output.iter().cloned());
        append_replay_input(&mut input, current_input)?;
        if input.is_empty() {
            return Err(AdapterError::invalid_request());
        }

        for key in [
            "instructions",
            "tools",
            "tool_choice",
            "reasoning",
            "max_output_tokens",
            "temperature",
            "top_p",
            "top_logprobs",
            "truncation",
            "parallel_tool_calls",
            "include",
            "metadata",
            "service_tier",
            "store",
        ] {
            if let Some(value) = continuation.get(key) {
                request.insert(key.to_string(), value.clone());
            }
        }
        request.remove("previous_response_id");
        request.insert("model".to_string(), Value::String(model.to_string()));
        request.insert("stream".to_string(), Value::Bool(stream));
        request.insert("input".to_string(), Value::Array(input));
        Ok(Value::Object(request))
    }
}

fn append_replay_input(target: &mut Vec<Value>, input: &Value) -> AdapterResult<()> {
    match input {
        Value::String(text) => target.push(json!({
            "role": "user",
            "content": [{"type": "input_text", "text": text}],
        })),
        Value::Array(items) => target.extend(items.iter().cloned()),
        Value::Object(item) => target.push(Value::Object(item.clone())),
        _ => return Err(AdapterError::invalid_request()),
    }
    Ok(())
}

/// Repairs a historic Responses function item only after a strict upstream has
/// rejected its item-id namespace.
///
/// `call_id` is the stable link used by `function_call_output`; the item `id`
/// is a separate opaque Responses item identifier. Some compatible upstreams
/// emit the call identifier in both fields, but strict Responses endpoints
/// require the function item identifier to use their `fc_` namespace. Keeping
/// this repair narrow lets native routes stay byte-for-byte passthrough until
/// an upstream proves that its stricter item contract is required.
pub(crate) fn repair_call_prefixed_function_item_ids(request: &mut Value) -> bool {
    let Some(input) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut repaired = false;
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            continue;
        }
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        if id.starts_with("fc_") || id.is_empty() {
            continue;
        }
        item.insert("id".to_string(), Value::String(format!("fc_{id}")));
        repaired = true;
    }
    repaired
}

/// Strict Responses endpoints use a separate `ctc_` namespace for
/// `custom_tool_call.id`. The `call_id` remains the stable link used by the
/// matching `custom_tool_call_output`, so only the item identifier is changed.
pub(super) fn custom_tool_item_id(call_id: &str) -> String {
    let call_id = call_id.trim();
    if call_id.starts_with("ctc_") {
        call_id.to_string()
    } else {
        format!("ctc_{call_id}")
    }
}

/// Repairs a historic Responses custom-tool item only after a strict upstream
/// has rejected its item-id namespace. This is deliberately separate from the
/// function-call repair because the two item types have different namespaces.
pub(crate) fn repair_custom_tool_item_ids(request: &mut Value) -> bool {
    let Some(input) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut repaired = false;
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if item.get("type").and_then(Value::as_str) != Some("custom_tool_call") {
            continue;
        }
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let normalized = custom_tool_item_id(id);
        if normalized != id {
            item.insert("id".to_string(), Value::String(normalized));
            repaired = true;
        }
    }
    repaired
}

/// Drops only foreign `item_` identifiers from message inputs after a strict
/// native Responses endpoint rejects them. Message item IDs are opaque and
/// server-owned, so Relay must not fabricate a `msg_` replacement. Preserve
/// native `msg_` IDs and every non-message item (especially reasoning and
/// tool-call links) exactly as the client supplied them.
pub(crate) fn remove_item_prefixed_message_ids(request: &mut Value) -> bool {
    let Some(input) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut repaired = false;
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        let is_message = item.get("type").and_then(Value::as_str) == Some("message")
            || matches!(
                item.get("role").and_then(Value::as_str),
                Some("user" | "assistant" | "developer" | "system")
            );
        if !is_message
            || !item
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with("item_"))
        {
            continue;
        }
        item.remove("id");
        repaired = true;
    }
    repaired
}

#[derive(Clone, Debug)]
pub struct MessagesBridgeRequest {
    pub(super) upstream_body: Value,
    pub(super) state: MessagesBridgeState,
    /// Stable local route scope used when deriving the client-facing
    /// response id. Keeping the scope in the request makes JSON and SSE
    /// translation use the exact same identity rule.
    pub(super) response_scope: String,
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

/// A translated response from any non-native source binding.
#[derive(Clone, Debug)]
pub enum AdapterResponse {
    Messages(MessagesBridgeResponse),
    Gemini(GeminiBridgeResponse),
}

impl AdapterResponse {
    pub fn response_body(&self) -> &Value {
        match self {
            Self::Messages(response) => &response.response_body,
            Self::Gemini(response) => &response.response_body,
        }
    }

    pub fn response_id(&self) -> &str {
        match self {
            Self::Messages(response) => &response.response_id,
            Self::Gemini(response) => &response.response_id,
        }
    }

    pub fn messages_continuation(&self) -> Option<&MessagesBridgeResponse> {
        match self {
            Self::Messages(response) => Some(response),
            Self::Gemini(_) => None,
        }
    }

    /// Returns the local continuation payload for either bridge. The method
    /// keeps the legacy name above source-compatible while making persistence
    /// protocol-agnostic.
    pub fn continuation(&self) -> Option<(&str, &MessagesBridgeState)> {
        match self {
            Self::Messages(response) => Some((&response.response_id, &response.continuation)),
            Self::Gemini(response) => Some((&response.response_id, &response.continuation)),
        }
    }
}

/// A source-agnostic prepared protocol route.
///
/// It pairs the exact upstream payload with the inverse translation required
/// when the response returns. The gateway only transports this value; it does
/// not need special branches for a provider or model family.
#[derive(Clone, Debug)]
pub enum PreparedAdapterRequest {
    Native { upstream_body: Value },
    ResponsesToMessages { request: Box<MessagesBridgeRequest> },
    ResponsesToGemini { request: Box<GeminiBridgeRequest> },
}

impl PreparedAdapterRequest {
    pub fn upstream_body(&self) -> &Value {
        match self {
            Self::Native { upstream_body } => upstream_body,
            Self::ResponsesToMessages { request } => request.upstream_body(),
            Self::ResponsesToGemini { request } => request.upstream_body(),
        }
    }

    /// Only native routes permit local request normalization after adapter
    /// preparation. Bridge bodies are already a complete upstream contract.
    pub fn native_upstream_body_mut(&mut self) -> Option<&mut Value> {
        match self {
            Self::Native { upstream_body } => Some(upstream_body),
            Self::ResponsesToMessages { .. } | Self::ResponsesToGemini { .. } => None,
        }
    }

    pub const fn is_passthrough(&self) -> bool {
        matches!(self, Self::Native { .. })
    }

    pub const fn requires_bridge_headers(&self) -> bool {
        matches!(
            self,
            Self::ResponsesToMessages { .. } | Self::ResponsesToGemini { .. }
        )
    }

    pub const fn uses_messages_continuation(&self) -> bool {
        matches!(
            self,
            Self::ResponsesToMessages { .. } | Self::ResponsesToGemini { .. }
        )
    }

    /// Translates a completed upstream response only when the selected route
    /// is a bridge. Native bytes remain untouched and can be proxied directly.
    pub fn translate_response_bytes(self, bytes: &[u8]) -> AdapterResult<Option<AdapterResponse>> {
        match self {
            Self::Native { .. } => Ok(None),
            Self::ResponsesToMessages { request } => {
                let upstream = serde_json::from_slice::<Value>(bytes)
                    .map_err(|_| AdapterError::upstream_response_invalid())?;
                messages::translate_messages_response(*request, &upstream)
                    .map(AdapterResponse::Messages)
                    .map(Some)
            }
            Self::ResponsesToGemini { request } => {
                let upstream = serde_json::from_slice::<Value>(bytes)
                    .map_err(|_| AdapterError::upstream_response_invalid())?;
                gemini::translate_gemini_response(*request, &upstream)
                    .map(AdapterResponse::Gemini)
                    .map(Some)
            }
        }
    }

    /// Returns the stream transformer for a bridged route. A native route
    /// intentionally returns `None` so its stream stays byte-for-byte
    /// passthrough.
    pub fn into_stream_bridge(self) -> Option<AdapterStreamBridge> {
        match self {
            Self::Native { .. } => None,
            Self::ResponsesToMessages { request } => Some(AdapterStreamBridge::Messages(Box::new(
                MessagesStreamBridge::new(*request),
            ))),
            Self::ResponsesToGemini { request } => Some(AdapterStreamBridge::Gemini(Box::new(
                super::stream::GeminiStreamBridge::new(*request),
            ))),
        }
    }
}
