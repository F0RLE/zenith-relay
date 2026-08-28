use super::{
    normalized_base_url, ProviderSource, SourceProtocolBinding, SourceProtocolBindingKey, WireApi,
};
use crate::{Error, Result, UpstreamProtocol};
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
    pub(crate) base_url: Url,
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
            base_url: base_url.clone(),
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

    /// Build the same connector below the conventional OpenAI `/v1` root.
    /// This is used only after an explicit root `/models` request returned
    /// 404; it is never a general URL guess for native non-OpenAI protocols.
    pub(crate) fn with_appended_v1(&self, bindings: &[SourceProtocolBinding]) -> Option<Self> {
        if self.base_url.path() != "/" {
            return None;
        }
        let mut base_url = self.base_url.clone();
        base_url.set_path("/v1/");
        Some(Self {
            id: self.id.clone(),
            responses_url: base_url.join("responses").ok()?,
            chat_completions_url: base_url.join("chat/completions").ok()?,
            messages_url: base_url.join("messages").ok()?,
            models_url: base_url.join("models").ok()?,
            base_url,
            bearer_authorization: self.bearer_authorization.clone(),
            messages_api_key: self.messages_api_key.clone(),
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
            WireApi::Gemini => (
                HeaderName::from_static("x-goog-api-key"),
                self.messages_api_key.clone(),
            ),
        }
    }

    fn authorization_for_protocol(&self, protocol: UpstreamProtocol) -> (HeaderName, HeaderValue) {
        match protocol {
            UpstreamProtocol::Messages => (
                HeaderName::from_static("x-api-key"),
                self.messages_api_key.clone(),
            ),
            UpstreamProtocol::GeminiGenerateContent => (
                HeaderName::from_static("x-goog-api-key"),
                self.messages_api_key.clone(),
            ),
            UpstreamProtocol::Responses | UpstreamProtocol::ChatCompletions => {
                (AUTHORIZATION, self.bearer_authorization.clone())
            }
        }
    }

    pub fn endpoint(
        &self,
        binding_key: SourceProtocolBindingKey,
        model: &str,
        stream: bool,
    ) -> Option<Url> {
        let binding = self.binding_for(binding_key)?;
        let protocol = binding.adapter.upstream_protocol(binding.wire_api);
        match protocol {
            UpstreamProtocol::Responses => Some(self.responses_url.clone()),
            UpstreamProtocol::ChatCompletions => Some(self.chat_completions_url.clone()),
            UpstreamProtocol::Messages => Some(self.messages_url.clone()),
            UpstreamProtocol::GeminiGenerateContent => self.gemini_endpoint(model, stream),
        }
    }

    fn gemini_endpoint(&self, model: &str, stream: bool) -> Option<Url> {
        let model = model.strip_prefix("models/").unwrap_or(model);
        if model.is_empty()
            || !model
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return None;
        }
        let mut url = self.base_url.clone();
        {
            let mut segments = url.path_segments_mut().ok()?;
            segments.pop_if_empty().push("models").push(model);
        }
        let path = format!(
            "{}:{}",
            url.path(),
            if stream {
                "streamGenerateContent"
            } else {
                "generateContent"
            }
        );
        url.set_path(&path);
        if stream {
            url.query_pairs_mut().append_pair("alt", "sse");
        }
        Some(url)
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
        self.authorization_for_protocol(binding.adapter.upstream_protocol(binding.wire_api))
    }

    /// Returns headers required by the selected upstream wire contract.
    ///
    /// Authentication stays separate because the runtime may refresh account
    /// credentials before a request is sent. Keeping protocol headers here
    /// means gateway execution does not need to know whether a source route is
    /// native Responses, native Messages, or a bridge.
    pub fn protocol_headers_for_binding(&self, binding: &SourceProtocolBinding) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if binding.adapter.upstream_protocol(binding.wire_api) == UpstreamProtocol::Messages {
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
            .field("base_url", &self.base_url)
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

    #[test]
    fn gemini_bridge_uses_model_url_and_google_key_header() {
        let bindings = vec![SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToGemini,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gemini-test".to_string()],
        }];
        let connector = SourceConnector::new(&source(), &bindings).unwrap();
        let (name, value) = connector.authorization_for_binding(&bindings[0]);
        assert_eq!(name, HeaderName::from_static("x-goog-api-key"));
        assert_eq!(value, "source-secret");
        assert_eq!(
            connector
                .endpoint(bindings[0].key(), "gemini-test", false)
                .unwrap()
                .as_str(),
            "https://api.example.test/v1/models/gemini-test:generateContent"
        );
        assert_eq!(
            connector
                .endpoint(bindings[0].key(), "gemini-test", true)
                .unwrap()
                .as_str(),
            "https://api.example.test/v1/models/gemini-test:streamGenerateContent?alt=sse"
        );
        assert!(connector
            .endpoint(bindings[0].key(), "../not-a-model", false)
            .is_none());
    }

    #[test]
    fn native_gemini_uses_the_same_upstream_contract_without_bridge_state() {
        let bindings = vec![SourceProtocolBinding {
            wire_api: WireApi::Gemini,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gemini-test".to_string()],
        }];
        let connector = SourceConnector::new(&source(), &bindings).unwrap();
        let (name, value) = connector.authorization_for_binding(&bindings[0]);
        assert_eq!(name, HeaderName::from_static("x-goog-api-key"));
        assert_eq!(value, "source-secret");
        assert_eq!(
            bindings[0].adapter.upstream_protocol(bindings[0].wire_api),
            UpstreamProtocol::GeminiGenerateContent
        );
        assert_eq!(
            connector
                .endpoint(bindings[0].key(), "gemini-test", false)
                .unwrap()
                .as_str(),
            "https://api.example.test/v1/models/gemini-test:generateContent"
        );
        assert_eq!(
            connector
                .endpoint(bindings[0].key(), "gemini-test", true)
                .unwrap()
                .as_str(),
            "https://api.example.test/v1/models/gemini-test:streamGenerateContent?alt=sse"
        );
    }
}
