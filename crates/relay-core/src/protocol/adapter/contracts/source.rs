use super::super::messages;
use super::{
    AdapterContinuationState, AdapterError, AdapterResult, MessagesReasoningMode,
    PreparedAdapterRequest,
};
use crate::{CacheWriteTtl, WireApi};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    GeminiInteractions,
}

impl UpstreamProtocol {
    pub const fn wire_api(self) -> WireApi {
        match self {
            Self::Responses => WireApi::Responses,
            Self::ChatCompletions => WireApi::ChatCompletions,
            Self::Messages => WireApi::Messages,
            Self::GeminiGenerateContent => WireApi::Gemini,
            Self::GeminiInteractions => WireApi::GeminiInteractions,
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
    pub previous: Option<AdapterContinuationState>,
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
                WireApi::GeminiInteractions => UpstreamProtocol::GeminiInteractions,
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
        if self.is_passthrough() {
            return true;
        }
        match self.upstream_protocol(WireApi::Responses) {
            UpstreamProtocol::Messages => mode.supports_effort(effort),
            UpstreamProtocol::GeminiGenerateContent => {
                let effort = crate::catalog::canonical_reasoning_level(effort);
                match mode {
                    _ if effort == "none" => true,
                    MessagesReasoningMode::Disabled => false,
                    MessagesReasoningMode::Adaptive => {
                        matches!(effort.as_str(), "minimal" | "low" | "medium" | "high")
                    }
                    MessagesReasoningMode::Budget => {
                        super::reasoning_effort_budget(&effort).is_some()
                    }
                }
            }
            UpstreamProtocol::Responses | UpstreamProtocol::ChatCompletions => true,
            UpstreamProtocol::GeminiInteractions => false,
        }
    }

    pub const fn uses_local_continuation_state(self) -> bool {
        !self.is_passthrough()
    }

    pub(crate) fn continuation_key(
        self,
        client: WireApi,
        request: &Value,
        model: &str,
    ) -> AdapterResult<Option<String>> {
        if self.is_passthrough() {
            return Ok(None);
        }
        if client == WireApi::Responses
            && let Some(key) = request
                .get("previous_response_id")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
        {
            return Ok(Some(key.to_owned()));
        }
        super::super::translation::continuation_key(client, request, model)
    }

    pub const fn route_suffix(self, client_wire_api: WireApi) -> &'static str {
        match (client_wire_api, self) {
            (WireApi::GeminiInteractions, Self::Native) => "gemini_interactions",
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
        self.prepare_with_capabilities(context, None)
    }

    pub(crate) fn prepare_request_with_capabilities(
        self,
        context: AdapterRequestContext<'_>,
        capabilities: &crate::model_metadata::ModelCapabilities,
    ) -> AdapterResult<PreparedAdapterRequest> {
        self.prepare_with_capabilities(context, Some(capabilities))
    }

    pub(crate) fn supports_reasoning_effort_for_model(
        self,
        mode: MessagesReasoningMode,
        effort: &str,
        capabilities: &crate::model_metadata::ModelCapabilities,
    ) -> bool {
        // Passthrough preserves native wire fields, not a capability claim.
        // Exact route evidence must also constrain the client model picker.
        if capabilities.reasoning_effort_conflict {
            return false;
        }
        if capabilities.reasoning == Some(false) {
            return crate::catalog::canonical_reasoning_level(effort) == "none";
        }
        self.is_passthrough()
            || super::ReasoningPolicy::new(
                self.upstream_protocol(WireApi::Responses).wire_api(),
                mode,
                Some(capabilities),
            )
            .apply_effort(&mut serde_json::Map::new(), effort, false)
            .is_ok()
    }

    /// Validate controls without cloning/re-encoding a native request. The same
    /// check runs during admission and preparation; it never clips user values.
    pub(crate) fn validate_model_controls(
        self,
        client: WireApi,
        request: &Value,
        capabilities: &crate::model_metadata::ModelCapabilities,
    ) -> AdapterResult<()> {
        let (effort, outputs): (&str, &[(&str, &str)]) = match client {
            WireApi::Responses => (
                "/reasoning/effort",
                &[("/max_output_tokens", "max_output_tokens")],
            ),
            WireApi::ChatCompletions => (
                "/reasoning_effort",
                &[
                    ("/max_completion_tokens", "max_completion_tokens"),
                    ("/max_tokens", "max_tokens"),
                ],
            ),
            WireApi::Messages => ("/output_config/effort", &[("/max_tokens", "max_tokens")]),
            WireApi::Gemini => (
                "/generationConfig/thinkingConfig/thinkingLevel",
                &[(
                    "/generationConfig/maxOutputTokens",
                    "generationConfig.maxOutputTokens",
                )],
            ),
            // Interactions has its own contract; do not guess an output field
            // from generateContent or a similarly named API.
            WireApi::GeminiInteractions => ("/generation_config/thinking_level", &[]),
        };
        if capabilities.reasoning_effort_conflict
            && request
                .pointer(effort)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(AdapterError::reasoning_metadata_conflict());
        }
        if let Some(maximum) = capabilities.output_limit.filter(|maximum| *maximum > 0) {
            for &(path, parameter) in outputs {
                if request
                    .pointer(path)
                    .and_then(Value::as_u64)
                    .is_some_and(|tokens| tokens > maximum)
                {
                    return Err(AdapterError::output_limit_exceeded(parameter));
                }
            }
        }
        Ok(())
    }

    fn prepare_with_capabilities(
        self,
        context: AdapterRequestContext<'_>,
        capabilities: Option<&crate::model_metadata::ModelCapabilities>,
    ) -> AdapterResult<PreparedAdapterRequest> {
        self.validate(context.client_wire_api, context.reasoning_mode)?;
        if let Some(capabilities) = capabilities {
            self.validate_model_controls(context.client_wire_api, context.request, capabilities)?;
        }
        let rewritten = if !self.is_passthrough() && context.client_wire_api == WireApi::Responses {
            match super::super::compaction::prepare_bridged_compaction(context.request)? {
                super::super::compaction::BridgedCompaction::Rewritten {
                    rewritten_request,
                    ..
                } => Some(rewritten_request),
                super::super::compaction::BridgedCompaction::Unchanged => None,
            }
        } else {
            None
        };
        let context = if let Some(request) = rewritten.as_ref() {
            AdapterRequestContext { request, ..context }
        } else {
            context
        };
        let history_request =
            if !self.is_passthrough() && context.client_wire_api == WireApi::Responses {
                Some(super::history::prepare_request(
                    context.request,
                    context.previous.as_ref(),
                    context.model,
                )?)
            } else {
                None
            };
        let context = if let Some(request) = history_request.as_deref() {
            AdapterRequestContext { request, ..context }
        } else {
            context
        };
        if !self.is_passthrough() {
            let upstream = self.upstream_protocol(context.client_wire_api).wire_api();
            return super::super::translation::TranslationRequest::prepare(
                context,
                upstream,
                capabilities,
            )
            .map(|request| PreparedAdapterRequest::Translated {
                request: Box::new(request),
            })
            .and_then(|prepared| {
                if let Some(capabilities) = capabilities {
                    self.validate_model_controls(upstream, prepared.upstream_body(), capabilities)?;
                }
                Ok(prepared)
            });
        }
        match self {
            Self::Native => {
                let mut upstream_body = context.request.clone();
                let upstream_object = upstream_body
                    .as_object_mut()
                    .ok_or_else(AdapterError::invalid_request)?;
                if context.client_wire_api == WireApi::Gemini {
                    // Gemini places the model in the endpoint path. A model
                    // field is not part of the generateContent contract and
                    // some providers reject it as an unknown field.
                    upstream_object.shift_remove("model");
                } else {
                    upstream_object.insert(
                        "model".to_string(),
                        Value::String(context.model.to_string()),
                    );
                }
                if context.client_wire_api == WireApi::Messages {
                    messages::apply_cache_write_ttl(&mut upstream_body, context.cache_write_ttl)?;
                }
                Ok(PreparedAdapterRequest::Native { upstream_body })
            }
            _ => unreachable!("all conversions use the shared protocol codecs"),
        }
    }
}
