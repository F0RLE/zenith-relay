mod stats;

pub use stats::{fetch_source_provider_stats, SourceProviderStats, SourceStatsProvider};
#[cfg(test)]
use stats::{openrouter_stats, source_stats_endpoint, source_stats_provider, zenith_stats};

use crate::{Error, Result};
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::fmt;
use url::{Host, Url};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireApi {
    Responses,
    ChatCompletions,
    Messages,
}

/// Associates an upstream-native wire contract with the models that are
/// explicitly known to work through that contract.
///
/// A source can expose the same model through more than one native endpoint.
/// Relay keeps those bindings separate so it never sends an Anthropic Messages
/// body to a Responses endpoint (or the reverse).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProtocolBinding {
    pub wire_api: WireApi,
    #[serde(default)]
    pub model_ids: Vec<String>,
}

impl SourceProtocolBinding {
    pub fn legacy(wire_api: WireApi, models: &[String]) -> Self {
        Self {
            wire_api,
            model_ids: models.to_vec(),
        }
    }
}

/// Normalizes source protocol bindings while keeping the source-provided
/// model order. Old configurations with only `wire_api` transparently become
/// one binding for every configured model.
pub fn normalize_source_protocol_bindings(
    bindings: Vec<SourceProtocolBinding>,
    fallback_wire_api: WireApi,
    models: &[String],
) -> Result<Vec<SourceProtocolBinding>> {
    let models = normalize_model_ids(models.iter().cloned());
    let known_models = models
        .iter()
        .map(|model| model.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let bindings = if bindings.is_empty() {
        vec![SourceProtocolBinding::legacy(fallback_wire_api, &models)]
    } else {
        bindings
    };
    let mut seen_protocols = BTreeSet::new();
    let mut normalized = Vec::with_capacity(bindings.len());

    for binding in bindings {
        if !seen_protocols.insert(binding.wire_api) {
            return Err(Error::Validation(
                "each source protocol may be configured only once".to_string(),
            ));
        }
        let mut model_ids = normalize_model_ids(binding.model_ids);
        if model_ids.is_empty() {
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
        normalized.push(SourceProtocolBinding {
            wire_api: binding.wire_api,
            model_ids,
        });
    }

    Ok(normalized)
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
        require_value("local key id", &self.id)?;
        require_value("local key secret", &self.secret)
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
    if url.scheme() == "http" && !is_loopback(&url) {
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
                    || (is_loopback(&source) && is_loopback(&gateway))
            });
    hosts_match
        && source.scheme() == gateway.scheme()
        && source.port_or_known_default() == gateway.port_or_known_default()
        && source.path() == gateway.path()
}

fn is_loopback(url: &Url) -> bool {
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

fn normalize_model_ids(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.to_ascii_lowercase()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
