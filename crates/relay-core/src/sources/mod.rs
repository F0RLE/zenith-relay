mod connector;
mod discovery;
mod stats;

pub use connector::SourceConnector;
pub(crate) use discovery::discover_models_with_client;
pub use discovery::{
    discover_source_models, discover_source_models_and_protocol_bindings,
    discover_source_models_for_protocol_bindings, SourceDiscovery,
};
pub use stats::{fetch_source_provider_stats, SourceProviderStats, SourceStatsProvider};
#[cfg(test)]
use stats::{openrouter_stats, source_stats_endpoint, source_stats_provider, zenith_stats};

use crate::{Error, MessagesReasoningMode, Result, SourceAdapter, UpstreamProtocol};
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use url::{Host, Url};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireApi {
    Responses,
    ChatCompletions,
    Messages,
}

impl WireApi {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Responses => "responses",
            Self::ChatCompletions => "chat_completions",
            Self::Messages => "messages",
        }
    }
}

/// Explicit Anthropic prompt-cache write lifetime for a Messages upstream.
/// `Provider` preserves the request as supplied; Relay never chooses a TTL
/// unless the source owner selected one.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum CacheWriteTtl {
    #[default]
    #[serde(rename = "provider")]
    Provider,
    #[serde(rename = "5m")]
    FiveMinutes,
    #[serde(rename = "1h")]
    OneHour,
}

impl CacheWriteTtl {
    pub const fn is_provider(&self) -> bool {
        matches!(self, Self::Provider)
    }

    pub const fn anthropic_ttl(self) -> Option<&'static str> {
        match self {
            Self::Provider => None,
            Self::FiveMinutes => Some("5m"),
            Self::OneHour => Some("1h"),
        }
    }

    pub fn from_anthropic_ttl(value: &str) -> Option<Self> {
        match value.trim() {
            "5m" => Some(Self::FiveMinutes),
            "1h" => Some(Self::OneHour),
            _ => None,
        }
    }
}

/// Associates a client-facing wire contract, an explicit adapter, and the
/// models that are known to work through that route.
///
/// `Native` keeps the client and upstream contracts equal. A bridge changes the
/// upstream contract only when it is explicitly selected and validated.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProtocolBinding {
    pub wire_api: WireApi,
    #[serde(default)]
    pub adapter: SourceAdapter,
    #[serde(default)]
    pub reasoning_mode: MessagesReasoningMode,
    #[serde(default)]
    #[serde(skip_serializing_if = "CacheWriteTtl::is_provider")]
    pub cache_write_ttl: CacheWriteTtl,
    #[serde(default)]
    pub model_ids: Vec<String>,
}

/// Stable in-memory identity for one source connector route.
///
/// A source can expose more than one Responses-facing route when the models
/// behind those routes require different upstream contracts. The adapter is
/// part of the identity: `responses/native` and
/// `responses/responses_to_messages` are distinct routes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceProtocolBindingKey {
    pub wire_api: WireApi,
    pub adapter: SourceAdapter,
}

impl SourceProtocolBinding {
    pub fn legacy(wire_api: WireApi, models: &[String]) -> Self {
        Self {
            wire_api,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: CacheWriteTtl::Provider,
            model_ids: models.to_vec(),
        }
    }

    pub const fn key(&self) -> SourceProtocolBindingKey {
        SourceProtocolBindingKey {
            wire_api: self.wire_api,
            adapter: self.adapter,
        }
    }
}

/// Normalizes source protocol bindings while keeping the source-provided
/// model order.
///
/// A legacy source has one implicit binding and therefore still expands an
/// empty model list to the source catalog. For an explicitly mixed source an
/// empty list means that the route has not been verified yet. Expanding it to
/// every source model would make an unknown route look usable, so the binding
/// stays empty until discovery fills it.
pub fn normalize_source_protocol_bindings(
    bindings: Vec<SourceProtocolBinding>,
    fallback_wire_api: WireApi,
    models: &[String],
) -> Result<Vec<SourceProtocolBinding>> {
    let models = crate::catalog::normalize_model_ids(models);
    let known_models = models
        .iter()
        .map(|model| model.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let bindings = if bindings.is_empty() {
        vec![SourceProtocolBinding::legacy(fallback_wire_api, &models)]
    } else {
        bindings
    };
    let mut seen_routes = BTreeSet::new();
    let mut assigned_models = BTreeMap::<WireApi, HashSet<String>>::new();
    let mut normalized = Vec::with_capacity(bindings.len());

    let expand_empty_models = bindings.len() == 1 && bindings[0].adapter.is_passthrough();
    for binding in bindings {
        binding
            .adapter
            .validate(binding.wire_api, binding.reasoning_mode)
            .map_err(|error| {
                Error::Validation(format!(
                    "source protocol binding is invalid: {}",
                    error.message()
                ))
            })?;
        if binding.cache_write_ttl != CacheWriteTtl::Provider
            && binding.adapter.upstream_protocol(binding.wire_api) != UpstreamProtocol::Messages
        {
            return Err(Error::Validation(
                "cache write TTL requires a Messages upstream route".to_string(),
            ));
        }
        if !seen_routes.insert(binding.key()) {
            return Err(Error::Validation(
                "each client protocol and adapter route may be configured only once".to_string(),
            ));
        }
        let mut model_ids = crate::catalog::normalize_model_ids(binding.model_ids);
        if model_ids.is_empty() && expand_empty_models {
            model_ids = models.clone();
        }
        if !known_models.is_empty()
            && model_ids
                .iter()
                .any(|model| !known_models.contains(&model.to_ascii_lowercase()))
        {
            return Err(Error::Validation(
                "source protocol binding references a model not exposed by the source".to_string(),
            ));
        }
        let assigned_for_protocol = assigned_models.entry(binding.wire_api).or_default();
        if model_ids
            .iter()
            .any(|model| !assigned_for_protocol.insert(model.to_ascii_lowercase()))
        {
            return Err(Error::Validation(
                "a model may be assigned to only one source route for the same client protocol"
                    .to_string(),
            ));
        }
        normalized.push(SourceProtocolBinding {
            wire_api: binding.wire_api,
            adapter: binding.adapter,
            reasoning_mode: binding.reasoning_mode,
            cache_write_ttl: binding.cache_write_ttl,
            model_ids,
        });
    }

    Ok(normalized)
}

/// Returns the normalized source models available through one client protocol.
pub fn source_models_for_wire_api(
    protocol_bindings: &[SourceProtocolBinding],
    fallback_wire_api: WireApi,
    source_models: &[String],
    wire_api: WireApi,
) -> Result<Vec<String>> {
    Ok(normalize_source_protocol_bindings(
        protocol_bindings.to_vec(),
        fallback_wire_api,
        source_models,
    )?
    .into_iter()
    .filter(|binding| binding.wire_api == wire_api)
    .flat_map(|binding| binding.model_ids)
    .collect())
}

#[derive(Clone)]
pub struct ProviderSource {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub wire_api: WireApi,
    pub models: Vec<String>,
}

impl ProviderSource {
    pub fn validate(&self) -> Result<()> {
        require_value("source id", &self.id)?;
        require_value("source name", &self.name)?;
        require_value("source API key", &self.api_key)?;

        let url = normalized_base_url(&self.base_url)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(Error::Validation(
                "source base URL must not contain credentials".to_string(),
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(Error::Validation(
                "source base URL must not contain a query or fragment".to_string(),
            ));
        }
        HeaderValue::from_str(&format!("Bearer {}", self.api_key)).map_err(|_| {
            Error::Validation("source API key contains invalid header characters".to_string())
        })?;
        if self.models.iter().any(|model| model.trim().is_empty()) {
            return Err(Error::Validation(
                "source model ids must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for ProviderSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderSource")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("base_url", &redacted_base_url(&self.base_url))
            .field("api_key", &"[redacted]")
            .field("wire_api", &self.wire_api)
            .field("models", &self.models)
            .finish()
    }
}

#[derive(Clone)]
pub struct LocalGatewayKey {
    pub id: String,
    pub secret: String,
}

impl LocalGatewayKey {
    pub fn validate(&self) -> Result<()> {
        require_value("gateway credential id", &self.id)?;
        require_value("gateway credential secret", &self.secret)
    }
}

impl fmt::Debug for LocalGatewayKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalGatewayKey")
            .field("id", &self.id)
            .field("secret", &"[redacted]")
            .finish()
    }
}

pub(crate) fn normalized_base_url(value: &str) -> Result<Url> {
    let mut url = Url::parse(value.trim())
        .map_err(|_| Error::Validation("source base URL is invalid".to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(Error::Validation(
            "source base URL must use HTTP or HTTPS".to_string(),
        ));
    }
    if url.scheme() == "http" && !is_loopback_url(&url) {
        return Err(Error::Validation(
            "unencrypted source base URLs are allowed only on loopback".to_string(),
        ));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

pub fn source_points_to_gateway(source_base_url: &str, gateway_base_url: &str) -> bool {
    let Ok(source) = normalized_base_url(source_base_url) else {
        return false;
    };
    let Ok(mut gateway) = Url::parse(gateway_base_url.trim()) else {
        return false;
    };
    if !gateway.path().ends_with('/') {
        gateway.set_path(&format!("{}/", gateway.path()));
    }
    let hosts_match =
        source
            .host_str()
            .zip(gateway.host_str())
            .is_some_and(|(source_host, gateway_host)| {
                source_host.eq_ignore_ascii_case(gateway_host)
                    || (is_loopback_url(&source) && is_loopback_url(&gateway))
            });
    hosts_match
        && source.scheme() == gateway.scheme()
        && source.port_or_known_default() == gateway.port_or_known_default()
        && source.path() == gateway.path()
}

pub fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn redacted_base_url(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return "[invalid]".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn require_value(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::Validation(format!("{name} must not be empty")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_empty_protocol_bindings_stay_unconfirmed() {
        let bindings = normalize_source_protocol_bindings(
            vec![
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
                    model_ids: Vec::new(),
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Messages,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
                    model_ids: Vec::new(),
                },
            ],
            WireApi::Responses,
            &["gpt-test".to_string(), "claude-test".to_string()],
        )
        .unwrap();
        assert_eq!(
            bindings,
            [
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
                    model_ids: Vec::new(),
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Messages,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
                    model_ids: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn single_empty_protocol_binding_keeps_legacy_source_catalog() {
        let bindings = normalize_source_protocol_bindings(
            vec![SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            }],
            WireApi::Responses,
            &["gpt-test".to_string()],
        )
        .unwrap();
        assert_eq!(bindings[0].model_ids, ["gpt-test"]);
    }

    #[test]
    fn single_empty_gemini_bridge_stays_unconfirmed() {
        let bindings = normalize_source_protocol_bindings(
            vec![SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToGemini,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            }],
            WireApi::Responses,
            &["gemini-3-pro".to_string()],
        )
        .unwrap();

        assert!(bindings[0].model_ids.is_empty());
    }

    #[test]
    fn bridge_binding_is_responses_only_and_reasoning_is_bridge_only() {
        let bridged = normalize_source_protocol_bindings(
            vec![SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".to_string()],
            }],
            WireApi::Responses,
            &["claude-test".to_string()],
        )
        .unwrap();
        assert_eq!(bridged[0].adapter, SourceAdapter::ResponsesToMessages);
        assert_eq!(bridged[0].reasoning_mode, MessagesReasoningMode::Adaptive);

        let invalid_protocol = normalize_source_protocol_bindings(
            vec![SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".to_string()],
            }],
            WireApi::Messages,
            &["claude-test".to_string()],
        )
        .unwrap_err();
        assert!(invalid_protocol
            .to_string()
            .contains("cannot serve this client protocol"));

        let invalid_reasoning = normalize_source_protocol_bindings(
            vec![SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Budget,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-test".to_string()],
            }],
            WireApi::Responses,
            &["gpt-test".to_string()],
        )
        .unwrap_err();
        assert!(invalid_reasoning
            .to_string()
            .contains("does not expose reasoning"));
    }

    #[test]
    fn responses_native_and_messages_bridge_are_distinct_source_routes() {
        let bindings = normalize_source_protocol_bindings(
            vec![
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
                    model_ids: vec!["gpt-test".to_string()],
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::ResponsesToMessages,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: Default::default(),
                    model_ids: vec!["claude-test".to_string()],
                },
            ],
            WireApi::Responses,
            &["gpt-test".to_string(), "claude-test".to_string()],
        )
        .unwrap();

        assert_eq!(bindings.len(), 2);
        assert_ne!(bindings[0].key(), bindings[1].key());
        assert_eq!(bindings[0].model_ids, ["gpt-test"]);
        assert_eq!(bindings[1].model_ids, ["claude-test"]);
    }

    #[test]
    fn model_cannot_be_assigned_to_two_routes_for_the_same_client_protocol() {
        let error = normalize_source_protocol_bindings(
            vec![
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
                    model_ids: vec!["shared-model".to_string()],
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::ResponsesToMessages,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
                    model_ids: vec!["shared-model".to_string()],
                },
            ],
            WireApi::Responses,
            &["shared-model".to_string()],
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("only one source route for the same client protocol"));
    }

    #[test]
    fn source_validation_rejects_unsafe_urls_and_redacts_secrets() {
        let mut source = ProviderSource {
            id: "source-1".to_string(),
            name: "Example".to_string(),
            base_url: "ftp://example.test/v1".to_string(),
            api_key: "upstream-secret".to_string(),
            wire_api: WireApi::Responses,
            models: vec!["model-1".to_string()],
        };
        assert!(source.validate().is_err());

        source.base_url = "https://user:password@example.test/v1".to_string();
        assert!(source.validate().is_err());
        assert!(!format!("{source:?}").contains("password"));

        source.base_url = "http://example.test/v1".to_string();
        assert!(source.validate().is_err());
        source.base_url = "http://127.0.0.1:14998/v1".to_string();
        assert!(source.validate().is_ok());
        assert!(!format!("{source:?}").contains("upstream-secret"));
    }

    #[test]
    fn source_protocol_helpers_preserve_canonical_models_and_loopback_rules() {
        let source_models = ["gpt-test".to_string(), "claude-test".to_string()];
        let bindings = [
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-test".to_string()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".to_string()],
            },
        ];

        assert_eq!(WireApi::Responses.as_str(), "responses");
        assert_eq!(WireApi::ChatCompletions.as_str(), "chat_completions");
        assert_eq!(WireApi::Messages.as_str(), "messages");
        assert_eq!(
            source_models_for_wire_api(
                &bindings,
                WireApi::Responses,
                &source_models,
                WireApi::Responses,
            )
            .unwrap(),
            ["gpt-test", "claude-test"]
        );
        assert!(is_loopback_url(&Url::parse("http://localhost").unwrap()));
        assert!(is_loopback_url(&Url::parse("http://[::1]").unwrap()));
        assert!(!is_loopback_url(
            &Url::parse("https://example.test").unwrap()
        ));
    }

    #[test]
    fn source_self_route_matches_only_the_same_gateway_endpoint() {
        assert!(source_points_to_gateway(
            "http://localhost:14998/v1",
            "http://127.0.0.1:14998/v1"
        ));
        assert!(source_points_to_gateway(
            "https://relay.example.test/v1/",
            "https://relay.example.test/v1"
        ));
        assert!(!source_points_to_gateway(
            "http://127.0.0.1:14999/v1",
            "http://127.0.0.1:14998/v1"
        ));
        assert!(!source_points_to_gateway(
            "https://provider.example.test/v1",
            "https://relay.example.test/v1"
        ));
    }

    #[test]
    fn provider_stats_use_numeric_micro_usd_and_reject_lookalike_hosts() {
        assert_eq!(
            source_stats_endpoint(
                SourceStatsProvider::Zenith,
                "https://api.zenithmarket.dev/v1"
            )
            .unwrap()
            .as_str(),
            "https://api.zenithmarket.dev/v1/zenith/key/stats"
        );
        assert_eq!(
            source_stats_endpoint(
                SourceStatsProvider::OpenRouter,
                "https://openrouter.ai/api/v1/"
            )
            .unwrap()
            .as_str(),
            "https://openrouter.ai/api/v1/credits"
        );
        assert_eq!(
            source_stats_provider("https://api.zenithmarket.dev/v1"),
            SourceStatsProvider::Zenith
        );
        assert_eq!(
            source_stats_provider("https://openrouter.ai.evil.test/api/v1"),
            SourceStatsProvider::Unsupported
        );
        let stats = openrouter_stats(&serde_json::json!({
            "data": { "total_credits": 12.5, "total_usage": 2.25 }
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&stats).unwrap()["provider"],
            "openrouter"
        );
        assert_eq!(stats.balance_micro_usd, Some(10_250_000));
        assert_eq!(stats.spent_micro_usd, Some(2_250_000));
        let stats = zenith_stats(&serde_json::json!({
            "data": {
                "displayBalanceMicrousd": 1234567,
                "spentCents": 250,
                "requests": 7,
                "totalTokens": 99
            }
        }))
        .unwrap();
        assert_eq!(stats.balance_micro_usd, Some(1_234_567));
        assert_eq!(stats.spent_micro_usd, Some(2_500_000));
        assert_eq!(stats.requests, Some(7));
    }
}
