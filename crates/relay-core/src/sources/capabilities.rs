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

impl ProtocolFeature {
    pub const ALL: [Self; 7] = [
        Self::Text,
        Self::Streaming,
        Self::Images,
        Self::FunctionTools,
        Self::ToolChoice,
        Self::StructuredOutput,
        Self::Reasoning,
    ];
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
    /// Numeric restrictions for this exact source/model endpoint only.
    #[serde(default, skip_serializing_if = "ModelTokenLimits::is_empty")]
    pub token_limits: ModelTokenLimits,
    /// An explicit route list may narrow reference modes. None leaves the
    /// model reference in charge; a present empty list permits Standard only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tiers: Option<Vec<crate::DefaultServiceTier>>,
}

impl ModelEndpointCapability {
    /// Exact source/model declarations and configured endpoints take priority
    /// over manufacturer hints. Host profiles and probes do not establish this.
    pub(crate) fn explicitly_declares_endpoint(&self) -> bool {
        self.status.available()
            && matches!(
                self.origin,
                CapabilityOrigin::Catalog
                    | CapabilityOrigin::Manual
                    | CapabilityOrigin::EndpointUrl
            )
    }
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
mod limits;
pub(crate) use catalog::catalog_capabilities;
pub use config::SourceProtocolResolution;
pub use limits::ModelTokenLimits;
pub fn endpoint_url_protocol(base_url: &str) -> Option<WireApi> {
    let url = url::Url::parse(base_url).ok()?;
    let path = url.path().trim_end_matches('/');
    if path.ends_with("/interactions") {
        Some(WireApi::GeminiInteractions)
    } else if path.ends_with("/responses") {
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
    super::services::Service::from_host(url.host_str()?)?.protocol(&url)
}

pub(super) fn service_protocols(base_url: &str) -> Vec<WireApi> {
    let Ok(url) = url::Url::parse(base_url) else {
        return Vec::new();
    };
    url.host_str()
        .and_then(super::services::Service::from_host)
        .map(|service| service.protocols(&url))
        .unwrap_or_default()
}

pub(super) fn endpoint_type(endpoint_type_text: &str) -> Option<WireApi> {
    match endpoint_type_text {
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
