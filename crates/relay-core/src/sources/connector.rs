use super::{
    normalized_base_url, ProviderSource, SourceProtocolBinding, SourceProtocolBindingKey, WireApi,
};
use crate::{Error, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION};
use std::fmt;
use url::Url;

/// Source-local HTTP connector.
///
/// The connector owns only transport details that belong to a configured
/// source: endpoint construction, protocol-specific authentication, and model
/// membership. It deliberately does not translate request or response bodies;
/// that work stays in the protocol adapter core.
#[derive(Clone)]
pub struct SourceConnector {
    pub(crate) id: String,
    pub(crate) responses_url: Url,
    pub(crate) chat_completions_url: Url,
    pub(crate) messages_url: Url,
    pub(crate) models_url: Url,
    bearer_authorization: HeaderValue,
    messages_api_key: HeaderValue,
    protocol_bindings: Vec<SourceProtocolBinding>,
}

impl SourceConnector {
    pub fn new(source: &ProviderSource, bindings: &[SourceProtocolBinding]) -> Result<Self> {
        let base_url = normalized_base_url(&source.base_url)?;
        let mut bearer_authorization = HeaderValue::from_str(&format!("Bearer {}", source.api_key))
            .map_err(|_| {
                Error::Validation("source API key contains invalid header characters".to_string())
            })?;
        bearer_authorization.set_sensitive(true);
        let mut messages_api_key = HeaderValue::from_str(&source.api_key).map_err(|_| {
            Error::Validation("source API key contains invalid header characters".to_string())
        })?;
        messages_api_key.set_sensitive(true);
        Ok(Self {
            id: source.id.clone(),
            responses_url: base_url
                .join("responses")
                .map_err(|_| Error::Validation("source responses URL is invalid".to_string()))?,
            chat_completions_url: base_url.join("chat/completions").map_err(|_| {
                Error::Validation("source chat completions URL is invalid".to_string())
            })?,
            messages_url: base_url
                .join("messages")
                .map_err(|_| Error::Validation("source messages URL is invalid".to_string()))?,
            models_url: base_url
                .join("models")
                .map_err(|_| Error::Validation("source models URL is invalid".to_string()))?,
            bearer_authorization,
            messages_api_key,
            protocol_bindings: bindings.to_vec(),
        })
    }

    pub fn authorization(&self, wire_api: WireApi) -> (HeaderName, HeaderValue) {
        match wire_api {
            WireApi::Messages => (
                HeaderName::from_static("x-api-key"),
                self.messages_api_key.clone(),
            ),
            WireApi::Responses | WireApi::ChatCompletions => {
                (AUTHORIZATION, self.bearer_authorization.clone())
            }
        }
    }

    pub fn endpoint(&self, binding_key: SourceProtocolBindingKey) -> Option<&Url> {
        let binding = self.binding_for(binding_key)?;
        let upstream_wire_api = binding.adapter.upstream_wire_api(binding.wire_api);
        Some(match upstream_wire_api {
            WireApi::Responses => &self.responses_url,
            WireApi::ChatCompletions => &self.chat_completions_url,
            WireApi::Messages => &self.messages_url,
        })
    }

    pub fn canonical_model_for(
        &self,
        binding_key: SourceProtocolBindingKey,
        model: &str,
    ) -> Option<String> {
        self.models_for(binding_key)?
            .iter()
            .find(|candidate| candidate.eq_ignore_ascii_case(model))
            .cloned()
    }

    pub fn binding_for(
        &self,
        binding_key: SourceProtocolBindingKey,
    ) -> Option<&SourceProtocolBinding> {
        self.protocol_bindings
            .iter()
            .find(|binding| binding.key() == binding_key)
    }

    pub fn authorization_for_binding(
        &self,
        binding: &SourceProtocolBinding,
    ) -> (HeaderName, HeaderValue) {
        self.authorization(binding.adapter.upstream_wire_api(binding.wire_api))
    }

    /// Returns headers required by the selected upstream wire contract.
    ///
    /// Authentication stays separate because the runtime may refresh account
    /// credentials before a request is sent. Keeping protocol headers here
    /// means gateway execution does not need to know whether a source route is
    /// native Responses, native Messages, or a bridge.
    pub fn protocol_headers_for_binding(&self, binding: &SourceProtocolBinding) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if binding.adapter.upstream_wire_api(binding.wire_api) == WireApi::Messages {
            headers.insert(
                HeaderName::from_static("anthropic-version"),
                HeaderValue::from_static("2023-06-01"),
            );
        }
        headers
    }

    pub fn protocol_headers(&self, binding_key: SourceProtocolBindingKey) -> Option<HeaderMap> {
        self.binding_for(binding_key)
            .map(|binding| self.protocol_headers_for_binding(binding))
    }

    pub fn models_for(&self, binding_key: SourceProtocolBindingKey) -> Option<&[String]> {
        self.binding_for(binding_key)
            .map(|binding| binding.model_ids.as_slice())
    }

    pub fn protocol_bindings(&self) -> &[SourceProtocolBinding] {
        &self.protocol_bindings
    }
}

impl fmt::Debug for SourceConnector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceConnector")
            .field("id", &self.id)
            .field("responses_url", &self.responses_url)
            .field("chat_completions_url", &self.chat_completions_url)
            .field("messages_url", &self.messages_url)
            .field("authorization", &"[redacted]")
            .field("protocol_bindings", &self.protocol_bindings)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessagesReasoningMode, SourceAdapter};

    fn source() -> ProviderSource {
        ProviderSource {
            id: "source-1".to_string(),
            name: "Synthetic source".to_string(),
            base_url: "https://api.example.test/v1".to_string(),
            api_key: "source-secret".to_string(),
            wire_api: WireApi::Responses,
            models: vec!["responses-model".to_string(), "messages-model".to_string()],
        }
    }

    #[test]
    fn connector_owns_protocol_headers_for_native_and_bridged_routes() {
        let bindings = vec![
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["responses-model".to_string()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: Default::default(),
                model_ids: vec!["messages-model".to_string()],
            },
        ];
        let connector = SourceConnector::new(&source(), &bindings).unwrap();

        assert!(connector
            .protocol_headers(bindings[0].key())
            .unwrap()
            .is_empty());
        assert_eq!(
            connector
                .protocol_headers(bindings[1].key())
                .unwrap()
                .get("anthropic-version")
                .and_then(|value| value.to_str().ok()),
            Some("2023-06-01")
        );
    }
}
