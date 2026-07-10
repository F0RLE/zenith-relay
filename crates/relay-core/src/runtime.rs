use crate::sources::normalized_base_url;
use crate::{Error, LocalGatewayKey, ProviderSource, Result, UsageCallback, WireApi};
use futures_util::StreamExt;
use reqwest::header::{HeaderValue, AUTHORIZATION};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;
use std::time::Duration;
use subtle::ConstantTimeEq;
use url::Url;

pub(crate) const MAX_MODELS_BODY_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_NON_STREAM_BODY_BYTES: usize = 16 * 1024 * 1024;

pub struct GatewayRuntime {
    pub(crate) client: reqwest::Client,
    pub(crate) bounded_client: reqwest::Client,
    discovery_client: reqwest::Client,
    pub(crate) source_id: String,
    pub(crate) local_key_id: String,
    pub(crate) wire_api: WireApi,
    pub(crate) configured_models: Vec<String>,
    pub(crate) responses_url: Url,
    models_url: Url,
    source_authorization: HeaderValue,
    local_key_hash: [u8; 32],
    pub(crate) usage: UsageCallback,
}

impl GatewayRuntime {
    pub fn new(
        source: ProviderSource,
        local_key: LocalGatewayKey,
        usage: UsageCallback,
    ) -> Result<Self> {
        source.validate()?;
        local_key.validate()?;
        if source.wire_api != WireApi::Responses {
            return Err(Error::UnsupportedWireApi);
        }

        let base_url = normalized_base_url(&source.base_url)?;
        let mut source_authorization = HeaderValue::from_str(&format!("Bearer {}", source.api_key))
            .map_err(|_| {
                Error::Validation("source API key contains invalid header characters".to_string())
            })?;
        source_authorization.set_sensitive(true);
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let bounded_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(900))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let discovery_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        Ok(Self {
            client,
            bounded_client,
            discovery_client,
            source_id: source.id,
            local_key_id: local_key.id,
            wire_api: source.wire_api,
            configured_models: dedupe(source.models),
            responses_url: base_url
                .join("responses")
                .map_err(|_| Error::Validation("source responses URL is invalid".to_string()))?,
            models_url: base_url
                .join("models")
                .map_err(|_| Error::Validation("source models URL is invalid".to_string()))?,
            source_authorization,
            local_key_hash: Sha256::digest(local_key.secret.as_bytes()).into(),
            usage,
        })
    }

    pub async fn discover_models(&self) -> Result<Vec<String>> {
        let response = self
            .discovery_client
            .get(self.models_url.clone())
            .header(AUTHORIZATION, self.source_authorization.clone())
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Error::InvalidUpstreamResponse(
                "upstream model discovery failed",
            ));
        }

        let body = collect_limited(response, MAX_MODELS_BODY_BYTES).await?;
        let body: Value = serde_json::from_slice(&body)
            .map_err(|_| Error::InvalidUpstreamResponse("upstream model response is invalid"))?;
        let data =
            body.get("data")
                .and_then(Value::as_array)
                .ok_or(Error::InvalidUpstreamResponse(
                    "upstream model response is invalid",
                ))?;
        let configured: HashSet<&str> = self.configured_models.iter().map(String::as_str).collect();
        let mut seen = HashSet::new();
        Ok(data
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .filter(|model| configured.is_empty() || configured.contains(*model))
            .filter(|model| seen.insert((*model).to_string()))
            .map(str::to_string)
            .collect())
    }

    pub(crate) fn authenticate(&self, authorization: Option<&HeaderValue>) -> bool {
        let Some(secret) = authorization
            .and_then(|value| value.to_str().ok())
            .and_then(parse_bearer)
        else {
            return false;
        };
        let candidate: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
        bool::from(candidate.ct_eq(&self.local_key_hash))
    }

    pub(crate) fn source_authorization(&self) -> HeaderValue {
        self.source_authorization.clone()
    }

    pub(crate) fn model_allowed(&self, model: &str) -> bool {
        self.configured_models.is_empty()
            || self
                .configured_models
                .iter()
                .any(|allowed| allowed == model)
    }
}

pub(crate) async fn collect_limited(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(Error::UpstreamBodyTooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(Error::UpstreamBodyTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

impl fmt::Debug for GatewayRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayRuntime")
            .field("source_id", &self.source_id)
            .field("local_key_id", &self.local_key_id)
            .field("wire_api", &self.wire_api)
            .field("configured_models", &self.configured_models)
            .field("responses_url", &self.responses_url)
            .field("source_authorization", &"[redacted]")
            .field("local_key_hash", &"[redacted]")
            .finish()
    }
}

fn parse_bearer(value: &str) -> Option<&str> {
    let (scheme, secret) = value.trim().split_once(char::is_whitespace)?;
    let secret = secret.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !secret.is_empty()).then_some(secret)
}

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn runtime() -> GatewayRuntime {
        GatewayRuntime::new(
            ProviderSource {
                id: "source-1".to_string(),
                name: "Example".to_string(),
                base_url: "https://example.test/v1".to_string(),
                api_key: "upstream-secret".to_string(),
                wire_api: WireApi::Responses,
                models: vec![],
            },
            LocalGatewayKey {
                id: "key-1".to_string(),
                secret: "local-secret".to_string(),
            },
            Arc::new(|_| {}),
        )
        .unwrap()
    }

    #[test]
    fn local_auth_accepts_only_the_local_bearer_key() {
        let runtime = runtime();
        assert!(runtime.authenticate(Some(&HeaderValue::from_static("Bearer local-secret"))));
        assert!(!runtime.authenticate(Some(&HeaderValue::from_static("Bearer upstream-secret"))));
        assert!(!format!("{runtime:?}").contains("local-secret"));
        assert!(!format!("{runtime:?}").contains("upstream-secret"));
    }
}
