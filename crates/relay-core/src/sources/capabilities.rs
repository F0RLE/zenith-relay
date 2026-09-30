use super::{SourceProtocolBinding, WireApi};
use crate::{CacheWriteTtl, MessagesReasoningMode, Result, SourceAdapter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Declared,
    Confirmed,
    Unsupported,
    #[default]
    Unknown,
}

impl CapabilityStatus {
    pub const fn available(self) -> bool {
        matches!(self, Self::Declared | Self::Confirmed)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityOrigin {
    Catalog,
    ServiceProfile,
    EndpointUrl,
    Manual,
    GenerationProbe,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolFeature {
    Text,
    Streaming,
    Images,
    FunctionTools,
    ToolChoice,
    StructuredOutput,
    Reasoning,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEndpointCapability {
    pub model_id: String,
    pub upstream_wire_api: WireApi,
    pub status: CapabilityStatus,
    pub origin: CapabilityOrigin,
    pub checked_at_ms: u64,
    #[serde(default)]
    pub features: BTreeMap<ProtocolFeature, CapabilityStatus>,
    #[serde(default)]
    pub reasoning_efforts: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProtocolConfig {
    /// Incremented whenever the address or credential changes. Probe results
    /// are applied only to the revision they actually tested.
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub capabilities: Vec<ModelEndpointCapability>,
    #[serde(default)]
    pub endpoint_hint: Option<WireApi>,
}

mod catalog;
mod config;
pub(crate) use catalog::catalog_capabilities;
pub use config::SourceProtocolResolution;
pub fn endpoint_url_protocol(base_url: &str) -> Option<WireApi> {
    let url = url::Url::parse(base_url).ok()?;
    let path = url.path().trim_end_matches('/');
    if path.ends_with("/responses") {
        Some(WireApi::Responses)
    } else if path.ends_with("/chat/completions") {
        Some(WireApi::ChatCompletions)
    } else if path.ends_with("/messages") {
        Some(WireApi::Messages)
    } else if path.ends_with(":generateContent") || path.ends_with(":streamGenerateContent") {
        Some(WireApi::Gemini)
    } else {
        None
    }
}

/// Published service defaults are declarations, never successful probes.
/// Match exact hosts to avoid interpreting lookalike domains as trusted profiles.
pub fn service_protocol(base_url: &str) -> Option<WireApi> {
    let url = url::Url::parse(base_url).ok()?;
    match url.host_str()? {
        "api.openai.com" | "api.zenithmarket.dev" => Some(WireApi::Responses),
        "openrouter.ai" | "api.deepseek.com" | "api.groq.com" | "api.mistral.ai" => {
            Some(WireApi::ChatCompletions)
        }
        "api.anthropic.com" => Some(WireApi::Messages),
        "generativelanguage.googleapis.com" => Some(WireApi::Gemini),
        _ => None,
    }
}

pub(super) fn endpoint_type(value: &str) -> Option<WireApi> {
    match value {
        "openai-response" | "responses" | "/v1/responses" => Some(WireApi::Responses),
        "openai" | "chat_completions" | "chat.completions" | "/v1/chat/completions" => {
            Some(WireApi::ChatCompletions)
        }
        "anthropic" | "messages" | "/v1/messages" => Some(WireApi::Messages),
        "gemini" | "generateContent" | "streamGenerateContent" => Some(WireApi::Gemini),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
