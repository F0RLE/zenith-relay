use crate::sources::WireApi;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
    pub previous: Option<MessagesBridgeState>,
    pub response_scope: &'a str,
}

impl SourceAdapter {
    pub const fn upstream_wire_api(self, client_wire_api: WireApi) -> WireApi {
        match self {
            Self::Native => client_wire_api,
            Self::ResponsesToMessages => WireApi::Messages,
        }
    }

    pub const fn is_passthrough(self) -> bool {
        matches!(self, Self::Native)
    }

    pub const fn uses_local_continuation_state(self) -> bool {
        matches!(self, Self::ResponsesToMessages)
    }

    pub const fn route_suffix(self, client_wire_api: WireApi) -> &'static str {
        match (client_wire_api, self) {
            (WireApi::Responses, Self::ResponsesToMessages) => "responses_to_messages",
            (WireApi::Responses, Self::Native) => "responses",
            (WireApi::ChatCompletions, Self::Native) => "chat_completions",
            (WireApi::Messages, Self::Native) => "messages",
            // Validation rejects this combination today. Keeping a stable
            // fallback makes candidate identity forward-compatible with a
            // future adapter that targets another upstream contract.
            (_, Self::ResponsesToMessages) => "bridge",
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
            Self::ResponsesToMessages => Err(AdapterError::unsupported_binding()),
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
            previous,
            response_scope,
        } = context;
        self.validate(client_wire_api, reasoning_mode)?;
        match self {
            Self::Native => {
                let mut upstream_body = request.clone();
                let object = upstream_body
                    .as_object_mut()
                    .ok_or_else(AdapterError::invalid_request)?;
                object.insert("model".to_string(), Value::String(model.to_string()));
                Ok(PreparedAdapterRequest::Native { upstream_body })
            }
            Self::ResponsesToMessages => prepare_responses_to_messages_scoped(
                request,
                model,
                stream,
                reasoning_mode,
                previous,
                response_scope,
            )
            .map(|request| PreparedAdapterRequest::ResponsesToMessages {
                request: Box::new(request),
            }),
        }
    }
}

/// The actual upstream thinking contract for a Messages bridge. It is explicit
/// configuration, not a provider-name heuristic.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessagesReasoningMode {
    #[default]
    Disabled,
    Budget,
    Adaptive,
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

    const fn invalid_request() -> Self {
        Self {
            code: "adapter_invalid_request",
            message: "request cannot be represented by the selected source adapter",
        }
    }

    const fn continuation_missing() -> Self {
        Self {
            code: "adapter_continuation_missing",
            message: "the adapter no longer has the prior response needed for this continuation",
        }
    }

    const fn continuation_mismatch() -> Self {
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

    const fn unsupported_tool() -> Self {
        Self {
            code: "adapter_tool_unsupported",
            message: "the selected source adapter supports JSON-schema function and direct custom text tools only",
        }
    }

    const fn reasoning_unsupported() -> Self {
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

    const fn upstream_stream_invalid() -> Self {
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResponsesToolKind {
    Function,
    Custom,
}

/// One client-visible tool represented by an upstream Messages tool name.
///
/// Namespace functions have only a local name inside the Responses contract,
/// while Messages requires one flat, globally unique tool name. The bridge
/// therefore records both identities and never asks the client to execute an
/// opaque generated upstream name.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ClientToolTarget {
    kind: ResponsesToolKind,
    name: String,
    namespace: Option<String>,
}

impl ResponsesToolKind {
    fn from_definition(tool: &Map<String, Value>) -> AdapterResult<Self> {
        match tool.get("type").and_then(Value::as_str) {
            Some("function") => Ok(Self::Function),
            Some("custom") => Ok(Self::Custom),
            _ => Err(AdapterError::unsupported_tool()),
        }
    }

    fn from_call_item(item: &Map<String, Value>) -> AdapterResult<Self> {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => Ok(Self::Function),
            Some("custom_tool_call") => Ok(Self::Custom),
            _ => Err(AdapterError::invalid_request()),
        }
    }

    fn from_output_item(item: &Map<String, Value>) -> AdapterResult<Self> {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call_output") => Ok(Self::Function),
            Some("custom_tool_call_output") => Ok(Self::Custom),
            _ => Err(AdapterError::invalid_request()),
        }
    }

    const fn response_item_type(self) -> &'static str {
        match self {
            Self::Function => "function_call",
            Self::Custom => "custom_tool_call",
        }
    }
}

#[derive(Debug)]
struct TranslatedTools {
    upstream: Vec<Value>,
    client_tools: BTreeMap<String, ClientToolTarget>,
}

/// Volatile continuation state for a Responses-to-Messages bridge. It is
/// intentionally local-only and is never serialized into diagnostics or usage
/// records because it contains the user's conversation content.
#[derive(Clone, Debug, PartialEq)]
pub struct MessagesBridgeState {
    model: String,
    system: Option<Value>,
    messages: Vec<Value>,
    tools: Option<Vec<Value>>,
    tool_targets: BTreeMap<String, ClientToolTarget>,
    tool_choice: Option<Value>,
    tool_allow_list: Option<BTreeSet<String>>,
    reasoning_mode: MessagesReasoningMode,
}

impl MessagesBridgeState {
    fn new(model: &str, reasoning_mode: MessagesReasoningMode) -> Self {
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

    fn append_assistant_content(&mut self, content: Vec<Value>) {
        if !content.is_empty() {
            self.messages
                .push(json!({"role": "assistant", "content": content}));
        }
    }

    fn upstream_tools(&self) -> Option<Vec<Value>> {
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

    fn allows_tool_name(&self, name: &str) -> bool {
        self.upstream_tools().is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|candidate| candidate == name)
            })
        })
    }

    fn client_tool(&self, upstream_name: &str) -> Option<&ClientToolTarget> {
        self.tool_targets
            .get(upstream_name)
            .filter(|_| self.allows_tool_name(upstream_name))
    }

    fn client_tool_kind(&self, upstream_name: &str) -> Option<ResponsesToolKind> {
        self.client_tool(upstream_name).map(|tool| tool.kind)
    }

    fn upstream_tool_name(&self, namespace: Option<&str>, name: &str) -> Option<&str> {
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
/// HTTP request can replay the conversation without pretending that the
/// upstream response id is portable across transports. This is deliberately
/// separate from `ResponsesToMessages`: no protocol conversion happens here.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeResponsesReplayState {
    model: String,
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
    /// rejected `previous_response_id` on HTTP.
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
        let Some(suffix) = id.strip_prefix("call_").filter(|suffix| !suffix.is_empty()) else {
            continue;
        };
        item.insert("id".to_string(), Value::String(format!("fc_{suffix}")));
        repaired = true;
    }
    repaired
}

#[derive(Clone, Debug)]
pub struct MessagesBridgeRequest {
    upstream_body: Value,
    state: MessagesBridgeState,
    /// Stable local route scope used when deriving the client-facing
    /// response id. Keeping the scope in the request makes JSON and SSE
    /// translation use the exact same identity rule.
    response_scope: String,
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

/// A source-agnostic prepared protocol route.
///
/// It pairs the exact upstream payload with the inverse translation required
/// when the response returns. The gateway only transports this value; it does
/// not need special branches for a provider or model family.
#[derive(Clone, Debug)]
pub enum PreparedAdapterRequest {
    Native { upstream_body: Value },
    ResponsesToMessages { request: Box<MessagesBridgeRequest> },
}

impl PreparedAdapterRequest {
    pub fn upstream_body(&self) -> &Value {
        match self {
            Self::Native { upstream_body } => upstream_body,
            Self::ResponsesToMessages { request } => request.upstream_body(),
        }
    }

    /// Only native routes permit local request normalization after adapter
    /// preparation. Bridge bodies are already a complete upstream contract.
    pub fn native_upstream_body_mut(&mut self) -> Option<&mut Value> {
        match self {
            Self::Native { upstream_body } => Some(upstream_body),
            Self::ResponsesToMessages { .. } => None,
        }
    }

    pub const fn is_passthrough(&self) -> bool {
        matches!(self, Self::Native { .. })
    }

    pub const fn requires_bridge_headers(&self) -> bool {
        matches!(self, Self::ResponsesToMessages { .. })
    }

    /// Translates a completed upstream response only when the selected route
    /// is a bridge. Native bytes remain untouched and can be proxied directly.
    pub fn translate_response_bytes(
        self,
        bytes: &[u8],
    ) -> AdapterResult<Option<MessagesBridgeResponse>> {
        match self {
            Self::Native { .. } => Ok(None),
            Self::ResponsesToMessages { request } => {
                let upstream = serde_json::from_slice::<Value>(bytes)
                    .map_err(|_| AdapterError::upstream_response_invalid())?;
                translate_messages_response(*request, &upstream).map(Some)
            }
        }
    }

    /// Returns the stream transformer for a bridged route. A native route
    /// intentionally returns `None` so its stream stays byte-for-byte
    /// passthrough.
    pub fn into_stream_bridge(self) -> Option<MessagesStreamBridge> {
        match self {
            Self::Native { .. } => None,
            Self::ResponsesToMessages { request } => Some(MessagesStreamBridge::new(*request)),
        }
    }
}

#[derive(Clone, Debug)]
struct StoredBridgeState {
    state: MessagesBridgeState,
    candidate_id: String,
    observed_at_ms: u64,
}

/// Bounded, in-memory state for bridge continuations. Losing this state during
/// a restart is surfaced as a clear continuation error instead of silently
/// sending a context-free tool output upstream.
#[derive(Debug)]
pub struct MessagesBridgeStore {
    entries: BTreeMap<(String, String), StoredBridgeState>,
    max_entries: usize,
    ttl_ms: u64,
}

impl Default for MessagesBridgeStore {
    fn default() -> Self {
        Self::new(256, 60 * 60 * 1_000)
    }
}

#[derive(Clone, Debug)]
struct StoredNativeResponsesReplay {
    state: NativeResponsesReplayState,
    candidate_id: String,
    observed_at_ms: u64,
}

/// Bounded, in-memory state for native Responses HTTP replay. It is never
/// serialized into request logs, diagnostics, or the persisted local store.
#[derive(Debug)]
pub struct NativeResponsesReplayStore {
    entries: BTreeMap<(String, String), StoredNativeResponsesReplay>,
    max_entries: usize,
    ttl_ms: u64,
}

impl Default for NativeResponsesReplayStore {
    fn default() -> Self {
        Self::new(256, 60 * 60 * 1_000)
    }
}

impl NativeResponsesReplayStore {
    pub const fn new(max_entries: usize, ttl_ms: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            ttl_ms,
        }
    }

    pub fn get(
        &mut self,
        local_key_id: &str,
        response_id: &str,
        candidate_id: &str,
        now_ms: u64,
    ) -> Option<NativeResponsesReplayState> {
        self.prune(now_ms);
        self.entries
            .get(&(local_key_id.to_string(), response_id.to_string()))
            .filter(|entry| entry.candidate_id == candidate_id)
            .map(|entry| entry.state.clone())
    }

    pub fn insert(
        &mut self,
        local_key_id: &str,
        response_id: &str,
        candidate_id: &str,
        state: NativeResponsesReplayState,
        now_ms: u64,
    ) {
        self.prune(now_ms);
        if self.max_entries == 0 {
            return;
        }
        while self.entries.len() >= self.max_entries {
            let Some(key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.observed_at_ms)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.entries.remove(&key);
        }
        self.entries.insert(
            (local_key_id.to_string(), response_id.to_string()),
            StoredNativeResponsesReplay {
                state,
                candidate_id: candidate_id.to_string(),
                observed_at_ms: now_ms,
            },
        );
    }

    fn prune(&mut self, now_ms: u64) {
        self.entries
            .retain(|_, entry| now_ms.saturating_sub(entry.observed_at_ms) <= self.ttl_ms);
    }
}

impl MessagesBridgeStore {
    pub const fn new(max_entries: usize, ttl_ms: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            ttl_ms,
        }
    }

    pub fn get(
        &mut self,
        local_key_id: &str,
        response_id: &str,
        candidate_id: &str,
        now_ms: u64,
    ) -> AdapterResult<MessagesBridgeState> {
        self.prune(now_ms);
        let key = (local_key_id.to_string(), response_id.to_string());
        let Some(entry) = self.entries.get(&key) else {
            return Err(AdapterError::continuation_missing());
        };
        if entry.candidate_id != candidate_id {
            return Err(AdapterError::continuation_mismatch());
        }
        Ok(entry.state.clone())
    }

    pub fn insert(
        &mut self,
        local_key_id: &str,
        response_id: &str,
        candidate_id: &str,
        state: MessagesBridgeState,
        now_ms: u64,
    ) {
        self.prune(now_ms);
        if self.max_entries == 0 {
            return;
        }
        while self.entries.len() >= self.max_entries {
            let Some(key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.observed_at_ms)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.entries.remove(&key);
        }
        self.entries.insert(
            (local_key_id.to_string(), response_id.to_string()),
            StoredBridgeState {
                state,
                candidate_id: candidate_id.to_string(),
                observed_at_ms: now_ms,
            },
        );
    }

    fn prune(&mut self, now_ms: u64) {
        self.entries
            .retain(|_, entry| now_ms.saturating_sub(entry.observed_at_ms) <= self.ttl_ms);
    }
}

/// Converts a Codex Responses request to the Anthropic Messages contract.
///
/// JSON-schema functions retain their object input. Direct custom tools are
/// represented as a function with one raw-text field and are translated back
/// to the exact Responses custom-call shape before the client sees them.
/// Provider-hosted, namespace, and dynamic-discovery tools remain rejected:
/// Relay must not claim it can execute or emulate a tool it cannot represent.
pub fn prepare_responses_to_messages(
    request: &Value,
    model: &str,
    stream: bool,
    reasoning_mode: MessagesReasoningMode,
    previous: Option<MessagesBridgeState>,
) -> AdapterResult<MessagesBridgeRequest> {
    prepare_responses_to_messages_scoped(request, model, stream, reasoning_mode, previous, "")
}

/// Variant of [`prepare_responses_to_messages`] that scopes generated local
/// response ids to one runtime route. A provider can legally reuse the same
/// upstream message id on two independent endpoints, so hashing only that
/// upstream id would let one continuation overwrite another.
pub fn prepare_responses_to_messages_scoped(
    request: &Value,
    model: &str,
    stream: bool,
    reasoning_mode: MessagesReasoningMode,
    previous: Option<MessagesBridgeState>,
    response_scope: &str,
) -> AdapterResult<MessagesBridgeRequest> {
    let object = request
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    let previous_response_id = object
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut state = match (previous_response_id, previous) {
        (Some(_), Some(state)) if state.model == model => state,
        (Some(_), Some(_)) => return Err(AdapterError::continuation_mismatch()),
        (Some(_), None) => return Err(AdapterError::continuation_missing()),
        (None, _) => MessagesBridgeState::new(model, reasoning_mode),
    };
    if state.reasoning_mode != reasoning_mode {
        return Err(AdapterError::continuation_mismatch());
    }

    if previous_response_id.is_none() {
        if let Some(instructions) = object.get("instructions") {
            append_system_value(&mut state, instructions)?;
        }
    } else if object.contains_key("instructions") {
        return Err(AdapterError::continuation_mismatch());
    }

    if let Some(tools) = request_tool_catalog(object)? {
        let TranslatedTools {
            upstream,
            client_tools,
        } = translate_tools(&tools)?;
        // Filtering an unsupported hosted tool is safe only when the client
        // also supplied at least one representable tool. With no remaining
        // client tool, silently sending a tool-less Messages request would
        // make Codex believe that its tool request completed normally.
        if !tools.is_empty() && upstream.is_empty() {
            return Err(AdapterError::unsupported_tool());
        }
        state.tools = (!upstream.is_empty()).then_some(upstream);
        state.tool_targets = client_tools;
        // A Responses request that supplies a new tool catalog without an
        // explicit choice returns to the protocol default of automatic
        // selection. Retaining a previous restricted list would silently hide
        // newly supplied tools.
        state.tool_choice = None;
        state.tool_allow_list = None;
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        let translated = translate_tool_choice(tool_choice, &state)?;
        state.tool_choice = translated.value;
        state.tool_allow_list = translated.allowed_names;
    }

    append_responses_input(
        &mut state,
        object
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?,
    )?;
    if state.messages.is_empty() {
        return Err(AdapterError::invalid_request());
    }

    let mut body = Map::from_iter([
        ("model".to_string(), Value::String(model.to_string())),
        ("messages".to_string(), Value::Array(state.messages.clone())),
        ("stream".to_string(), Value::Bool(stream)),
        (
            "max_tokens".to_string(),
            object
                .get("max_output_tokens")
                .cloned()
                .unwrap_or_else(|| Value::from(8_192_u64)),
        ),
    ]);
    if let Some(system) = state.system.clone() {
        body.insert("system".to_string(), system);
    }
    if let Some(tools) = state.upstream_tools() {
        body.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(tool_choice) = state.tool_choice.clone() {
        body.insert("tool_choice".to_string(), tool_choice);
    }
    if let Some(temperature) = object.get("temperature") {
        body.insert("temperature".to_string(), temperature.clone());
    }
    if let Some(top_p) = object.get("top_p") {
        body.insert("top_p".to_string(), top_p.clone());
    }
    if let Some(stop_sequences) = object.get("stop") {
        body.insert("stop_sequences".to_string(), stop_sequences.clone());
    }
    apply_reasoning(&mut body, object.get("reasoning"), reasoning_mode)?;
    Ok(MessagesBridgeRequest {
        upstream_body: Value::Object(body),
        state,
        response_scope: response_scope.trim().to_string(),
    })
}

/// Converts a complete Anthropic Messages response to a complete Responses
/// object and captures the exact native assistant blocks for the next turn.
pub fn translate_messages_response(
    request: MessagesBridgeRequest,
    upstream: &Value,
) -> AdapterResult<MessagesBridgeResponse> {
    let upstream_id = upstream
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(AdapterError::upstream_response_invalid)?;
    let content = upstream
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::upstream_response_invalid)?
        .clone();
    validate_messages_tool_calls(&request.state, &content)?;
    let (mut output, _) = responses_output_from_messages_content(&content, &request.state)?;
    let response_id = bridged_response_id_scoped(request.response_scope(), upstream_id);
    set_message_output_id(&mut output, &response_id);
    let usage = responses_usage(upstream.get("usage"));
    let response_body = json!({
        "id": response_id,
        "object": "response",
        "created_at": upstream
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        "status": "completed",
        "model": request.state.model,
        "output": output,
        "usage": usage,
    });
    let mut continuation = request.state;
    continuation.append_assistant_content(content);
    Ok(MessagesBridgeResponse {
        response_body,
        response_id,
        continuation,
    })
}

pub fn bridged_response_id(upstream_id: &str) -> String {
    bridged_response_id_scoped("", upstream_id)
}

/// Derives a deterministic client-facing id from both the upstream id and the
/// connector route that produced it. Length-prefixing the scope prevents
/// ambiguous concatenations such as `ab` + `c` versus `a` + `bc`.
pub fn bridged_response_id_scoped(scope: &str, upstream_id: &str) -> String {
    if scope.is_empty() {
        let digest = Sha256::digest(upstream_id.as_bytes());
        return format!("resp_bridge_{}", hex::encode(&digest[..12]));
    }
    let mut hasher = Sha256::new();
    hasher.update((scope.len() as u64).to_le_bytes());
    hasher.update(scope.as_bytes());
    hasher.update((upstream_id.len() as u64).to_le_bytes());
    hasher.update(upstream_id.as_bytes());
    let digest = hasher.finalize();
    format!("resp_bridge_{}", hex::encode(&digest[..12]))
}

fn set_message_output_id(output: &mut [Value], response_id: &str) {
    let mut message_index = 0_usize;
    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        if let Some(object) = item.as_object_mut() {
            let id = if message_index == 0 {
                format!("msg_{response_id}")
            } else {
                format!("msg_{response_id}_{message_index}")
            };
            object.insert("id".to_string(), Value::String(id));
            message_index = message_index.saturating_add(1);
        }
    }
}

/// Collects the complete client-side tool catalog for one Responses request.
///
/// Codex can place newly loaded client tools in `input.additional_tools`.
/// Anthropic has no matching deferred-tool wire item, so the bridge loads these
/// functions into the current Messages request together with root `tools`.
/// Hosted-only tools are filtered later by [`translate_tools`], never injected
/// into the Anthropic contract.
fn request_tool_catalog(object: &Map<String, Value>) -> AdapterResult<Option<Vec<Value>>> {
    let mut declared = false;
    let mut tools = Vec::new();
    if let Some(root) = object.get("tools") {
        declared = true;
        tools.extend(
            root.as_array()
                .ok_or_else(AdapterError::invalid_request)?
                .iter()
                .cloned(),
        );
    }
    if let Some(input) = object.get("input").and_then(Value::as_array) {
        for item in input {
            let Some(item) = item.as_object() else {
                continue;
            };
            if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
                continue;
            }
            declared = true;
            tools.extend(
                item.get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(AdapterError::invalid_request)?
                    .iter()
                    .cloned(),
            );
        }
    }
    Ok(declared.then_some(tools))
}

fn append_system_value(state: &mut MessagesBridgeState, value: &Value) -> AdapterResult<()> {
    let text = content_to_messages_blocks(value)?;
    if text.is_empty() {
        return Ok(());
    }
    let next = Value::Array(text);
    match state.system.take() {
        None => state.system = Some(next),
        Some(Value::Array(mut current)) => {
            current.extend(next.as_array().into_iter().flatten().cloned());
            state.system = Some(Value::Array(current));
        }
        Some(_) => return Err(AdapterError::invalid_request()),
    }
    Ok(())
}

fn append_responses_input(state: &mut MessagesBridgeState, input: &Value) -> AdapterResult<()> {
    match input {
        Value::String(text) => append_user_blocks(state, vec![text_block(text)]),
        Value::Array(items) => {
            let mut tool_results = Vec::new();
            for item in items {
                let item = item.as_object().ok_or_else(AdapterError::invalid_request)?;
                if matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("function_call_output" | "custom_tool_call_output")
                ) {
                    tool_results.push(tool_result_block(state, item)?);
                    continue;
                }
                flush_tool_results(state, &mut tool_results)?;
                if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                    // Tool definitions were collected before the Messages body was
                    // built. This Responses control item has no conversation
                    // equivalent and must not be emitted as a user message.
                    continue;
                }
                if let Some(role) = item.get("role").and_then(Value::as_str) {
                    match role {
                        "system" | "developer" => {
                            append_system_value(
                                state,
                                item.get("content")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?;
                        }
                        "user" => append_user_blocks(
                            state,
                            content_to_messages_blocks(
                                item.get("content")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?,
                        )?,
                        "assistant" => append_assistant_from_responses_item(state, item)?,
                        _ => return Err(AdapterError::invalid_request()),
                    }
                    continue;
                }
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call" | "custom_tool_call") => {
                        append_assistant_tool_use(state, item)?
                    }
                    Some("reasoning") => {
                        // A bridge continuation retains native thinking blocks locally. A
                        // standalone Responses reasoning item has no Anthropic signature and
                        // cannot be replayed safely.
                        if state.messages.is_empty() {
                            return Err(AdapterError::continuation_missing());
                        }
                    }
                    Some("message") => {
                        let role = item
                            .get("role")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        if role != "user" {
                            return Err(AdapterError::invalid_request());
                        }
                        append_user_blocks(
                            state,
                            content_to_messages_blocks(
                                item.get("content")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?,
                        )?;
                    }
                    _ => return Err(AdapterError::invalid_request()),
                }
            }
            flush_tool_results(state, &mut tool_results)
        }
        _ => Err(AdapterError::invalid_request()),
    }
}

fn append_assistant_from_responses_item(
    state: &mut MessagesBridgeState,
    item: &Map<String, Value>,
) -> AdapterResult<()> {
    let content = item
        .get("content")
        .map(content_to_messages_blocks)
        .transpose()?;
    if let Some(content) = content.filter(|content| !content.is_empty()) {
        state
            .messages
            .push(json!({"role": "assistant", "content": content}));
    }
    Ok(())
}

fn append_assistant_tool_use(
    state: &mut MessagesBridgeState,
    item: &Map<String, Value>,
) -> AdapterResult<()> {
    let kind = ResponsesToolKind::from_call_item(item)?;
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    let namespace = match item.get("namespace") {
        None => None,
        Some(namespace) => Some(
            namespace
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(AdapterError::invalid_request)?,
        ),
    };
    let upstream_name = state
        .upstream_tool_name(namespace, name)
        .map(str::to_string)
        .ok_or_else(AdapterError::invalid_request)?;
    if state.client_tool_kind(&upstream_name) != Some(kind) {
        return Err(AdapterError::invalid_request());
    }
    let input = match kind {
        ResponsesToolKind::Function => {
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            serde_json::from_str::<Value>(arguments)
                .ok()
                .filter(Value::is_object)
                .ok_or_else(AdapterError::invalid_request)?
        }
        ResponsesToolKind::Custom => {
            let input = item
                .get("input")
                .and_then(Value::as_str)
                .ok_or_else(AdapterError::invalid_request)?;
            json!({"input": input})
        }
    };
    state.messages.push(json!({
        "role": "assistant",
        "content": [{"type": "tool_use", "id": call_id, "name": upstream_name, "input": input}],
    }));
    Ok(())
}

fn append_user_blocks(state: &mut MessagesBridgeState, blocks: Vec<Value>) -> AdapterResult<()> {
    if blocks.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    state
        .messages
        .push(json!({"role": "user", "content": blocks}));
    Ok(())
}

fn flush_tool_results(
    state: &mut MessagesBridgeState,
    results: &mut Vec<Value>,
) -> AdapterResult<()> {
    if results.is_empty() {
        return Ok(());
    }
    let Some(last) = state.messages.last() else {
        return Err(AdapterError::continuation_missing());
    };
    if last.get("role").and_then(Value::as_str) != Some("assistant")
        || !last
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            })
    {
        return Err(AdapterError::continuation_mismatch());
    }
    let known_call_ids = last
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| block.get("id").and_then(Value::as_str))
        .map(|id| (id.to_string(), ()))
        .collect::<BTreeMap<_, _>>();
    let mut returned_call_ids = BTreeMap::new();
    for result in results.iter() {
        let call_id = result
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(AdapterError::continuation_mismatch)?;
        if !known_call_ids.contains_key(call_id)
            || returned_call_ids.insert(call_id.to_string(), ()).is_some()
        {
            return Err(AdapterError::continuation_mismatch());
        }
    }
    let content = std::mem::take(results);
    state
        .messages
        .push(json!({"role": "user", "content": content}));
    Ok(())
}

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

fn content_to_messages_blocks(content: &Value) -> AdapterResult<Vec<Value>> {
    match content {
        Value::String(text) if !text.is_empty() => Ok(vec![text_block(text)]),
        Value::String(_) => Ok(Vec::new()),
        Value::Array(parts) => parts
            .iter()
            .map(|part| {
                if let Some(text) = part.as_str() {
                    return Ok(text_block(text));
                }
                let part = part.as_object().ok_or_else(AdapterError::invalid_request)?;
                match part.get("type").and_then(Value::as_str) {
                    Some("input_text" | "output_text" | "text") => part
                        .get("text")
                        .and_then(Value::as_str)
                        .map(text_block)
                        .ok_or_else(AdapterError::invalid_request),
                    _ => Err(AdapterError::invalid_request()),
                }
            })
            .collect(),
        _ => Err(AdapterError::invalid_request()),
    }
}

fn tool_result_block(
    state: &MessagesBridgeState,
    item: &Map<String, Value>,
) -> AdapterResult<Value> {
    let kind = ResponsesToolKind::from_output_item(item)?;
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    let expected = state
        .messages
        .last()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks.iter().find(|block| {
                block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && block.get("id").and_then(Value::as_str) == Some(call_id)
            })
        })
        .and_then(|block| block.get("name"))
        .and_then(Value::as_str)
        .and_then(|name| state.client_tool(name))
        .ok_or_else(AdapterError::continuation_mismatch)?;
    if kind != expected.kind {
        return Err(AdapterError::continuation_mismatch());
    }
    if let Some(namespace) = item.get("namespace") {
        let namespace = namespace
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(AdapterError::invalid_request)?;
        if expected.namespace.as_deref() != Some(namespace) {
            return Err(AdapterError::continuation_mismatch());
        }
    }
    let content = match (kind, item.get("output")) {
        (ResponsesToolKind::Custom, Some(Value::String(output))) => Value::String(output.clone()),
        (ResponsesToolKind::Custom, _) => return Err(AdapterError::invalid_request()),
        (ResponsesToolKind::Function, Some(Value::String(output))) => Value::String(output.clone()),
        (ResponsesToolKind::Function, Some(value)) => Value::String(
            serde_json::to_string(value).map_err(|_| AdapterError::invalid_request())?,
        ),
        (ResponsesToolKind::Function, None) => Value::String(String::new()),
    };
    Ok(json!({
        "type": "tool_result",
        "tool_use_id": call_id,
        "content": content,
    }))
}

/// An upstream Messages response may only invoke a function that Relay sent in
/// the translated client catalog. This protects the client tool router from an
/// upstream-invented tool name while preserving the exact name supplied by the
/// client when the call is valid.
fn validate_messages_tool_calls(
    state: &MessagesBridgeState,
    content: &[Value],
) -> AdapterResult<()> {
    let mut call_ids = BTreeSet::new();
    for block in content {
        let block = block
            .as_object()
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let id = block
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        if !block.get("input").is_some_and(Value::is_object)
            || !state.allows_tool_name(name)
            || !call_ids.insert(id.to_string())
        {
            return Err(AdapterError::upstream_response_invalid());
        }
    }
    Ok(())
}

fn translate_tools(tools: &[Value]) -> AdapterResult<TranslatedTools> {
    let mut upstream = Vec::with_capacity(tools.len());
    let mut client_tools = BTreeMap::new();
    for tool in tools {
        let tool = tool
            .as_object()
            .ok_or_else(AdapterError::unsupported_tool)?;
        match tool.get("type").and_then(Value::as_str) {
            Some("function" | "custom") => {
                translate_client_tool(&mut upstream, &mut client_tools, tool, None, None)?;
            }
            Some("namespace") => {
                let namespace = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(AdapterError::unsupported_tool)?;
                let namespace_description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let children = tool
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(AdapterError::unsupported_tool)?;
                for child in children {
                    let child = child
                        .as_object()
                        .ok_or_else(AdapterError::unsupported_tool)?;
                    if let Some("function" | "custom") = child.get("type").and_then(Value::as_str) {
                        translate_client_tool(
                            &mut upstream,
                            &mut client_tools,
                            child,
                            Some(namespace),
                            namespace_description,
                        )?;
                    }
                    // The Responses namespace can carry tools that require
                    // server execution or a separate adapter. Do not make a
                    // Messages source advertise them under a fake contract.
                }
            }
            // Hosted and dynamic Responses tools (for example web search and
            // tool search) cannot be executed by an Anthropic Messages source.
            // Omitting them preserves every representable client tool instead
            // of rejecting the complete Codex request.
            _ => {}
        }
    }
    Ok(TranslatedTools {
        upstream,
        client_tools,
    })
}

fn translate_client_tool(
    upstream: &mut Vec<Value>,
    client_tools: &mut BTreeMap<String, ClientToolTarget>,
    tool: &Map<String, Value>,
    namespace: Option<&str>,
    namespace_description: Option<&str>,
) -> AdapterResult<()> {
    let kind = ResponsesToolKind::from_definition(tool)?;
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(AdapterError::unsupported_tool)?;
    let upstream_name = namespace
        .map(|namespace| bridged_namespace_tool_name(namespace, name))
        .unwrap_or_else(|| name.to_string());
    if client_tools.contains_key(&upstream_name) {
        return Err(AdapterError::unsupported_tool());
    }

    let mut translated =
        Map::from_iter([("name".to_string(), Value::String(upstream_name.clone()))]);
    match kind {
        ResponsesToolKind::Function => {
            let mut schema = tool
                .get("parameters")
                .or_else(|| tool.get("input_schema"))
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            let schema = schema
                .as_object_mut()
                .ok_or_else(AdapterError::unsupported_tool)?;
            match schema.get("type").and_then(Value::as_str) {
                Some("object") => {}
                None => {
                    schema.insert("type".to_string(), Value::String("object".to_string()));
                }
                _ => return Err(AdapterError::unsupported_tool()),
            }
            translated.insert("input_schema".to_string(), Value::Object(schema.clone()));
        }
        ResponsesToolKind::Custom => {
            if tool
                .get("defer_loading")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || tool
                    .get("allowed_callers")
                    .is_some_and(|callers| !callers.is_null())
            {
                return Err(AdapterError::unsupported_tool());
            }
            translated.insert("input_schema".to_string(), custom_tool_input_schema(tool)?);
        }
    }
    let tool_description = tool
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(namespace) = namespace {
        let mut description = format!("Codex namespace `{namespace}` tool `{name}`.");
        if let Some(namespace_description) = namespace_description {
            description.push_str(&format!(" {namespace_description}"));
        }
        if let Some(tool_description) = tool_description {
            description.push_str(&format!(" {tool_description}"));
        }
        translated.insert("description".to_string(), Value::String(description));
    } else if let Some(description) = tool_description {
        translated.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
    }
    client_tools.insert(
        upstream_name,
        ClientToolTarget {
            kind,
            name: name.to_string(),
            namespace: namespace.map(str::to_string),
        },
    );
    upstream.push(Value::Object(translated));
    Ok(())
}

fn bridged_namespace_tool_name(namespace: &str, name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((namespace.len() as u64).to_le_bytes());
    hasher.update(namespace.as_bytes());
    hasher.update((name.len() as u64).to_le_bytes());
    hasher.update(name.as_bytes());
    let digest = hasher.finalize();
    format!("relay_ns_{}", hex::encode(&digest[..12]))
}

fn custom_tool_input_schema(tool: &Map<String, Value>) -> AdapterResult<Value> {
    let mut input = Map::from_iter([("type".to_string(), Value::String("string".to_string()))]);
    if let Some(format) = tool.get("format") {
        let format = format
            .as_object()
            .ok_or_else(AdapterError::unsupported_tool)?;
        match format.get("type").and_then(Value::as_str) {
            Some("text") => {}
            Some("grammar") => {
                let syntax = format
                    .get("syntax")
                    .and_then(Value::as_str)
                    .filter(|syntax| matches!(*syntax, "lark" | "regex"))
                    .ok_or_else(AdapterError::unsupported_tool)?;
                let definition = format
                    .get("definition")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|definition| !definition.is_empty())
                    .ok_or_else(AdapterError::unsupported_tool)?;
                input.insert(
                    "description".to_string(),
                    Value::String(format!(
                        "Raw tool input. It must satisfy this {syntax} grammar:\n{definition}"
                    )),
                );
            }
            _ => return Err(AdapterError::unsupported_tool()),
        }
    }
    Ok(json!({
        "type": "object",
        "properties": {"input": Value::Object(input)},
        "required": ["input"],
        "additionalProperties": false,
    }))
}

#[derive(Debug)]
struct TranslatedToolChoice {
    value: Option<Value>,
    allowed_names: Option<BTreeSet<String>>,
}

fn translate_tool_choice(
    tool_choice: &Value,
    state: &MessagesBridgeState,
) -> AdapterResult<TranslatedToolChoice> {
    let has_tools = state.upstream_tools().is_some();
    match tool_choice {
        Value::String(value) => match value.as_str() {
            "auto" => Ok(TranslatedToolChoice {
                value: has_tools.then(|| json!({"type": "auto"})),
                allowed_names: None,
            }),
            "none" => Ok(TranslatedToolChoice {
                value: has_tools.then(|| json!({"type": "none"})),
                allowed_names: None,
            }),
            "required" if has_tools => Ok(TranslatedToolChoice {
                value: Some(json!({"type": "any"})),
                allowed_names: None,
            }),
            "required" => Err(AdapterError::unsupported_tool()),
            _ => Err(AdapterError::unsupported_tool()),
        },
        Value::Object(value)
            if matches!(
                value.get("type").and_then(Value::as_str),
                Some("function" | "custom")
            ) =>
        {
            let name = selected_upstream_tool_name(state, value)?;
            Ok(TranslatedToolChoice {
                value: Some(json!({"type": "tool", "name": name})),
                allowed_names: None,
            })
        }
        Value::Object(value) if value.get("type").and_then(Value::as_str) == Some("namespace") => {
            let namespace = value
                .get("name")
                .or_else(|| value.get("namespace"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|namespace| !namespace.is_empty())
                .ok_or_else(AdapterError::unsupported_tool)?;
            let allowed_names = state
                .tool_targets
                .iter()
                .filter_map(|(upstream_name, target)| {
                    (target.namespace.as_deref() == Some(namespace)
                        && state.allows_tool_name(upstream_name))
                    .then_some(upstream_name.clone())
                })
                .collect::<BTreeSet<_>>();
            if allowed_names.is_empty() {
                return Err(AdapterError::unsupported_tool());
            }
            Ok(TranslatedToolChoice {
                value: Some(json!({"type": "any"})),
                allowed_names: Some(allowed_names),
            })
        }
        Value::Object(value)
            if value.get("type").and_then(Value::as_str) == Some("allowed_tools") =>
        {
            let configured_tools = value
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(AdapterError::unsupported_tool)?;
            let mut allowed_names = BTreeSet::new();
            for tool in configured_tools {
                let tool = tool
                    .as_object()
                    .ok_or_else(AdapterError::unsupported_tool)?;
                match tool.get("type").and_then(Value::as_str) {
                    Some("function" | "custom") => {
                        allowed_names.insert(selected_upstream_tool_name(state, tool)?);
                    }
                    Some("namespace") => {
                        let namespace = tool
                            .get("name")
                            .or_else(|| tool.get("namespace"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|namespace| !namespace.is_empty())
                            .ok_or_else(AdapterError::unsupported_tool)?;
                        allowed_names.extend(state.tool_targets.iter().filter_map(
                            |(upstream_name, target)| {
                                (target.namespace.as_deref() == Some(namespace)
                                    && state.allows_tool_name(upstream_name))
                                .then_some(upstream_name.clone())
                            },
                        ));
                    }
                    // A hosted-only tool cannot be selected through a
                    // Messages bridge. Keep the representable client tools in
                    // an allowed-tools set; fail below when none remain.
                    _ => {}
                }
            }
            if allowed_names.is_empty() {
                return Err(AdapterError::unsupported_tool());
            }
            let value = match value.get("mode").and_then(Value::as_str).unwrap_or("auto") {
                "auto" => json!({"type": "auto"}),
                "required" => json!({"type": "any"}),
                _ => return Err(AdapterError::unsupported_tool()),
            };
            Ok(TranslatedToolChoice {
                value: Some(value),
                allowed_names: Some(allowed_names),
            })
        }
        _ => Err(AdapterError::unsupported_tool()),
    }
}

fn selected_upstream_tool_name(
    state: &MessagesBridgeState,
    tool: &Map<String, Value>,
) -> AdapterResult<String> {
    let kind = ResponsesToolKind::from_definition(tool)?;
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(AdapterError::unsupported_tool)?;
    let namespace = match tool.get("namespace") {
        None => None,
        Some(namespace) => Some(
            namespace
                .as_str()
                .map(str::trim)
                .filter(|namespace| !namespace.is_empty())
                .ok_or_else(AdapterError::unsupported_tool)?,
        ),
    };
    let upstream_name = state
        .upstream_tool_name(namespace, name)
        .ok_or_else(AdapterError::unsupported_tool)?;
    if state.client_tool_kind(upstream_name) != Some(kind) {
        return Err(AdapterError::unsupported_tool());
    }
    Ok(upstream_name.to_string())
}

fn apply_reasoning(
    body: &mut Map<String, Value>,
    reasoning: Option<&Value>,
    mode: MessagesReasoningMode,
) -> AdapterResult<()> {
    let effort = reasoning
        .and_then(Value::as_object)
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|effort| !effort.is_empty() && *effort != "none");
    let Some(effort) = effort else {
        return Ok(());
    };
    match mode {
        MessagesReasoningMode::Disabled => Err(AdapterError::reasoning_unsupported()),
        MessagesReasoningMode::Budget => {
            let budget_tokens = match effort {
                "minimal" => 1_024,
                "low" => 4_096,
                "high" => 16_384,
                "xhigh" => 24_576,
                "max" | "ultra" => 32_000,
                "medium" => 8_192,
                _ => return Err(AdapterError::reasoning_unsupported()),
            };
            let minimum_max_tokens = budget_tokens + 1_024;
            let max_tokens = body
                .get("max_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default()
                .max(minimum_max_tokens);
            body.insert("max_tokens".to_string(), Value::from(max_tokens));
            body.insert(
                "thinking".to_string(),
                json!({"type": "enabled", "budget_tokens": budget_tokens}),
            );
            body.remove("temperature");
            body.remove("top_p");
            Ok(())
        }
        MessagesReasoningMode::Adaptive => {
            let effort = match effort {
                "minimal" => "low",
                "ultra" => "max",
                "low" | "medium" | "high" | "xhigh" | "max" => effort,
                _ => return Err(AdapterError::reasoning_unsupported()),
            };
            body.insert("thinking".to_string(), json!({"type": "adaptive"}));
            body.insert("output_config".to_string(), json!({"effort": effort}));
            body.remove("temperature");
            body.remove("top_p");
            Ok(())
        }
    }
}

fn responses_output_from_messages_content(
    content: &[Value],
    state: &MessagesBridgeState,
) -> AdapterResult<(Vec<Value>, Vec<Value>)> {
    let mut output = Vec::new();
    let mut preserved = Vec::new();
    let mut text = Vec::new();
    let mut text_message_index = 0_usize;
    let flush_text =
        |output: &mut Vec<Value>, text: &mut Vec<Value>, text_message_index: &mut usize| {
            if text.is_empty() {
                return;
            }
            let index = *text_message_index;
            *text_message_index = (*text_message_index).saturating_add(1);
            output.push(json!({
                "id": if index == 0 {
                    "msg_bridge_output".to_string()
                } else {
                    format!("msg_bridge_output_{index}")
                },
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": std::mem::take(text),
            }));
        };
    for block in content {
        let block = block
            .as_object()
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let value = block
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                if !value.is_empty() {
                    text.push(json!({"type": "output_text", "text": value, "annotations": []}));
                }
                preserved.push(Value::Object(block.clone()));
            }
            Some("tool_use") => {
                flush_text(&mut output, &mut text, &mut text_message_index);
                let call_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                let target = state
                    .client_tool(name)
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                let kind = target.kind;
                let client_name = target.name.clone();
                let client_namespace = target.namespace.clone();
                let input = block
                    .get("input")
                    .filter(|value| value.is_object())
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                let mut item = match kind {
                    ResponsesToolKind::Function => json!({
                        "id": call_id,
                        "type": kind.response_item_type(),
                        "status": "completed",
                        "call_id": call_id,
                        "name": client_name.clone(),
                        "arguments": serde_json::to_string(input).map_err(|_| AdapterError::upstream_response_invalid())?,
                    }),
                    ResponsesToolKind::Custom => json!({
                        "id": call_id,
                        "type": kind.response_item_type(),
                        "status": "completed",
                        "call_id": call_id,
                        "name": client_name.clone(),
                        "input": custom_tool_input(input)?,
                    }),
                };
                if let Some(namespace) = client_namespace {
                    item.as_object_mut()
                        .expect("Responses output item is an object")
                        .insert("namespace".to_string(), Value::String(namespace));
                }
                output.push(item);
                preserved.push(Value::Object(block.clone()));
            }
            Some("thinking" | "redacted_thinking") => {
                // The native block (including its signature) must survive in bridge
                // state, but it is intentionally not exposed as fake Responses
                // encrypted content.
                preserved.push(Value::Object(block.clone()));
            }
            _ => return Err(AdapterError::upstream_response_invalid()),
        }
    }
    flush_text(&mut output, &mut text, &mut text_message_index);
    if output.is_empty() {
        return Err(AdapterError::upstream_response_invalid());
    }
    Ok((output, preserved))
}

fn custom_tool_input(input: &Value) -> AdapterResult<&str> {
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

fn responses_usage(usage: Option<&Value>) -> Value {
    let input_tokens = usage
        .and_then(|usage| usage.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output_tokens = usage
        .and_then(|usage| usage.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let mut result = Map::from_iter([
        ("input_tokens".to_string(), Value::from(input_tokens)),
        ("output_tokens".to_string(), Value::from(output_tokens)),
        (
            "total_tokens".to_string(),
            Value::from(input_tokens.saturating_add(output_tokens)),
        ),
    ]);
    if let Some(cache_read) = usage
        .and_then(|usage| usage.get("cache_read_input_tokens"))
        .and_then(Value::as_u64)
    {
        result.insert(
            "input_tokens_details".to_string(),
            json!({"cached_tokens": cache_read}),
        );
    }
    if let Some(cache_write) = usage
        .and_then(|usage| usage.get("cache_creation_input_tokens"))
        .and_then(Value::as_u64)
    {
        result
            .entry("input_tokens_details".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("usage details is an object")
            .insert("cache_write_tokens".to_string(), Value::from(cache_write));
    }
    Value::Object(result)
}

/// Incremental Messages-to-Responses state machine. It owns no network
/// client and can therefore be reused by desktop, server, and contract tests.
#[derive(Debug)]
pub struct MessagesStreamBridge {
    request: Option<MessagesBridgeRequest>,
    model: String,
    pending: Vec<u8>,
    output: VecDeque<Vec<u8>>,
    assistant_blocks: BTreeMap<usize, StreamBlock>,
    closed_blocks: BTreeSet<usize>,
    response_id: Option<String>,
    upstream_id: Option<String>,
    usage: Option<Value>,
    text_output: Option<TextOutput>,
    next_output_index: usize,
    next_message_index: usize,
    completed: Option<MessagesBridgeResponse>,
    terminal: bool,
}

#[derive(Clone, Debug)]
enum StreamBlock {
    Text {
        text: String,
        content_index: Option<usize>,
        output_index: Option<usize>,
    },
    Tool {
        id: String,
        upstream_name: String,
        name: String,
        namespace: Option<String>,
        kind: ResponsesToolKind,
        arguments: String,
        output_index: usize,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
}

#[derive(Debug)]
enum StreamDelta {
    Tool {
        item_id: String,
        output_index: usize,
        delta: String,
    },
    NoOutput,
}

#[derive(Debug)]
struct TextOutput {
    item_id: String,
    output_index: usize,
    next_content_index: usize,
}

impl MessagesStreamBridge {
    pub fn new(request: MessagesBridgeRequest) -> Self {
        Self {
            model: request.state.model.clone(),
            request: Some(request),
            pending: Vec::new(),
            output: VecDeque::new(),
            assistant_blocks: BTreeMap::new(),
            closed_blocks: BTreeSet::new(),
            response_id: None,
            upstream_id: None,
            usage: None,
            text_output: None,
            next_output_index: 0,
            next_message_index: 0,
            completed: None,
            terminal: false,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if self.terminal {
            return;
        }
        self.pending.extend_from_slice(bytes);
        while let Some(end) = sse_event_end(&self.pending) {
            let event = self.pending.drain(..end).collect::<Vec<_>>();
            self.handle_event(&event);
            if self.terminal {
                self.pending.clear();
                return;
            }
        }
    }

    pub fn finish(&mut self) {
        if self.terminal {
            return;
        }
        self.fail(AdapterError::upstream_stream_invalid());
    }

    pub fn pop_output(&mut self) -> Option<Vec<u8>> {
        self.output.pop_front()
    }

    pub fn completed(&self) -> Option<&MessagesBridgeResponse> {
        self.completed.as_ref()
    }

    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    fn handle_event(&mut self, event: &[u8]) {
        let Some(value) = parse_sse_data(event) else {
            if sse_event_has_data(event) {
                self.fail(AdapterError::upstream_stream_invalid());
            }
            return;
        };
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        match kind {
            "message_start" => self.handle_message_start(&value),
            "content_block_start" => self.handle_block_start(&value),
            "content_block_delta"
                if value
                    .get("delta")
                    .and_then(|delta| delta.get("type"))
                    .and_then(Value::as_str)
                    .is_some_and(|delta| {
                        matches!(delta, "citations_delta" | "document" | "compaction_delta")
                    }) => {}
            "content_block_delta" => self.handle_block_delta(&value),
            "content_block_stop" => self.handle_block_stop(&value),
            "message_delta" => {
                if let Some(usage) = value.get("usage") {
                    self.usage = Some(usage.clone());
                }
            }
            "message_stop" => self.complete(),
            // Anthropic may emit keep-alives and metadata deltas which do not
            // change the client-visible Responses output. They must not turn
            // an otherwise valid stream into a synthetic adapter failure.
            "ping" => {}
            "error" => self.fail(AdapterError::upstream_stream_invalid()),
            kind if is_ignorable_metadata_event(kind) => {}
            _ => self.fail(AdapterError::upstream_stream_invalid()),
        }
    }

    fn handle_message_start(&mut self, value: &Value) {
        let Some(message) = value.get("message") else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let Some(upstream_id) = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if self.response_id.is_some() {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        let response_scope = self
            .request
            .as_ref()
            .map_or("", MessagesBridgeRequest::response_scope);
        let response_id = bridged_response_id_scoped(response_scope, upstream_id);
        self.upstream_id = Some(upstream_id.to_string());
        self.response_id = Some(response_id.clone());
        if let Some(usage) = message.get("usage") {
            self.usage = Some(usage.clone());
        }
        self.frame(
            "response.created",
            json!({
                "type": "response.created",
                "response": {
                    "id": response_id,
                    "object": "response",
                    "status": "in_progress",
                    "model": self.model,
                    "output": [],
                }
            }),
        );
    }

    fn handle_block_start(&mut self, value: &Value) {
        let Some(index) = value
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize)
        else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let Some(block) = value.get("content_block").and_then(Value::as_object) else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if self.response_id.is_none()
            || self.assistant_blocks.contains_key(&index)
            || self.closed_blocks.contains(&index)
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let initial_text = block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let allocation = (!initial_text.is_empty())
                    .then(|| self.begin_text_content())
                    .flatten();
                if !initial_text.is_empty() && allocation.is_none() {
                    return;
                }
                self.assistant_blocks.insert(
                    index,
                    StreamBlock::Text {
                        text: initial_text.clone(),
                        content_index: allocation
                            .as_ref()
                            .map(|(_, _, content_index)| *content_index),
                        output_index: allocation
                            .as_ref()
                            .map(|(_, output_index, _)| *output_index),
                    },
                );
                if let Some((item_id, output_index, content_index)) = allocation {
                    self.emit_text_delta(item_id, output_index, content_index, initial_text);
                }
            }
            Some("tool_use") => {
                if !self.finish_active_text_output() {
                    return;
                }
                let Some(id) = block
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                let Some(name) = block
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                let Some(target) = self
                    .request
                    .as_ref()
                    .and_then(|request| request.state.client_tool(name))
                    .cloned()
                else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                let tool_kind = target.kind;
                let client_name = target.name;
                let client_namespace = target.namespace;
                if self.assistant_blocks.values().any(|existing| {
                    matches!(existing, StreamBlock::Tool { id: existing_id, .. } if existing_id == id)
                }) {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
                if block.get("input").is_some_and(|input| !input.is_object()) {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
                let initial_arguments = block
                    .get("input")
                    .filter(|input| input.as_object().is_some_and(|object| !object.is_empty()))
                    .and_then(|input| serde_json::to_string(input).ok())
                    .unwrap_or_default();
                let output_index = self.next_output_index;
                self.next_output_index = self.next_output_index.saturating_add(1);
                let mut item = match tool_kind {
                    ResponsesToolKind::Function => json!({
                        "id": id,
                        "type": tool_kind.response_item_type(),
                        "status": "in_progress",
                        "call_id": id,
                        "name": client_name,
                        "arguments": "",
                    }),
                    ResponsesToolKind::Custom => json!({
                        "id": id,
                        "type": tool_kind.response_item_type(),
                        "status": "in_progress",
                        "call_id": id,
                        "name": client_name,
                        "input": "",
                    }),
                };
                if let Some(namespace) = client_namespace.as_ref() {
                    item.as_object_mut()
                        .expect("Responses stream item is an object")
                        .insert("namespace".to_string(), Value::String(namespace.clone()));
                }
                self.frame(
                    "response.output_item.added",
                    json!({
                        "type": "response.output_item.added",
                        "output_index": output_index,
                        "item": item,
                    }),
                );
                self.assistant_blocks.insert(
                    index,
                    StreamBlock::Tool {
                        id: id.to_string(),
                        upstream_name: name.to_string(),
                        name: client_name,
                        namespace: client_namespace,
                        kind: tool_kind,
                        arguments: initial_arguments.clone(),
                        output_index,
                    },
                );
                if tool_kind == ResponsesToolKind::Function && !initial_arguments.is_empty() {
                    self.frame(
                        "response.function_call_arguments.delta",
                        json!({
                            "type": "response.function_call_arguments.delta",
                            "response_id": self.response_id.clone(),
                            "item_id": id,
                            "output_index": output_index,
                            "delta": initial_arguments,
                        }),
                    );
                }
            }
            Some("thinking") => {
                let thinking = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let signature = block
                    .get("signature")
                    .and_then(Value::as_str)
                    .filter(|signature| !signature.is_empty())
                    .map(str::to_string);
                self.assistant_blocks.insert(
                    index,
                    StreamBlock::Thinking {
                        thinking,
                        signature,
                    },
                );
            }
            Some("redacted_thinking") => {
                let Some(data) = block.get("data").and_then(Value::as_str) else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                self.assistant_blocks.insert(
                    index,
                    StreamBlock::RedactedThinking {
                        data: data.to_string(),
                    },
                );
            }
            _ => self.fail(AdapterError::upstream_stream_invalid()),
        }
    }

    fn handle_block_delta(&mut self, value: &Value) {
        let Some(index) = value
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize)
        else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let Some(delta) = value.get("delta").and_then(Value::as_object) else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if self.closed_blocks.contains(&index) {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
            let Some(delta) = delta.get("text").and_then(Value::as_str) else {
                self.fail(AdapterError::upstream_stream_invalid());
                return;
            };
            if delta.is_empty() {
                return;
            }
            let needs_content_part = match self.assistant_blocks.get(&index) {
                Some(StreamBlock::Text { content_index, .. }) => content_index.is_none(),
                _ => {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
            };
            if needs_content_part {
                let Some((_, output_index, content_index)) = self.begin_text_content() else {
                    return;
                };
                let Some(StreamBlock::Text {
                    content_index: block_content_index,
                    output_index: block_output_index,
                    ..
                }) = self.assistant_blocks.get_mut(&index)
                else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                *block_content_index = Some(content_index);
                *block_output_index = Some(output_index);
            }
            let (output_index, content_index) = {
                let Some(StreamBlock::Text {
                    text,
                    content_index: Some(content_index),
                    output_index: Some(output_index),
                }) = self.assistant_blocks.get_mut(&index)
                else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                text.push_str(delta);
                (*output_index, *content_index)
            };
            let Some(text_output) = self
                .text_output
                .as_ref()
                .filter(|output| output.output_index == output_index)
            else {
                self.fail(AdapterError::upstream_stream_invalid());
                return;
            };
            self.emit_text_delta(
                text_output.item_id.clone(),
                output_index,
                content_index,
                delta.to_string(),
            );
            return;
        }
        let stream_delta = {
            let Some(block) = self.assistant_blocks.get_mut(&index) else {
                self.fail(AdapterError::upstream_stream_invalid());
                return;
            };
            match (block, delta.get("type").and_then(Value::as_str)) {
                (
                    StreamBlock::Tool {
                        id,
                        kind,
                        arguments,
                        output_index,
                        ..
                    },
                    Some("input_json_delta"),
                ) => {
                    let Some(delta) = delta.get("partial_json").and_then(Value::as_str) else {
                        self.fail(AdapterError::upstream_stream_invalid());
                        return;
                    };
                    arguments.push_str(delta);
                    if *kind == ResponsesToolKind::Function {
                        StreamDelta::Tool {
                            item_id: id.clone(),
                            output_index: *output_index,
                            delta: delta.to_string(),
                        }
                    } else {
                        StreamDelta::NoOutput
                    }
                }
                (StreamBlock::Thinking { thinking, .. }, Some("thinking_delta")) => {
                    let Some(delta) = delta.get("thinking").and_then(Value::as_str) else {
                        self.fail(AdapterError::upstream_stream_invalid());
                        return;
                    };
                    thinking.push_str(delta);
                    StreamDelta::NoOutput
                }
                (StreamBlock::Thinking { signature, .. }, Some("signature_delta")) => {
                    let Some(delta) = delta.get("signature").and_then(Value::as_str) else {
                        self.fail(AdapterError::upstream_stream_invalid());
                        return;
                    };
                    signature.get_or_insert_with(String::new).push_str(delta);
                    StreamDelta::NoOutput
                }
                _ => {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                }
            }
        };
        match stream_delta {
            StreamDelta::Tool {
                item_id,
                output_index,
                delta,
            } => {
                let response_id = self.response_id.clone();
                self.frame(
                    "response.function_call_arguments.delta",
                    json!({
                        "type": "response.function_call_arguments.delta",
                        "response_id": response_id,
                        "item_id": item_id,
                        "output_index": output_index,
                        "delta": delta,
                    }),
                );
            }
            StreamDelta::NoOutput => {}
        }
    }

    fn handle_block_stop(&mut self, value: &Value) {
        let Some(index) = value
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize)
        else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if !self.closed_blocks.insert(index) {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        let Some(block) = self.assistant_blocks.get(&index).cloned() else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        match block {
            StreamBlock::Text {
                text: block_text,
                content_index,
                output_index,
            } => {
                let (Some(content_index), Some(output_index)) = (content_index, output_index)
                else {
                    // Anthropic may emit an empty text block before a tool
                    // block. It has no client-visible Responses equivalent,
                    // so do not manufacture an empty message output item.
                    return;
                };
                let Some(text_output) = self
                    .text_output
                    .as_ref()
                    .filter(|output| output.output_index == output_index)
                else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                let item_id = text_output.item_id.clone();
                let response_id = self.response_id.clone();
                self.frame(
                    "response.output_text.done",
                    json!({
                        "type": "response.output_text.done",
                        "response_id": response_id,
                        "item_id": item_id.clone(),
                        "output_index": output_index,
                        "content_index": content_index,
                        "text": block_text,
                    }),
                );
                self.frame(
                    "response.content_part.done",
                    json!({
                        "type": "response.content_part.done",
                        "response_id": response_id,
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": content_index,
                    }),
                );
            }
            StreamBlock::Tool {
                id,
                name,
                namespace,
                kind,
                arguments: raw_arguments,
                output_index,
                ..
            } => {
                let arguments = if raw_arguments.trim().is_empty() {
                    "{}".to_string()
                } else {
                    raw_arguments
                };
                let Some(input) = tool_arguments_value(&arguments) else {
                    self.fail(AdapterError::upstream_stream_invalid());
                    return;
                };
                match kind {
                    ResponsesToolKind::Function => {
                        let response_id = self.response_id.clone();
                        let mut arguments_done = json!({
                            "type": "response.function_call_arguments.done",
                            "response_id": response_id.clone(),
                            "item_id": id.clone(),
                            "call_id": id.clone(),
                            "name": name.clone(),
                            "output_index": output_index,
                            "arguments": arguments.clone(),
                        });
                        if let Some(namespace) = namespace.as_ref() {
                            arguments_done
                                .as_object_mut()
                                .expect("Responses function call event is an object")
                                .insert("namespace".to_string(), Value::String(namespace.clone()));
                        }
                        self.frame("response.function_call_arguments.done", arguments_done);
                        let mut item = json!({
                            "id": id,
                            "type": kind.response_item_type(),
                            "status": "completed",
                            "call_id": id,
                            "name": name,
                            "arguments": arguments,
                        });
                        if let Some(namespace) = namespace {
                            item.as_object_mut()
                                .expect("Responses function call item is an object")
                                .insert("namespace".to_string(), Value::String(namespace));
                        }
                        self.frame(
                            "response.output_item.done",
                            json!({
                                "type": "response.output_item.done",
                                "response_id": response_id,
                                "output_index": output_index,
                                "item": item,
                            }),
                        );
                    }
                    ResponsesToolKind::Custom => {
                        let Ok(raw_input) = custom_tool_input(&input) else {
                            self.fail(AdapterError::upstream_stream_invalid());
                            return;
                        };
                        let response_id = self.response_id.clone();
                        self.frame(
                            "response.custom_tool_call_input.done",
                            json!({
                                "type": "response.custom_tool_call_input.done",
                                "response_id": response_id.clone(),
                                "item_id": id.clone(),
                                "output_index": output_index,
                                "input": raw_input,
                            }),
                        );
                        let mut item = json!({
                            "id": id,
                            "type": kind.response_item_type(),
                            "status": "completed",
                            "call_id": id,
                            "name": name,
                            "input": raw_input,
                        });
                        if let Some(namespace) = namespace {
                            item.as_object_mut()
                                .expect("Responses custom tool item is an object")
                                .insert("namespace".to_string(), Value::String(namespace));
                        }
                        self.frame(
                            "response.output_item.done",
                            json!({
                                "type": "response.output_item.done",
                                "response_id": response_id,
                                "output_index": output_index,
                                "item": item,
                            }),
                        );
                    }
                }
            }
            StreamBlock::Thinking { .. } | StreamBlock::RedactedThinking { .. } => {}
        }
    }

    fn ensure_text_output(&mut self) -> Option<&mut TextOutput> {
        if self.text_output.is_none() {
            let response_id = self.response_id.as_deref()?;
            let message_index = self.next_message_index;
            self.next_message_index = self.next_message_index.saturating_add(1);
            let item_id = if message_index == 0 {
                format!("msg_{response_id}")
            } else {
                format!("msg_{response_id}_{message_index}")
            };
            let output_index = self.next_output_index;
            self.next_output_index = self.next_output_index.saturating_add(1);
            self.frame(
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {
                        "id": item_id.clone(),
                        "type": "message",
                        "status": "in_progress",
                        "role": "assistant",
                        "content": [],
                    }
                }),
            );
            self.text_output = Some(TextOutput {
                item_id,
                output_index,
                next_content_index: 0,
            });
        }
        self.text_output.as_mut()
    }

    fn begin_text_content(&mut self) -> Option<(String, usize, usize)> {
        let (item_id, output_index, content_index) = {
            let text_output = self.ensure_text_output()?;
            let content_index = text_output.next_content_index;
            text_output.next_content_index = text_output.next_content_index.saturating_add(1);
            (
                text_output.item_id.clone(),
                text_output.output_index,
                content_index,
            )
        };
        self.frame(
            "response.content_part.added",
            json!({
                "type": "response.content_part.added",
                "item_id": item_id.clone(),
                "output_index": output_index,
                "content_index": content_index,
                "part": {"type": "output_text", "text": ""},
            }),
        );
        (!self.terminal).then_some((item_id, output_index, content_index))
    }

    fn emit_text_delta(
        &mut self,
        item_id: String,
        output_index: usize,
        content_index: usize,
        delta: String,
    ) {
        let Some(response_id) = self.response_id.clone() else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        self.frame(
            "response.output_text.delta",
            json!({
                "type": "response.output_text.delta",
                "response_id": response_id,
                "item_id": item_id,
                "output_index": output_index,
                "content_index": content_index,
                "delta": delta,
            }),
        );
    }

    fn finish_active_text_output(&mut self) -> bool {
        let Some(text_output) = self.text_output.take() else {
            return true;
        };
        let mut parts = self
            .assistant_blocks
            .iter()
            .filter_map(|(block_index, block)| match block {
                StreamBlock::Text {
                    text,
                    content_index: Some(content_index),
                    output_index: Some(output_index),
                } if *output_index == text_output.output_index => {
                    Some((*block_index, *content_index, text.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if parts.is_empty()
            || parts
                .iter()
                .any(|(block_index, _, _)| !self.closed_blocks.contains(block_index))
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return false;
        }
        parts.sort_by_key(|(_, content_index, _)| *content_index);
        if parts
            .iter()
            .enumerate()
            .any(|(expected, (_, content_index, _))| *content_index != expected)
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return false;
        }
        let content = Value::Array(
            parts
                .into_iter()
                .map(|(_, _, text)| json!({"type": "output_text", "text": text, "annotations": []}))
                .collect(),
        );
        self.frame(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "response_id": self.response_id.clone(),
                "output_index": text_output.output_index,
                "item": {
                    "id": text_output.item_id,
                    "type": "message",
                    "status": "completed",
                    "role": "assistant",
                    "content": content,
                }
            }),
        );
        !self.terminal
    }

    fn complete(&mut self) {
        if self.response_id.is_none()
            || self.assistant_blocks.is_empty()
            || self.closed_blocks.len() != self.assistant_blocks.len()
        {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        if !self.finish_active_text_output() {
            return;
        }
        let Some(request) = self.request.take() else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let Some(upstream_id) = self.upstream_id.clone() else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        let content = self
            .assistant_blocks
            .values()
            .map(|block| match block {
                StreamBlock::Text { text, .. } => Ok(json!({"type": "text", "text": text})),
                StreamBlock::Tool {
                    id,
                    upstream_name,
                    arguments,
                    ..
                } => {
                    let input = tool_arguments_value(arguments)
                        .ok_or_else(AdapterError::upstream_stream_invalid)?;
                    Ok(json!({"type": "tool_use", "id": id, "name": upstream_name, "input": input}))
                }
                StreamBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    let mut block = Map::from_iter([
                        ("type".to_string(), Value::String("thinking".to_string())),
                        ("thinking".to_string(), Value::String(thinking.clone())),
                    ]);
                    if let Some(signature) = signature {
                        block.insert("signature".to_string(), Value::String(signature.clone()));
                    }
                    Ok(Value::Object(block))
                }
                StreamBlock::RedactedThinking { data } => {
                    Ok(json!({"type": "redacted_thinking", "data": data}))
                }
            })
            .collect::<AdapterResult<Vec<_>>>();
        let Ok(content) = content else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if validate_messages_tool_calls(&request.state, &content).is_err() {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        let (mut output, _) = match responses_output_from_messages_content(&content, &request.state)
        {
            Ok(value) => value,
            Err(error) => {
                self.fail(error);
                return;
            }
        };
        let Some(response_id) = self.response_id.clone() else {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        };
        if response_id != bridged_response_id_scoped(request.response_scope(), &upstream_id) {
            self.fail(AdapterError::upstream_stream_invalid());
            return;
        }
        set_message_output_id(&mut output, &response_id);
        let response_body = json!({
            "id": response_id,
            "object": "response",
            "created_at": 0,
            "status": "completed",
            "model": self.model,
            "output": output,
            "usage": responses_usage(self.usage.as_ref()),
        });
        let mut continuation = request.state;
        continuation.append_assistant_content(content);
        self.completed = Some(MessagesBridgeResponse {
            response_body: response_body.clone(),
            response_id: response_id.clone(),
            continuation,
        });
        self.frame(
            "response.completed",
            json!({
                "type": "response.completed",
                "response": response_body,
            }),
        );
        self.terminal = true;
    }

    fn fail(&mut self, error: AdapterError) {
        if self.terminal {
            return;
        }
        let response_id = self
            .response_id
            .clone()
            .unwrap_or_else(|| "resp_bridge_stream_failed".to_string());
        self.frame(
            "response.failed",
            json!({
                "type": "response.failed",
                "response": {
                    "id": response_id,
                    "object": "response",
                    "status": "failed",
                    "model": self.model,
                    "output": [],
                    "error": {
                        "type": "invalid_request_error",
                        "code": error.code(),
                        "message": error.message(),
                    }
                }
            }),
        );
        self.terminal = true;
    }

    fn frame(&mut self, event: &str, payload: Value) {
        let Ok(payload) = serde_json::to_vec(&payload) else {
            self.terminal = true;
            return;
        };
        let mut frame = Vec::with_capacity(event.len() + payload.len() + 20);
        frame.extend_from_slice(b"event: ");
        frame.extend_from_slice(event.as_bytes());
        frame.extend_from_slice(b"\ndata: ");
        frame.extend_from_slice(&payload);
        frame.extend_from_slice(b"\n\n");
        self.output.push_back(frame);
    }
}

fn parse_sse_data(event: &[u8]) -> Option<Value> {
    let mut data = Vec::new();
    for line in event.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(value) = line.strip_prefix(b"data:") else {
            continue;
        };
        if !data.is_empty() {
            data.push(b'\n');
        }
        data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
    }
    (!data.is_empty())
        .then(|| serde_json::from_slice(&data).ok())
        .flatten()
}

fn sse_event_has_data(event: &[u8]) -> bool {
    event.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        line.strip_prefix(b"data:")
            .is_some_and(|value| value.iter().any(|byte| !byte.is_ascii_whitespace()))
    })
}

fn is_ignorable_metadata_event(kind: &str) -> bool {
    matches!(
        kind,
        "message_metadata" | "content_block_metadata" | "citation" | "message_citation"
    ) || kind.ends_with("_metadata")
        || kind.ends_with("_citation")
}

fn tool_arguments_value(arguments: &str) -> Option<Value> {
    if arguments.trim().is_empty() {
        return Some(json!({}));
    }
    serde_json::from_str::<Value>(arguments)
        .ok()
        .filter(Value::is_object)
}

fn sse_event_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| position + 2)
        .or_else(|| {
            bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(input: Value) -> Value {
        json!({
            "model": "claude-test",
            "input": input,
            "tools": [{
                "type": "function",
                "name": "run_command",
                "description": "Run a command",
                "parameters": {
                    "type": "object",
                    "properties": {"command": {"type": "string"}},
                    "required": ["command"]
                }
            }]
        })
    }

    #[test]
    fn native_prepared_request_is_transparent_for_opaque_tools() {
        let request = json!({
            "model": "alias",
            "input": "inspect",
            "tools": [{
                "type": "computer_use_preview",
                "name": "PowerShell",
                "display_width": 1200,
                "display_height": 800
            }]
        });
        let prepared = SourceAdapter::Native
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Responses,
                request: &request,
                model: "resolved-model",
                stream: false,
                reasoning_mode: MessagesReasoningMode::Disabled,
                previous: None,
                response_scope: "native-route",
            })
            .unwrap();

        assert!(prepared.is_passthrough());
        assert_eq!(prepared.upstream_body()["model"], "resolved-model");
        assert_eq!(prepared.upstream_body()["tools"], request["tools"]);
        assert!(prepared
            .translate_response_bytes(br#"{}"#)
            .unwrap()
            .is_none());
    }

    #[test]
    fn messages_bridge_converts_function_tools_and_preserves_tool_turn_state() {
        let first = prepare_responses_to_messages(
            &request(Value::String("inspect the project".to_string())),
            "claude-test",
            false,
            MessagesReasoningMode::Adaptive,
            None,
        )
        .unwrap();
        assert_eq!(
            first.upstream_body()["tools"][0]["input_schema"]["type"],
            "object"
        );

        let response = translate_messages_response(
            first,
            &json!({
                "id": "msg_01",
                "model": "claude-test",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_01",
                    "name": "run_command",
                    "input": {"command": "pwd"}
                }],
                "stop_reason": "tool_use",
                "usage": {"input_tokens": 12, "output_tokens": 3}
            }),
        )
        .unwrap();
        assert_eq!(response.response_body["output"][0]["type"], "function_call");
        assert_eq!(response.response_body["output"][0]["call_id"], "toolu_01");

        let second = prepare_responses_to_messages(
            &json!({
                "model": "claude-test",
                "previous_response_id": response.response_id,
                "input": [{
                    "type": "function_call_output",
                    "call_id": "toolu_01",
                    "output": "/workspace"
                }]
            }),
            "claude-test",
            false,
            MessagesReasoningMode::Adaptive,
            Some(response.continuation),
        )
        .unwrap();
        assert_eq!(
            second.upstream_body()["messages"][2]["content"][0]["type"],
            "tool_result"
        );
        assert_eq!(
            second.upstream_body()["messages"][2]["content"][0]["tool_use_id"],
            "toolu_01"
        );
    }

    #[test]
    fn messages_bridge_preserves_custom_tool_call_and_output_shapes() {
        let first = prepare_responses_to_messages(
            &json!({
                "model": "claude-test",
                "input": "List the project files.",
                "tools": [{
                    "type": "custom",
                    "name": "PowerShell",
                    "description": "Runs one PowerShell command.",
                    "format": {
                        "type": "grammar",
                        "syntax": "regex",
                        "definition": "[^\\n]+"
                    }
                }],
                "tool_choice": {"type": "custom", "name": "PowerShell"}
            }),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();
        assert_eq!(first.upstream_body()["tools"][0]["name"], "PowerShell");
        assert_eq!(
            first.upstream_body()["tools"][0]["input_schema"]["properties"]["input"]["type"],
            "string"
        );
        assert!(
            first.upstream_body()["tools"][0]["input_schema"]["properties"]["input"]["description"]
                .as_str()
                .unwrap()
                .contains("regex grammar")
        );
        assert_eq!(
            first.upstream_body()["tool_choice"],
            json!({"type": "tool", "name": "PowerShell"})
        );

        let response = translate_messages_response(
            first,
            &json!({
                "id": "msg_custom",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_custom",
                    "name": "PowerShell",
                    "input": {"input": "Get-ChildItem -Force"}
                }]
            }),
        )
        .unwrap();
        assert_eq!(
            response.response_body["output"][0]["type"],
            "custom_tool_call"
        );
        assert_eq!(
            response.response_body["output"][0]["input"],
            "Get-ChildItem -Force"
        );

        let second = prepare_responses_to_messages(
            &json!({
                "model": "claude-test",
                "previous_response_id": response.response_id,
                "input": [{
                    "type": "custom_tool_call_output",
                    "call_id": "toolu_custom",
                    "output": "Cargo.toml\nsrc"
                }]
            }),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            Some(response.continuation),
        )
        .unwrap();
        assert_eq!(
            second.upstream_body()["messages"][2]["content"][0],
            json!({
                "type": "tool_result",
                "tool_use_id": "toolu_custom",
                "content": "Cargo.toml\nsrc"
            })
        );
    }

    #[test]
    fn messages_bridge_rejects_non_text_custom_tool_output() {
        let first = prepare_responses_to_messages(
            &json!({
                "model": "claude-test",
                "input": "List the project files.",
                "tools": [{"type": "custom", "name": "PowerShell"}]
            }),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();
        let response = translate_messages_response(
            first,
            &json!({
                "id": "msg_custom_output",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_custom_output",
                    "name": "PowerShell",
                    "input": {"input": "Get-ChildItem"}
                }]
            }),
        )
        .unwrap();

        let error = prepare_responses_to_messages(
            &json!({
                "model": "claude-test",
                "previous_response_id": response.response_id,
                "input": [{
                    "type": "custom_tool_call_output",
                    "call_id": "toolu_custom_output",
                    "output": [{"type": "input_text", "text": "not a direct text result"}]
                }]
            }),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            Some(response.continuation),
        )
        .unwrap_err();

        assert_eq!(error.code(), "adapter_invalid_request");
    }

    #[test]
    fn messages_bridge_preserves_allowed_tool_subset_without_lie() {
        let mut request = request(Value::String("choose".to_string()));
        request["tools"] = json!([
            {
                "type": "function",
                "name": "run_command",
                "parameters": {"type": "object"}
            },
            {
                "type": "function",
                "name": "read_file",
                "parameters": {"type": "object"}
            }
        ]);
        request["tool_choice"] = json!({
            "type": "allowed_tools",
            "mode": "required",
            "tools": [{"type": "function", "name": "run_command"}]
        });

        let prepared = prepare_responses_to_messages(
            &request,
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();
        assert_eq!(
            prepared.upstream_body()["tools"].as_array().unwrap().len(),
            1
        );
        assert_eq!(prepared.upstream_body()["tools"][0]["name"], "run_command");
        assert_eq!(prepared.upstream_body()["tool_choice"]["type"], "any");
    }

    #[test]
    fn messages_bridge_rejects_an_upstream_tool_that_was_not_declared() {
        let prepared = prepare_responses_to_messages(
            &request(Value::String("choose".to_string())),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();
        let error = translate_messages_response(
            prepared,
            &json!({
                "id": "msg_unknown_tool",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_unknown",
                    "name": "not_declared",
                    "input": {}
                }]
            }),
        )
        .unwrap_err();
        assert_eq!(error.code(), "adapter_upstream_response_invalid");
    }

    #[test]
    fn messages_bridge_preserves_text_and_tool_output_order() {
        let prepared = prepare_responses_to_messages(
            &request(Value::String("ordered".to_string())),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();
        let response = translate_messages_response(
            prepared,
            &json!({
                "id": "msg_ordered",
                "content": [
                    {"type": "text", "text": "before"},
                    {"type": "tool_use", "id": "tool_ordered", "name": "run_command", "input": {"command": "pwd"}},
                    {"type": "text", "text": "after"}
                ]
            }),
        )
        .unwrap();
        let output = response.response_body["output"].as_array().unwrap();
        assert_eq!(output[0]["type"], "message");
        assert_eq!(output[1]["type"], "function_call");
        assert_eq!(output[2]["type"], "message");
        assert_ne!(output[0]["id"], output[2]["id"]);
    }

    #[test]
    fn messages_bridge_rejects_tool_result_for_an_unknown_call() {
        let first = prepare_responses_to_messages(
            &request(Value::String("inspect the project".to_string())),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();
        let response = translate_messages_response(
            first,
            &json!({
                "id": "msg_01",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_01",
                    "name": "run_command",
                    "input": {"command": "pwd"}
                }]
            }),
        )
        .unwrap();

        let error = prepare_responses_to_messages(
            &json!({
                "model": "claude-test",
                "previous_response_id": response.response_id,
                "input": [{
                    "type": "function_call_output",
                    "call_id": "toolu_other",
                    "output": "unexpected"
                }]
            }),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            Some(response.continuation),
        )
        .unwrap_err();

        assert_eq!(error.code(), "adapter_continuation_mismatch");
    }

    #[test]
    fn messages_bridge_maps_reasoning_only_when_binding_supports_it() {
        let mut with_reasoning = request(Value::String("think".to_string()));
        with_reasoning["reasoning"] = json!({"effort": "high"});
        let adaptive = prepare_responses_to_messages(
            &with_reasoning,
            "claude-test",
            false,
            MessagesReasoningMode::Adaptive,
            None,
        )
        .unwrap();
        assert_eq!(adaptive.upstream_body()["thinking"]["type"], "adaptive");
        assert_eq!(adaptive.upstream_body()["output_config"]["effort"], "high");
        assert!(adaptive.upstream_body().get("temperature").is_none());

        let error = prepare_responses_to_messages(
            &with_reasoning,
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap_err();
        assert_eq!(error.code(), "adapter_reasoning_unsupported");
    }

    #[test]
    fn messages_bridge_rejects_hosted_tools_instead_of_lying_about_support() {
        let error = prepare_responses_to_messages(
            &json!({
                "model": "claude-test",
                "input": "hello",
                "tools": [{"type": "web_search"}]
            }),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap_err();
        assert_eq!(error.code(), "adapter_tool_unsupported");
    }

    #[test]
    fn messages_bridge_keeps_client_tools_when_hosted_tools_are_also_present() {
        let prepared = prepare_responses_to_messages(
            &json!({
                "model": "claude-test",
                "input": "inspect",
                "tools": [
                    {"type": "web_search"},
                    {
                        "type": "function",
                        "name": "run_command",
                        "parameters": {"type": "object"}
                    }
                ]
            }),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();

        assert_eq!(
            prepared.upstream_body()["tools"],
            json!([{
                "name": "run_command",
                "input_schema": {"type": "object"}
            }])
        );
    }

    #[test]
    fn messages_stream_bridge_emits_responses_tool_events_and_completion() {
        let request = prepare_responses_to_messages(
            &request(Value::String("run pwd".to_string())),
            "claude-test",
            true,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();
        let mut bridge = MessagesStreamBridge::new(request);
        bridge.push(
            br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_stream","usage":{"input_tokens":4,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_stream","name":"run_command","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"pwd\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":4,"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#,
        );
        let output = std::iter::from_fn(|| bridge.pop_output())
            .map(|frame| String::from_utf8(frame).unwrap())
            .collect::<Vec<_>>()
            .join("");
        assert!(output.contains("response.function_call_arguments.delta"));
        assert!(output.contains("response.function_call_arguments.done"));
        assert!(output.contains("response.output_item.done"));
        assert!(output.contains("response.completed"));
        assert_eq!(
            bridge.completed().unwrap().response_body["output"][0]["call_id"],
            "toolu_stream"
        );
    }

    #[test]
    fn messages_stream_bridge_emits_custom_tool_events_and_completion() {
        let request = prepare_responses_to_messages(
            &json!({
                "model": "claude-test",
                "input": "List the project files.",
                "tools": [{"type": "custom", "name": "PowerShell"}]
            }),
            "claude-test",
            true,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();
        let mut bridge = MessagesStreamBridge::new(request);
        bridge.push(
            br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_custom_stream","usage":{"input_tokens":4,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_custom_stream","name":"PowerShell","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"input\":\"Get-ChildItem\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":4,"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#,
        );
        let output = std::iter::from_fn(|| bridge.pop_output())
            .map(|frame| String::from_utf8(frame).unwrap())
            .collect::<Vec<_>>()
            .join("");
        assert!(output.contains("response.custom_tool_call_input.done"));
        assert!(output.contains("\"type\":\"custom_tool_call\""));
        assert!(!output.contains("response.function_call_arguments.delta"));
        assert_eq!(
            bridge.completed().unwrap().response_body["output"][0]["type"],
            "custom_tool_call"
        );
        assert_eq!(
            bridge.completed().unwrap().response_body["output"][0]["input"],
            "Get-ChildItem"
        );
    }

    #[test]
    fn scoped_bridge_ids_keep_same_upstream_id_isolated_between_routes() {
        let first = prepare_responses_to_messages_scoped(
            &request(Value::String("first".to_string())),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
            "source-a/responses-bridge",
        )
        .unwrap();
        let second = prepare_responses_to_messages_scoped(
            &request(Value::String("second".to_string())),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
            "source-b/responses-bridge",
        )
        .unwrap();
        let upstream = json!({
            "id": "msg_same",
            "content": [{"type": "text", "text": "ok"}]
        });
        let first = translate_messages_response(first, &upstream).unwrap();
        let second = translate_messages_response(second, &upstream).unwrap();

        assert_ne!(first.response_id, second.response_id);
        assert_eq!(
            first.response_body["output"][0]["id"],
            format!("msg_{}", first.response_id)
        );
        assert_eq!(
            second.response_body["output"][0]["id"],
            format!("msg_{}", second.response_id)
        );
    }

    #[test]
    fn messages_stream_bridge_emits_text_done_and_accepts_metadata_events() {
        let request = prepare_responses_to_messages(
            &request(Value::String("say hi".to_string())),
            "claude-test",
            true,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();
        let mut bridge = MessagesStreamBridge::new(request);
        bridge.push(
            br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_text","usage":{"input_tokens":1}}}

event: ping
data: {"type":"ping"}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"citations_delta","citation":{"type":"char_location"}}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_stop
data: {"type":"message_stop"}

"#,
        );
        let output = std::iter::from_fn(|| bridge.pop_output())
            .map(|frame| String::from_utf8(frame).unwrap())
            .collect::<Vec<_>>()
            .join("");

        assert!(bridge.completed().is_some());
        assert!(output.contains("response.output_text.delta"));
        assert!(output.contains("response.output_text.done"));
        assert!(output.contains("\"text\":\"hello\""));
    }

    #[test]
    fn messages_stream_bridge_keeps_text_tool_text_order_and_response_item_ids() {
        let request = prepare_responses_to_messages(
            &request(Value::String("inspect then summarize".to_string())),
            "claude-test",
            true,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();
        let mut bridge = MessagesStreamBridge::new(request);
        bridge.push(
            br#"data: {"type":"message_start","message":{"id":"msg_interleaved"}}

data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":"before"}}

data: {"type":"content_block_stop","index":0}

data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_interleaved","name":"run_command","input":{}}}

data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"pwd\"}"}}

data: {"type":"content_block_stop","index":1}

data: {"type":"content_block_start","index":2,"content_block":{"type":"text"}}

data: {"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"after"}}

data: {"type":"content_block_stop","index":2}

data: {"type":"message_stop"}

"#,
        );

        let frames = std::iter::from_fn(|| bridge.pop_output())
            .map(|frame| {
                let frame = String::from_utf8(frame).unwrap();
                let data = frame
                    .lines()
                    .find_map(|line| line.strip_prefix("data: "))
                    .expect("bridge frames contain JSON data");
                serde_json::from_str::<Value>(data).unwrap()
            })
            .collect::<Vec<_>>();
        let completed = bridge.completed().expect("interleaved stream completes");
        let output = completed.response_body["output"].as_array().unwrap();
        assert_eq!(
            output
                .iter()
                .map(|item| item["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["message", "function_call", "message"]
        );
        assert_eq!(output[0]["content"][0]["text"], "before");
        assert_eq!(output[1]["call_id"], "toolu_interleaved");
        assert_eq!(output[2]["content"][0]["text"], "after");
        assert_ne!(output[0]["id"], output[2]["id"]);

        let completed_items = frames
            .iter()
            .filter(|frame| frame["type"] == "response.output_item.done")
            .collect::<Vec<_>>();
        assert_eq!(completed_items.len(), 3);
        assert_eq!(completed_items[0]["output_index"], 0);
        assert_eq!(completed_items[0]["item"]["id"], output[0]["id"]);
        assert_eq!(completed_items[1]["output_index"], 1);
        assert_eq!(completed_items[1]["item"]["id"], output[1]["id"]);
        assert_eq!(completed_items[2]["output_index"], 2);
        assert_eq!(completed_items[2]["item"]["id"], output[2]["id"]);

        let continuation = prepare_responses_to_messages(
            &json!({
                "model": "claude-test",
                "previous_response_id": completed.response_id,
                "input": [{
                    "type": "function_call_output",
                    "call_id": "toolu_interleaved",
                    "output": "/workspace"
                }]
            }),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            Some(completed.continuation.clone()),
        )
        .unwrap();
        assert_eq!(
            continuation.upstream_body()["messages"][2]["content"][0]["tool_use_id"],
            "toolu_interleaved"
        );
    }

    #[test]
    fn messages_stream_bridge_rejects_unclosed_blocks_before_message_stop() {
        let request = prepare_responses_to_messages(
            &request(Value::String("say hi".to_string())),
            "claude-test",
            true,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();
        let mut bridge = MessagesStreamBridge::new(request);
        bridge.push(
            br#"data: {"type":"message_start","message":{"id":"msg_incomplete"}}

data: {"type":"content_block_start","index":0,"content_block":{"type":"text"}}

data: {"type":"message_stop"}

"#,
        );

        assert!(bridge.is_terminal());
        assert!(bridge.completed().is_none());
        let output = std::iter::from_fn(|| bridge.pop_output())
            .map(|frame| String::from_utf8(frame).unwrap())
            .collect::<Vec<_>>()
            .join("");
        assert!(output.contains("response.failed"));
    }

    #[test]
    fn messages_stream_bridge_preserves_initial_and_incremental_thinking_signature() {
        let request = prepare_responses_to_messages(
            &request(Value::String("think".to_string())),
            "claude-test",
            true,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap();
        let mut bridge = MessagesStreamBridge::new(request);
        bridge.push(
            br#"data: {"type":"message_start","message":{"id":"msg_thinking"}}

data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"initial ","signature":"sig-"}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"more"}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"part"}}

data: {"type":"content_block_stop","index":0}

data: {"type":"content_block_start","index":1,"content_block":{"type":"text"}}

data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"done"}}

data: {"type":"content_block_stop","index":1}

data: {"type":"message_stop"}

"#,
        );

        let completed = bridge.completed().expect("thinking stream should complete");
        let messages = &completed.continuation.messages;
        assert_eq!(messages[1]["content"][0]["thinking"], "initial more");
        assert_eq!(messages[1]["content"][0]["signature"], "sig-part");
    }

    #[test]
    fn native_responses_replay_materializes_tool_turn_without_protocol_conversion() {
        let initial = json!({
            "model": "alias",
            "input": "inspect the workspace",
            "tools": [{
                "type": "function",
                "name": "run_command",
                "parameters": {
                    "type": "object",
                    "properties": {"command": {"type": "string"}},
                    "required": ["command"]
                }
            }]
        });
        let upstream = json!({
            "id": "resp_tool_01",
            "output": [{
                "type": "function_call",
                "call_id": "call_01",
                "name": "run_command",
                "arguments": "{\"command\":\"pwd\"}"
            }]
        });
        let (response_id, replay) =
            NativeResponsesReplayState::from_response(&initial, "gpt-test", &upstream)
                .expect("a completed native response is replayable");

        assert_eq!(response_id, "resp_tool_01");
        let continuation = json!({
            "model": "alias",
            "previous_response_id": response_id,
            "input": [{
                "type": "function_call_output",
                "call_id": "call_01",
                "output": "C:\\workspace"
            }],
            "max_output_tokens": 128
        });
        let replayed = replay
            .replay_request(&continuation, "gpt-test", false)
            .expect("the tool result can be replayed as native Responses input");

        assert_eq!(replayed["model"], "gpt-test");
        assert_eq!(replayed["stream"], false);
        assert!(replayed.get("previous_response_id").is_none());
        assert_eq!(replayed["max_output_tokens"], 128);
        assert_eq!(replayed["tools"], initial["tools"]);
        let input = replayed["input"]
            .as_array()
            .expect("replayed input is an array");
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["text"], "inspect the workspace");
        assert_eq!(input[1], upstream["output"][0]);
        assert_eq!(input[2], continuation["input"][0]);
    }

    #[test]
    fn call_prefixed_function_item_id_repair_keeps_the_tool_result_link() {
        let mut request = json!({
            "input": [
                {"type": "message", "id": "call_message"},
                {
                    "type": "function_call",
                    "id": "call_function_01",
                    "call_id": "call_function_01",
                    "name": "run_command",
                    "arguments": "{\"command\":\"pwd\"}"
                },
                {
                    "type": "function_call",
                    "id": "fc_function_02",
                    "call_id": "call_function_02",
                    "name": "read_file",
                    "arguments": "{\"path\":\"Cargo.toml\"}"
                },
                {
                    "type": "custom_tool_call",
                    "id": "call_custom_01",
                    "call_id": "call_custom_01",
                    "name": "PowerShell",
                    "input": "Get-ChildItem"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_function_01",
                    "output": "C:\\workspace"
                }
            ]
        });

        assert!(repair_call_prefixed_function_item_ids(&mut request));
        let input = request["input"].as_array().expect("input is an array");
        assert_eq!(input[0]["id"], "call_message");
        assert_eq!(input[1]["id"], "fc_function_01");
        assert_eq!(input[1]["call_id"], "call_function_01");
        assert_eq!(input[2]["id"], "fc_function_02");
        assert_eq!(input[3]["id"], "call_custom_01");
        assert_eq!(input[4]["call_id"], "call_function_01");
        assert!(!repair_call_prefixed_function_item_ids(&mut request));
    }

    #[test]
    fn native_responses_replay_rejects_model_mismatch() {
        let initial = json!({"model": "alias", "input": "inspect"});
        let upstream = json!({"id": "resp_model_01", "output": []});
        let (_, replay) =
            NativeResponsesReplayState::from_response(&initial, "gpt-test", &upstream)
                .expect("a completed native response is replayable");
        let error = replay
            .replay_request(
                &json!({
                    "previous_response_id": "resp_model_01",
                    "input": "continue"
                }),
                "other-model",
                false,
            )
            .expect_err("a response cannot cross model routes");

        assert_eq!(error.code(), "adapter_continuation_mismatch");
    }

    #[test]
    fn native_responses_replay_store_is_route_scoped_bounded_and_expiring() {
        let initial = json!({"model": "alias", "input": "inspect"});
        let upstream = json!({"id": "resp_store_01", "output": []});
        let (_, first) = NativeResponsesReplayState::from_response(&initial, "gpt-test", &upstream)
            .expect("a completed native response is replayable");
        let (_, second) = NativeResponsesReplayState::from_response(
            &initial,
            "gpt-test",
            &json!({"id": "resp_store_02", "output": []}),
        )
        .expect("a completed native response is replayable");
        let mut store = NativeResponsesReplayStore::new(1, 10);

        store.insert("key-a", "resp_store_01", "route-a", first, 100);
        assert!(store
            .get("key-a", "resp_store_01", "route-b", 100)
            .is_none());
        assert!(store
            .get("key-b", "resp_store_01", "route-a", 100)
            .is_none());
        assert!(store
            .get("key-a", "resp_store_01", "route-a", 100)
            .is_some());

        store.insert("key-a", "resp_store_02", "route-a", second, 101);
        assert!(store
            .get("key-a", "resp_store_01", "route-a", 101)
            .is_none());
        assert!(store
            .get("key-a", "resp_store_02", "route-a", 112)
            .is_none());
    }
}
