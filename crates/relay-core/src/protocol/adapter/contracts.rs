use super::{
    gemini::{self, GeminiBridgeRequest, GeminiBridgeResponse},
    messages,
    stream::{AdapterStreamBridge, MessagesStreamBridge},
};
use crate::error_codes;
use crate::{CacheWriteTtl, WireApi};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub(super) fn bridged_namespace_tool_name(namespace: &str, name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((namespace.len() as u64).to_le_bytes());
    hasher.update(namespace.as_bytes());
    hasher.update((name.len() as u64).to_le_bytes());
    hasher.update(name.as_bytes());
    let digest = hasher.finalize();
    format!("relay_ns_{}", hex::encode(&digest[..12]))
}

pub(super) fn prepare_bridge_state<'a>(
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
    ResponsesToChatCompletions,
    ChatCompletionsToResponses,
    ChatCompletionsToMessages,
    ChatCompletionsToGemini,
    MessagesToResponses,
    MessagesToChatCompletions,
    MessagesToGemini,
    GeminiToResponses,
    GeminiToChatCompletions,
    GeminiToMessages,
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

impl UpstreamProtocol {
    pub const fn wire_api(self) -> WireApi {
        match self {
            Self::Responses => WireApi::Responses,
            Self::ChatCompletions => WireApi::ChatCompletions,
            Self::Messages => WireApi::Messages,
            Self::GeminiGenerateContent => WireApi::Gemini,
        }
    }
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
    /// Only executable transformations may be returned by the route resolver.
    pub fn between(client: WireApi, upstream: WireApi) -> Option<Self> {
        if client == upstream {
            return Some(Self::Native);
        }
        match (client, upstream) {
            (WireApi::Responses, WireApi::Messages) => Some(Self::ResponsesToMessages),
            (WireApi::Responses, WireApi::Gemini) => Some(Self::ResponsesToGemini),
            (WireApi::Responses, WireApi::ChatCompletions) => {
                Some(Self::ResponsesToChatCompletions)
            }
            (WireApi::ChatCompletions, WireApi::Responses) => {
                Some(Self::ChatCompletionsToResponses)
            }
            (WireApi::ChatCompletions, WireApi::Messages) => Some(Self::ChatCompletionsToMessages),
            (WireApi::ChatCompletions, WireApi::Gemini) => Some(Self::ChatCompletionsToGemini),
            (WireApi::Messages, WireApi::Responses) => Some(Self::MessagesToResponses),
            (WireApi::Messages, WireApi::ChatCompletions) => Some(Self::MessagesToChatCompletions),
            (WireApi::Messages, WireApi::Gemini) => Some(Self::MessagesToGemini),
            (WireApi::Gemini, WireApi::Responses) => Some(Self::GeminiToResponses),
            (WireApi::Gemini, WireApi::ChatCompletions) => Some(Self::GeminiToChatCompletions),
            (WireApi::Gemini, WireApi::Messages) => Some(Self::GeminiToMessages),
            _ => None,
        }
    }

    pub const fn upstream_protocol(self, client_wire_api: WireApi) -> UpstreamProtocol {
        match self {
            Self::Native => match client_wire_api {
                WireApi::Responses => UpstreamProtocol::Responses,
                WireApi::ChatCompletions => UpstreamProtocol::ChatCompletions,
                WireApi::Messages => UpstreamProtocol::Messages,
                WireApi::Gemini => UpstreamProtocol::GeminiGenerateContent,
            },
            Self::ResponsesToMessages
            | Self::ChatCompletionsToMessages
            | Self::GeminiToMessages => UpstreamProtocol::Messages,
            Self::ResponsesToGemini | Self::ChatCompletionsToGemini | Self::MessagesToGemini => {
                UpstreamProtocol::GeminiGenerateContent
            }
            Self::ChatCompletionsToResponses
            | Self::MessagesToResponses
            | Self::GeminiToResponses => UpstreamProtocol::Responses,
            Self::ResponsesToChatCompletions
            | Self::MessagesToChatCompletions
            | Self::GeminiToChatCompletions => UpstreamProtocol::ChatCompletions,
        }
    }

    pub const fn is_passthrough(self) -> bool {
        matches!(self, Self::Native)
    }

    pub fn supports_reasoning_effort(self, mode: MessagesReasoningMode, effort: &str) -> bool {
        let effort = effort.trim().to_ascii_lowercase();
        if self.is_passthrough() {
            return true;
        }
        if mode == MessagesReasoningMode::Disabled {
            return false;
        }
        match self.upstream_protocol(WireApi::Responses) {
            UpstreamProtocol::Messages if mode == MessagesReasoningMode::Adaptive => {
                matches!(effort.as_str(), "none" | "low" | "medium" | "high" | "max")
            }
            UpstreamProtocol::GeminiGenerateContent if mode == MessagesReasoningMode::Adaptive => {
                matches!(effort.as_str(), "minimal" | "low" | "medium" | "high")
            }
            UpstreamProtocol::Responses | UpstreamProtocol::ChatCompletions => true,
            _ => mode.supports_effort(&effort),
        }
    }

    pub const fn uses_local_continuation_state(self) -> bool {
        matches!(
            self,
            Self::ResponsesToMessages | Self::ResponsesToGemini | Self::ResponsesToChatCompletions
        )
    }

    pub const fn route_suffix(self, client_wire_api: WireApi) -> &'static str {
        match (client_wire_api, self) {
            (WireApi::Responses, Self::ResponsesToMessages) => "responses_to_messages",
            (WireApi::Responses, Self::ResponsesToGemini) => "responses_to_gemini",
            (WireApi::Responses, Self::Native) => "responses",
            (WireApi::ChatCompletions, Self::Native) => "chat_completions",
            (WireApi::Messages, Self::Native) => "messages",
            (WireApi::Gemini, Self::Native) => "gemini",
            (_, Self::ResponsesToChatCompletions) => "responses_to_chat_completions",
            (_, Self::ChatCompletionsToResponses) => "chat_completions_to_responses",
            (_, Self::ChatCompletionsToMessages) => "chat_completions_to_messages",
            (_, Self::ChatCompletionsToGemini) => "chat_completions_to_gemini",
            (_, Self::MessagesToResponses) => "messages_to_responses",
            (_, Self::MessagesToChatCompletions) => "messages_to_chat_completions",
            (_, Self::MessagesToGemini) => "messages_to_gemini",
            (_, Self::GeminiToResponses) => "gemini_to_responses",
            (_, Self::GeminiToChatCompletions) => "gemini_to_chat_completions",
            (_, Self::GeminiToMessages) => "gemini_to_messages",
            // Validation rejects this combination today. Keeping a stable
            // fallback makes candidate identity forward-compatible with a
            // future adapter that targets another upstream contract.
            (_, Self::ResponsesToMessages | Self::ResponsesToGemini) => "bridge",
        }
    }

    pub fn validate(
        self,
        client_wire_api: WireApi,
        _reasoning_mode: MessagesReasoningMode,
    ) -> AdapterResult<()> {
        match self {
            // Native traffic is already expressed in the selected client
            // protocol. It must preserve the request as-is; the shared
            // reasoning mode only governs translated routes.
            Self::Native => Ok(()),
            adapter
                if Self::between(
                    client_wire_api,
                    adapter.upstream_protocol(client_wire_api).wire_api(),
                ) == Some(adapter) =>
            {
                Ok(())
            }
            _ => Err(AdapterError::unsupported_binding()),
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
        self.validate(context.client_wire_api, context.reasoning_mode)?;
        if !matches!(
            self,
            Self::Native | Self::ResponsesToMessages | Self::ResponsesToGemini
        ) {
            let upstream = self.upstream_protocol(context.client_wire_api).wire_api();
            return super::translation::TranslationRequest::prepare(context, upstream).map(
                |request| PreparedAdapterRequest::Translated {
                    request: Box::new(request),
                },
            );
        }
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
            _ => unreachable!("registered translation handled before legacy adapters"),
        }
    }
}

/// Responses context management cannot be translated into another protocol.
/// Inspect only the control field and input item types, never text or nested
/// tool payloads.
pub(super) fn validate_bridge_compaction(request: &Value) -> AdapterResult<()> {
    let is_compaction = |item: &Value| {
        matches!(
            item.get("type").and_then(Value::as_str),
            Some("compaction" | "compaction_summary" | "compaction_trigger")
        )
    };
    let configured = request
        .get("context_management")
        .is_some_and(|value| match value {
            Value::Null => false,
            Value::Array(items) => !items.is_empty(),
            Value::Object(fields) => !fields.is_empty(),
            _ => true,
        });
    let history = request.get("input").is_some_and(|input| match input {
        Value::Array(items) => items.iter().any(is_compaction),
        Value::Object(_) => is_compaction(input),
        _ => false,
    });
    if configured || history {
        return Err(AdapterError {
            code: error_codes::ADAPTER_COMPACTION_UNSUPPORTED,
            message: "Responses compaction history requires a native Responses route",
            parameter: Some(if configured {
                "context_management"
            } else {
                "input"
            }),
        });
    }
    Ok(())
}

pub(super) fn validate_responses_bridge_request(
    request: &Value,
    upstream: WireApi,
) -> AdapterResult<()> {
    validate_bridge_compaction(request)?;
    let object = request
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    let mut allowed = vec![
        "model",
        "stream",
        "input",
        "instructions",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "max_output_tokens",
        "temperature",
        "top_p",
        "stop",
        "text",
        "reasoning",
        "previous_response_id",
        "store",
        "background",
        "include",
        "context_management",
        "prompt_cache_key",
        "client_metadata",
    ];
    if upstream == WireApi::Gemini {
        allowed.extend([
            "top_k",
            "presence_penalty",
            "frequency_penalty",
            "seed",
            "response_format",
        ]);
    }
    if let Some((name, _)) = object
        .iter()
        .find(|(name, value)| !allowed.contains(&name.as_str()) && !value.is_null())
    {
        // Only report contract field names, never arbitrary keys from a payload.
        let parameter = [
            "stream_options",
            "metadata",
            "access_programs",
            "prompt_cache_retention",
            "prompt_cache_options",
        ]
        .into_iter()
        .find(|known| *known == name)
        .unwrap_or("request");
        return Err(AdapterError::parameter_unsupported_for(parameter));
    }
    validate_responses_transport_controls(request)?;
    for name in ["stream", "parallel_tool_calls"] {
        if object
            .get(name)
            .is_some_and(|value| !value.is_null() && !value.is_boolean())
        {
            return Err(AdapterError::invalid_request());
        }
    }
    for name in [
        "temperature",
        "top_p",
        "top_k",
        "presence_penalty",
        "frequency_penalty",
        "seed",
    ] {
        if object
            .get(name)
            .is_some_and(|value| !value.is_null() && !value.is_number())
        {
            return Err(AdapterError::invalid_request());
        }
    }
    if object
        .get("max_output_tokens")
        .is_some_and(|value| !value.is_null() && value.as_u64().is_none_or(|tokens| tokens == 0))
    {
        return Err(AdapterError::invalid_request());
    }
    for name in ["instructions", "previous_response_id", "prompt_cache_key"] {
        if object
            .get(name)
            .is_some_and(|value| !value.is_null() && !value.is_string())
        {
            return Err(AdapterError::invalid_request());
        }
    }
    for name in ["text", "reasoning"] {
        if object
            .get(name)
            .is_some_and(|value| !value.is_null() && !value.is_object())
        {
            return Err(AdapterError::invalid_request());
        }
    }
    if let Some(reasoning) = object.get("reasoning").and_then(Value::as_object) {
        if reasoning
            .get("effort")
            .is_some_and(|value| !value.is_null() && !value.is_string())
        {
            return Err(AdapterError::invalid_request());
        }
    }
    if let Some(format) = request
        .pointer("/text/format")
        .filter(|value| !value.is_null())
    {
        validate_bridge_fields(format, &["type", "name", "schema", "strict", "description"])?;
    }
    for name in ["background", "store"] {
        if object
            .get(name)
            .is_some_and(|value| !value.is_null() && value != false)
        {
            return Err(AdapterError::parameter_unsupported_for(name));
        }
    }
    if object
        .get("text")
        .and_then(Value::as_object)
        .is_some_and(|text| {
            text.iter()
                .any(|(name, value)| name != "format" && !value.is_null())
        })
    {
        let field = if request
            .pointer("/text/verbosity")
            .is_some_and(|v| !v.is_null())
        {
            "text.verbosity"
        } else {
            "text"
        };
        return Err(AdapterError::parameter_unsupported_for(field));
    }
    if object
        .get("reasoning")
        .and_then(Value::as_object)
        .is_some_and(|reasoning| {
            reasoning.iter().any(|(name, value)| {
                !matches!(name.as_str(), "effort" | "summary") && !value.is_null()
            })
        })
    {
        return Err(AdapterError::parameter_unsupported_for("reasoning"));
    }
    if upstream == WireApi::Gemini && object.get("parallel_tool_calls") == Some(&Value::Bool(false))
    {
        return Err(AdapterError::parameter_unsupported_for(
            "parallel_tool_calls",
        ));
    }
    if let Some(tools) = request_tool_catalog(object)? {
        for tool in &tools {
            validate_bridge_tool(tool)?;
        }
    }
    Ok(())
}

fn validate_responses_transport_controls(request: &Value) -> AdapterResult<()> {
    // The cache key is consumed by Relay affinity. Client metadata is tracing
    // information for the receiving server, not model input or provider metadata.
    if let Some(metadata) = request.get("client_metadata").filter(|v| !v.is_null()) {
        if metadata
            .as_object()
            .is_none_or(|fields| fields.values().any(|v| !v.is_string()))
        {
            return Err(AdapterError::invalid_request().with_parameter("client_metadata"));
        }
    }
    // `include` asks for optional output fields; it does not supply encrypted
    // history. Bridges keep their native continuation state locally. Never
    // fabricate an OpenAI encrypted blob or discard one received in input.
    if let Some(include) = request.get("include").filter(|v| !v.is_null()) {
        let items = include
            .as_array()
            .ok_or_else(|| AdapterError::invalid_request().with_parameter("include"))?;
        if items
            .iter()
            .any(|v| v.as_str() != Some("reasoning.encrypted_content"))
        {
            return Err(AdapterError::parameter_unsupported_for("include"));
        }
    }
    if let Some(summary) = request
        .pointer("/reasoning/summary")
        .filter(|v| !v.is_null())
    {
        if summary != "auto" {
            return Err(AdapterError::parameter_unsupported_for("reasoning.summary"));
        }
    }
    if request
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("encrypted_content").is_some_and(|v| !v.is_null()))
        })
    {
        return Err(AdapterError::parameter_unsupported_for(
            "input.encrypted_content",
        ));
    }
    Ok(())
}

fn validate_bridge_tool(tool: &Value) -> AdapterResult<()> {
    let object = tool
        .as_object()
        .ok_or_else(AdapterError::unsupported_tool)?;
    match object.get("type").and_then(Value::as_str) {
        Some("function" | "custom") | None
            if object.get("name").and_then(Value::as_str).is_some() =>
        {
            validate_bridge_fields(
                tool,
                &[
                    "type",
                    "name",
                    "description",
                    "parameters",
                    "format",
                    "strict",
                    "defer_loading",
                    "allowed_callers",
                ],
            )?;
            for name in ["strict", "defer_loading"] {
                if object
                    .get(name)
                    .is_some_and(|value| !value.is_null() && !value.is_boolean())
                {
                    return Err(AdapterError::invalid_request());
                }
            }
            if object.get("defer_loading").and_then(Value::as_bool) == Some(true)
                || object
                    .get("allowed_callers")
                    .is_some_and(|value| !value.is_null())
            {
                return Err(AdapterError::unsupported_tool());
            }
        }
        Some("namespace") => {
            validate_bridge_fields(tool, &["type", "name", "description", "tools"])?;
            if object
                .get("name")
                .and_then(Value::as_str)
                .is_none_or(|name| name.is_empty())
            {
                return Err(AdapterError::unsupported_tool());
            }
            for child in object
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(AdapterError::unsupported_tool)?
            {
                validate_bridge_tool(child)?;
            }
        }
        _ => return Err(AdapterError::unsupported_tool()),
    }
    Ok(())
}

fn validate_bridge_fields(value: &Value, allowed: &[&str]) -> AdapterResult<()> {
    let fields = value
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    if fields
        .iter()
        .any(|(name, value)| !allowed.contains(&name.as_str()) && !value.is_null())
    {
        return Err(AdapterError::parameter_unsupported());
    }
    Ok(())
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
    parameter: Option<&'static str>,
}

impl AdapterError {
    pub const fn code(self) -> &'static str {
        self.code
    }

    pub const fn message(self) -> &'static str {
        self.message
    }

    pub const fn parameter(self) -> Option<&'static str> {
        self.parameter
    }

    pub(crate) const fn with_parameter(mut self, parameter: &'static str) -> Self {
        self.parameter = Some(parameter);
        self
    }

    pub(crate) const fn parameter_unsupported_for(parameter: &'static str) -> Self {
        Self::parameter_unsupported().with_parameter(parameter)
    }

    pub fn is_upstream_failure(self) -> bool {
        matches!(
            self.code,
            error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID
                | error_codes::ADAPTER_UPSTREAM_STREAM_INVALID
        )
    }

    pub(crate) fn is_route_incompatible(self) -> bool {
        matches!(
            self.code,
            error_codes::ADAPTER_TOOL_UNSUPPORTED
                | error_codes::ADAPTER_PARAMETER_UNSUPPORTED
                | error_codes::ADAPTER_BINDING_UNSUPPORTED
                | error_codes::ADAPTER_REASONING_UNSUPPORTED
                | error_codes::ADAPTER_COMPACTION_UNSUPPORTED
        )
    }

    pub(super) const fn invalid_request() -> Self {
        Self {
            code: error_codes::ADAPTER_INVALID_REQUEST,
            message: "request cannot be represented by the selected source adapter",
            parameter: None,
        }
    }

    pub(crate) const fn parameter_unsupported() -> Self {
        Self {
            code: error_codes::ADAPTER_PARAMETER_UNSUPPORTED,
            message: "a request parameter has no lossless mapping on this route; use a compatible native endpoint",
            parameter: None,
        }
    }

    pub(super) const fn continuation_missing() -> Self {
        Self {
            code: error_codes::ADAPTER_CONTINUATION_MISSING,
            message: "the adapter no longer has the prior response needed for this continuation",
            parameter: None,
        }
    }

    pub(super) const fn continuation_mismatch() -> Self {
        Self {
            code: error_codes::ADAPTER_CONTINUATION_MISMATCH,
            message: "the continuation belongs to a different model or source route",
            parameter: None,
        }
    }

    pub(crate) const fn unsupported_binding() -> Self {
        Self {
            code: error_codes::ADAPTER_BINDING_UNSUPPORTED,
            message: "the selected adapter cannot serve this client protocol",
            parameter: None,
        }
    }

    pub(super) const fn unsupported_tool() -> Self {
        Self {
            code: error_codes::ADAPTER_TOOL_UNSUPPORTED,
            message: "the selected source adapter supports JSON-schema function and direct custom text tools only",
            parameter: None,
        }
    }

    pub(crate) const fn reasoning_unsupported() -> Self {
        Self {
            code: error_codes::ADAPTER_REASONING_UNSUPPORTED,
            message: "the selected source adapter does not expose reasoning for this binding",
            parameter: None,
        }
    }

    pub(crate) const fn upstream_response_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID,
            message: "the upstream response cannot be represented as a Responses response",
            parameter: None,
        }
    }

    pub(super) const fn upstream_stream_invalid() -> Self {
        Self {
            code: error_codes::ADAPTER_UPSTREAM_STREAM_INVALID,
            message: "the upstream stream cannot be represented as a Responses stream",
            parameter: None,
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

/// Collects the complete client-side tool catalog for one Responses request.
/// Codex can place tools loaded during a turn in `input.additional_tools`;
/// bridges need to combine those with the root catalog before they translate
/// their distinct upstream contracts.
pub(super) fn request_tool_catalog(
    object: &Map<String, Value>,
) -> AdapterResult<Option<Vec<Value>>> {
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
    pub(super) portable_history: Option<Vec<super::translation::Message>>,
    pub(super) model: String,
    pub(super) system: Option<Value>,
    pub(super) historical_system: Option<Value>,
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

/// Local history for native Responses recovery before client-visible output.
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
    pub(crate) fn model(&self) -> &str {
        &self.model
    }

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
        if native_replay_has_provider_state(request) {
            return None;
        }
        let request = request.as_object()?.clone();
        if !request.contains_key("input") {
            return None;
        }
        let input = request.get("input")?;
        let items = input
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_else(|| std::slice::from_ref(input));
        if !native_tool_outputs_are_materialized(items) {
            return None;
        }
        // The stored request must already be self-contained. If a predecessor
        // was unavailable, retaining this opaque id would create a replay that
        // appears valid while silently losing the earlier conversation.
        if request
            .get("previous_response_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.trim().is_empty())
        {
            return None;
        }
        // A native replay is safe only when the completed response contains
        // its output items. Never store a request-only snapshot: replaying it
        // after a quota handoff would silently drop tool/context output.
        let output = response.get("output").and_then(Value::as_array)?.clone();
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
    /// in `input`, so an unavailable owner or rejected response reference can
    /// recover before the stream is committed.
    pub fn replay_request(
        &self,
        continuation: &Value,
        model: &str,
        stream: bool,
    ) -> AdapterResult<Value> {
        if !self.model.eq_ignore_ascii_case(model) || native_replay_has_provider_state(continuation)
        {
            return Err(AdapterError::continuation_mismatch());
        }
        let continuation = continuation
            .as_object()
            .ok_or_else(AdapterError::invalid_request)?;
        let initial_input = self
            .request
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?;
        let current_input = continuation
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?;
        let mut input = Vec::new();
        append_replay_input(&mut input, initial_input)?;
        input.extend(self.output.iter().cloned());
        append_replay_input(&mut input, current_input)?;
        if input.is_empty() {
            return Err(AdapterError::invalid_request());
        }
        if !native_tool_outputs_are_materialized(&input) {
            return Err(AdapterError::continuation_mismatch());
        }

        // Only history crosses turns. Reusing the old request template would
        // revive old instructions, output settings or transport control fields.
        let mut request = continuation.clone();
        request.remove("previous_response_id");
        request.insert("model".to_string(), Value::String(model.to_string()));
        request.insert("stream".to_string(), Value::Bool(stream));
        request.insert("input".to_string(), Value::Array(input));
        Ok(Value::Object(request))
    }
}

fn native_tool_outputs_are_materialized(items: &[Value]) -> bool {
    let mut calls = BTreeSet::new();
    for item in items {
        let Some(kind) = item.get("type").and_then(Value::as_str) else {
            continue;
        };
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty());
        let output_call_kind = if kind == "tool_search_output" {
            Some("tool_search_call")
        } else if kind.ends_with("_call_output") {
            kind.strip_suffix("_output")
        } else {
            None
        };
        if let Some(call_kind) = output_call_kind {
            if !call_id.is_some_and(|id| calls.remove(&(call_kind, id))) {
                return false;
            }
        } else if kind.ends_with("_call") {
            if let Some(id) = call_id {
                calls.insert((kind, id));
            }
        }
    }
    true
}

fn native_replay_has_provider_state(request: &Value) -> bool {
    if request
        .get("conversation")
        .is_some_and(|value| !value.is_null())
    {
        return true;
    }
    let is_reference = |item: &Value| {
        item.get("type").and_then(Value::as_str) == Some("item_reference")
            || (item.get("id").is_some()
                && item.get("type").is_none()
                && item.get("role").is_none())
    };
    match request.get("input") {
        Some(Value::Array(items)) => items.iter().any(is_reference),
        Some(item @ Value::Object(_)) => is_reference(item),
        _ => false,
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
    Translated(MessagesBridgeResponse),
}

impl AdapterResponse {
    pub fn response_body(&self) -> &Value {
        match self {
            Self::Messages(response) | Self::Translated(response) => &response.response_body,
            Self::Gemini(response) => &response.response_body,
        }
    }

    pub fn response_id(&self) -> &str {
        match self {
            Self::Messages(response) | Self::Translated(response) => &response.response_id,
            Self::Gemini(response) => &response.response_id,
        }
    }

    pub fn messages_continuation(&self) -> Option<&MessagesBridgeResponse> {
        match self {
            Self::Messages(response) | Self::Translated(response) => Some(response),
            Self::Gemini(_) => None,
        }
    }

    /// Returns the local continuation payload for either bridge. The method
    /// keeps the legacy name above source-compatible while making persistence
    /// protocol-agnostic.
    pub fn continuation(&self) -> Option<(&str, &MessagesBridgeState)> {
        match self {
            Self::Messages(response) | Self::Translated(response) => {
                Some((&response.response_id, &response.continuation))
            }
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
    Native {
        upstream_body: Value,
    },
    ResponsesToMessages {
        request: Box<MessagesBridgeRequest>,
    },
    ResponsesToGemini {
        request: Box<GeminiBridgeRequest>,
    },
    Translated {
        request: Box<super::translation::TranslationRequest>,
    },
}

impl PreparedAdapterRequest {
    pub fn upstream_body(&self) -> &Value {
        match self {
            Self::Native { upstream_body } => upstream_body,
            Self::ResponsesToMessages { request } => request.upstream_body(),
            Self::ResponsesToGemini { request } => request.upstream_body(),
            Self::Translated { request } => request.upstream_body(),
        }
    }

    /// Only native routes permit local request normalization after adapter
    /// preparation. Bridge bodies are already a complete upstream contract.
    pub fn native_upstream_body_mut(&mut self) -> Option<&mut Value> {
        match self {
            Self::Native { upstream_body } => Some(upstream_body),
            Self::ResponsesToMessages { .. }
            | Self::ResponsesToGemini { .. }
            | Self::Translated { .. } => None,
        }
    }

    pub fn upstream_body_mut(&mut self) -> &mut Value {
        match self {
            Self::Native { upstream_body } => upstream_body,
            Self::ResponsesToMessages { request } => &mut request.upstream_body,
            Self::ResponsesToGemini { request } => &mut request.upstream_body,
            Self::Translated { request } => &mut request.upstream_body,
        }
    }

    pub const fn is_passthrough(&self) -> bool {
        matches!(self, Self::Native { .. })
    }

    pub const fn requires_bridge_headers(&self) -> bool {
        matches!(
            self,
            Self::ResponsesToMessages { .. }
                | Self::ResponsesToGemini { .. }
                | Self::Translated { .. }
        )
    }

    pub const fn uses_messages_continuation(&self) -> bool {
        matches!(
            self,
            Self::ResponsesToMessages { .. }
                | Self::ResponsesToGemini { .. }
                | Self::Translated { .. }
        )
    }

    /// Translates a completed upstream response only when the selected route
    /// is a bridge. Native bytes remain untouched and can be proxied directly.
    pub fn translate_response_bytes(self, bytes: &[u8]) -> AdapterResult<Option<AdapterResponse>> {
        match self {
            Self::Native { .. } => Ok(None),
            Self::Translated { request } => {
                let upstream = serde_json::from_slice::<Value>(bytes)
                    .map_err(|_| AdapterError::upstream_response_invalid())?;
                request
                    .translate(&upstream)
                    .map(AdapterResponse::Translated)
                    .map(Some)
            }
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
            Self::Translated { request } => Some(AdapterStreamBridge::Translated(Box::new(
                super::translation::TranslationStream::new(*request),
            ))),
            Self::ResponsesToMessages { request } => Some(AdapterStreamBridge::Messages(Box::new(
                MessagesStreamBridge::new(*request),
            ))),
            Self::ResponsesToGemini { request } => Some(AdapterStreamBridge::Gemini(Box::new(
                super::stream::GeminiStreamBridge::new(*request),
            ))),
        }
    }
}
