use super::super::{gemini, messages};
use super::{
    AdapterError, AdapterResult, MessagesBridgeState, MessagesReasoningMode, PreparedAdapterRequest,
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
        let rewritten = if !self.is_passthrough() && context.client_wire_api == WireApi::Responses {
            match super::super::compaction::prepare_bridged_compaction(context.request)? {
                super::super::compaction::BridgedCompaction::Rewritten { request, .. } => {
                    Some(request)
                }
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
        if !matches!(
            self,
            Self::Native | Self::ResponsesToMessages | Self::ResponsesToGemini
        ) {
            let upstream = self.upstream_protocol(context.client_wire_api).wire_api();
            return super::super::translation::TranslationRequest::prepare(context, upstream).map(
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
