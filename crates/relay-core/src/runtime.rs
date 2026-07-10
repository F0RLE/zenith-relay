use crate::sources::normalized_base_url;
use crate::{
    CandidateHealth, CandidateKind, CandidateQuota, CandidateScope, Error, LocalGatewayKey,
    ModelRegistry, ModelRules, PoolScheduler, ProviderSource, Result, RuntimeCandidate, Selection,
    SelectionRequest, UsageCallback, WireApi,
};
use futures_util::StreamExt;
use reqwest::header::{HeaderValue, AUTHORIZATION};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use subtle::ConstantTimeEq;
use url::Url;

pub(crate) const MAX_MODELS_BODY_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_NON_STREAM_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct RuntimeSource {
    pub source: ProviderSource,
    pub enabled: bool,
    pub draining: bool,
    pub priority: i32,
    pub weight: u32,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub last_used_at_ms: Option<u64>,
}

impl RuntimeSource {
    pub fn unrestricted(source: ProviderSource) -> Self {
        Self {
            source,
            enabled: true,
            draining: false,
            priority: 0,
            weight: 1,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            last_used_at_ms: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeLocalKey {
    pub key: LocalGatewayKey,
    pub enabled: bool,
    pub source_ids: Option<Vec<String>>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub model_prefix: Option<String>,
}

impl RuntimeLocalKey {
    pub fn unrestricted(key: LocalGatewayKey) -> Self {
        Self {
            key,
            enabled: true,
            source_ids: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GatewayRuntimeOptions {
    pub max_retry_candidates: usize,
    pub session_affinity_ttl: Option<Duration>,
    pub max_affinity_entries: usize,
}

impl Default for GatewayRuntimeOptions {
    fn default() -> Self {
        Self {
            max_retry_candidates: 3,
            session_affinity_ttl: Some(Duration::from_secs(3_600)),
            max_affinity_entries: 4_096,
        }
    }
}

pub struct GatewayRuntime {
    pub(crate) client: reqwest::Client,
    pub(crate) bounded_client: reqwest::Client,
    discovery_client: reqwest::Client,
    sources: BTreeMap<String, SourceExecutor>,
    keys: Vec<RuntimeKey>,
    scheduler: Arc<Mutex<PoolScheduler>>,
    registry: ModelRegistry,
    max_retry_candidates: usize,
    affinity_enabled: bool,
    pub(crate) usage: UsageCallback,
}

#[derive(Clone)]
pub(crate) struct AuthenticatedKey {
    pub(crate) id: String,
    pub(crate) scope: CandidateScope,
    pub(crate) model_rules: ModelRules,
    pub(crate) model_prefix: Option<String>,
}

pub(crate) struct SourceExecutor {
    pub(crate) id: String,
    pub(crate) wire_api: WireApi,
    pub(crate) responses_url: Url,
    pub(crate) chat_completions_url: Url,
    models_url: Url,
    source_authorization: HeaderValue,
    configured_models: BTreeSet<String>,
}

struct RuntimeKey {
    id: String,
    enabled: bool,
    secret_hash: [u8; 32],
    scope: CandidateScope,
    model_rules: ModelRules,
    model_prefix: Option<String>,
}

impl GatewayRuntime {
    pub fn new(
        source: ProviderSource,
        local_key: LocalGatewayKey,
        usage: UsageCallback,
    ) -> Result<Self> {
        Self::from_pool(
            vec![RuntimeSource::unrestricted(source)],
            vec![RuntimeLocalKey::unrestricted(local_key)],
            GatewayRuntimeOptions::default(),
            usage,
        )
    }

    pub fn from_pool(
        sources: Vec<RuntimeSource>,
        keys: Vec<RuntimeLocalKey>,
        options: GatewayRuntimeOptions,
        usage: UsageCallback,
    ) -> Result<Self> {
        if !(1..=8).contains(&options.max_retry_candidates) {
            return Err(Error::Validation(
                "max retry candidates must be between 1 and 8".to_string(),
            ));
        }

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

        let affinity_ttl_ms = options
            .session_affinity_ttl
            .map(|ttl| ttl.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default();
        let mut scheduler = PoolScheduler::new(options.max_affinity_entries, affinity_ttl_ms);
        let mut registry = ModelRegistry::default();
        let mut source_executors = BTreeMap::new();
        for source in sources {
            source.source.validate()?;
            if source.weight == 0 {
                return Err(Error::Validation(
                    "source weight must be at least one".to_string(),
                ));
            }
            if source_executors.contains_key(&source.source.id) {
                return Err(Error::Validation("source ids must be unique".to_string()));
            }
            let executor = SourceExecutor::new(&source.source)?;
            let models = normalized_set(source.source.models.iter());
            let candidate = RuntimeCandidate {
                id: source.source.id.clone(),
                kind: CandidateKind::ApiSource,
                source_id: source.source.id.clone(),
                account_id: None,
                protocol: source.source.wire_api,
                enabled: source.enabled,
                draining: source.draining,
                priority: source.priority,
                weight: source.weight,
                models: models.clone(),
                model_rules: model_rules(source.allowed_models, source.excluded_models),
                health: CandidateHealth::Healthy,
                quota: CandidateQuota::Unknown,
                cooldowns: BTreeMap::new(),
                last_used_at: source.last_used_at_ms,
                consecutive_failures: 0,
                secret_available: true,
            };
            registry.replace(candidate.id.clone(), models.iter());
            scheduler.upsert(candidate);
            source_executors.insert(source.source.id, executor);
        }

        let mut runtime_keys = Vec::new();
        let mut key_ids = HashSet::new();
        for key in keys {
            key.key.validate()?;
            if !key_ids.insert(key.key.id.clone()) {
                return Err(Error::Validation(
                    "local gateway key ids must be unique".to_string(),
                ));
            }
            runtime_keys.push(RuntimeKey {
                id: key.key.id,
                enabled: key.enabled,
                secret_hash: Sha256::digest(key.key.secret.as_bytes()).into(),
                scope: CandidateScope {
                    source_ids: key.source_ids.map(|ids| normalized_set(ids.iter())),
                    account_ids: None,
                    model_rules: ModelRules::default(),
                },
                model_rules: model_rules(key.allowed_models, key.excluded_models),
                model_prefix: normalize_prefix(key.model_prefix),
            });
        }

        if source_executors.is_empty() {
            return Err(Error::Validation(
                "at least one provider source is required".to_string(),
            ));
        }
        if !runtime_keys.iter().any(|key| key.enabled) {
            return Err(Error::Validation(
                "at least one enabled local gateway key is required".to_string(),
            ));
        }
        let has_usable_key = runtime_keys.iter().filter(|key| key.enabled).any(|key| {
            registry
                .visible_models(
                    &scheduler,
                    &key.scope,
                    &[WireApi::Responses, WireApi::ChatCompletions],
                    current_time_ms(),
                )
                .into_iter()
                .any(|model| key.model_rules.allows(&model))
        });
        if !has_usable_key {
            return Err(Error::Validation(
                "no enabled local key can reach an eligible Responses source".to_string(),
            ));
        }

        Ok(Self {
            client,
            bounded_client,
            discovery_client,
            sources: source_executors,
            keys: runtime_keys,
            scheduler: Arc::new(Mutex::new(scheduler)),
            registry,
            max_retry_candidates: options.max_retry_candidates,
            affinity_enabled: options.session_affinity_ttl.is_some(),
            usage,
        })
    }

    pub async fn discover_models(&self) -> Result<Vec<String>> {
        let source = self.sources.values().next().ok_or_else(|| {
            Error::Validation("at least one provider source is required".to_string())
        })?;
        discover_with(&self.discovery_client, source).await
    }

    pub(crate) fn authenticate(
        &self,
        authorization: Option<&HeaderValue>,
    ) -> Option<AuthenticatedKey> {
        let secret = authorization
            .and_then(|value| value.to_str().ok())
            .and_then(parse_bearer)?;
        let candidate: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
        self.keys
            .iter()
            .find(|key| key.enabled && bool::from(candidate.ct_eq(&key.secret_hash)))
            .map(|key| AuthenticatedKey {
                id: key.id.clone(),
                scope: key.scope.clone(),
                model_rules: key.model_rules.clone(),
                model_prefix: key.model_prefix.clone(),
            })
    }

    pub(crate) fn resolve_model(&self, key: &AuthenticatedKey, model: &str) -> Option<String> {
        let model = model.trim();
        if model.is_empty() {
            return None;
        }
        let model = match key.model_prefix.as_deref() {
            Some(prefix) => strip_prefix_ignore_ascii_case(model, &format!("{prefix}/"))?,
            None => model,
        };
        key.model_rules.allows(model).then(|| model.to_string())
    }

    pub(crate) fn visible_models(
        &self,
        key: &AuthenticatedKey,
        allowed_protocols: &[WireApi],
        now_ms: u64,
    ) -> Vec<String> {
        let scheduler = self.lock_scheduler();
        self.registry
            .visible_models(&scheduler, &key.scope, allowed_protocols, now_ms)
            .into_iter()
            .filter(|model| key.model_rules.allows(model))
            .map(|model| match key.model_prefix.as_deref() {
                Some(prefix) => format!("{prefix}/{model}"),
                None => model,
            })
            .collect()
    }

    pub(crate) fn select(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        affinity_key: Option<&str>,
        now_ms: u64,
    ) -> Option<Selection> {
        self.lock_scheduler().select(SelectionRequest {
            model,
            allowed_protocols,
            scope: &key.scope,
            tried,
            affinity_key,
            now_ms,
        })
    }

    pub(crate) fn earliest_retry_at(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        now_ms: u64,
    ) -> Option<u64> {
        self.lock_scheduler().earliest_retry_at(SelectionRequest {
            model,
            allowed_protocols,
            scope: &key.scope,
            tried,
            affinity_key: None,
            now_ms,
        })
    }

    pub(crate) fn source(&self, candidate_id: &str) -> Option<&SourceExecutor> {
        self.sources.get(candidate_id)
    }

    pub(crate) fn max_retry_candidates(&self) -> usize {
        self.max_retry_candidates
    }

    pub(crate) fn affinity_key(
        &self,
        key_id: &str,
        wire_api: WireApi,
        model: &str,
        session: Option<&str>,
    ) -> Option<String> {
        if !self.affinity_enabled {
            return None;
        }
        let session = session?.trim();
        if session.is_empty() {
            return None;
        }
        Some(format!(
            "{:x}",
            Sha256::digest(format!("{wire_api:?}\0{model}\0{key_id}\0{session}").as_bytes())
        ))
    }

    pub(crate) fn bind_affinity(&self, key: Option<&str>, candidate_id: &str, now_ms: u64) {
        if let Some(key) = key {
            self.lock_scheduler()
                .bind_affinity(key, candidate_id, now_ms);
        }
    }

    pub(crate) fn record_success(&self, candidate_id: &str, model: &str, now_ms: u64) {
        self.lock_scheduler()
            .record_success(candidate_id, model, now_ms);
    }

    pub(crate) fn record_failure(&self, candidate_id: &str) -> u32 {
        self.lock_scheduler()
            .record_failure(candidate_id)
            .unwrap_or(1)
    }

    pub(crate) fn set_cooldown(&self, candidate_id: &str, model: &str, retry_at_ms: u64) {
        self.lock_scheduler()
            .set_cooldown(candidate_id, model, retry_at_ms);
    }

    fn lock_scheduler(&self) -> MutexGuard<'_, PoolScheduler> {
        self.scheduler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl SourceExecutor {
    fn new(source: &ProviderSource) -> Result<Self> {
        let base_url = normalized_base_url(&source.base_url)?;
        let mut source_authorization = HeaderValue::from_str(&format!("Bearer {}", source.api_key))
            .map_err(|_| {
                Error::Validation("source API key contains invalid header characters".to_string())
            })?;
        source_authorization.set_sensitive(true);
        Ok(Self {
            id: source.id.clone(),
            wire_api: source.wire_api,
            responses_url: base_url
                .join("responses")
                .map_err(|_| Error::Validation("source responses URL is invalid".to_string()))?,
            chat_completions_url: base_url.join("chat/completions").map_err(|_| {
                Error::Validation("source chat completions URL is invalid".to_string())
            })?,
            models_url: base_url
                .join("models")
                .map_err(|_| Error::Validation("source models URL is invalid".to_string()))?,
            source_authorization,
            configured_models: normalized_set(source.models.iter()),
        })
    }

    pub(crate) fn source_authorization(&self) -> HeaderValue {
        self.source_authorization.clone()
    }

    pub(crate) fn endpoint(&self, wire_api: WireApi) -> Option<&Url> {
        (wire_api == self.wire_api).then_some(match wire_api {
            WireApi::Responses => &self.responses_url,
            WireApi::ChatCompletions => &self.chat_completions_url,
            WireApi::Messages => return None,
        })
    }

    pub(crate) fn canonical_model(&self, model: &str) -> Option<String> {
        self.configured_models
            .iter()
            .find(|candidate| candidate.eq_ignore_ascii_case(model))
            .cloned()
    }
}

impl fmt::Debug for GatewayRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayRuntime")
            .field("source_ids", &self.sources.keys().collect::<Vec<_>>())
            .field("local_key_count", &self.keys.len())
            .field("max_retry_candidates", &self.max_retry_candidates)
            .field("affinity_enabled", &self.affinity_enabled)
            .finish()
    }
}

impl fmt::Debug for SourceExecutor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceExecutor")
            .field("id", &self.id)
            .field("wire_api", &self.wire_api)
            .field("responses_url", &self.responses_url)
            .field("chat_completions_url", &self.chat_completions_url)
            .field("source_authorization", &"[redacted]")
            .field("configured_models", &self.configured_models)
            .finish()
    }
}

pub async fn discover_source_models(source: &ProviderSource) -> Result<Vec<String>> {
    source.validate()?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    discover_with(&client, &SourceExecutor::new(source)?).await
}

async fn discover_with(client: &reqwest::Client, source: &SourceExecutor) -> Result<Vec<String>> {
    let response = client
        .get(source.models_url.clone())
        .header(AUTHORIZATION, source.source_authorization())
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
    let data = body
        .get("data")
        .and_then(Value::as_array)
        .ok_or(Error::InvalidUpstreamResponse(
            "upstream model response is invalid",
        ))?;
    let mut seen = HashSet::new();
    Ok(data
        .iter()
        .filter_map(|model| model.get("id").and_then(Value::as_str))
        .filter(|model| {
            source.configured_models.is_empty()
                || source
                    .configured_models
                    .iter()
                    .any(|configured| configured.eq_ignore_ascii_case(model))
        })
        .filter(|model| seen.insert(model.to_ascii_lowercase()))
        .map(str::to_string)
        .collect())
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

fn parse_bearer(value: &str) -> Option<&str> {
    let (scheme, secret) = value.trim().split_once(char::is_whitespace)?;
    let secret = secret.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !secret.is_empty()).then_some(secret)
}

fn normalized_set<'a>(values: impl IntoIterator<Item = &'a String>) -> BTreeSet<String> {
    let mut normalized = BTreeMap::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            normalized
                .entry(value.to_ascii_lowercase())
                .or_insert_with(|| value.to_string());
        }
    }
    normalized.into_values().collect()
}

fn model_rules(allowed: Vec<String>, excluded: Vec<String>) -> ModelRules {
    ModelRules {
        allowed: normalized_set(allowed.iter()),
        excluded: normalized_set(excluded.iter()),
    }
}

fn normalize_prefix(prefix: Option<String>) -> Option<String> {
    prefix
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .then(|| &value[prefix.len()..])
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, key: &str, models: &[&str]) -> ProviderSource {
        ProviderSource {
            id: id.to_string(),
            name: id.to_string(),
            base_url: "https://example.test/v1".to_string(),
            api_key: key.to_string(),
            wire_api: WireApi::Responses,
            models: models.iter().map(|model| (*model).to_string()).collect(),
        }
    }

    fn key(id: &str, secret: &str) -> LocalGatewayKey {
        LocalGatewayKey {
            id: id.to_string(),
            secret: secret.to_string(),
        }
    }

    #[test]
    fn local_auth_returns_only_the_matching_redacted_key_policy() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source-1",
                "upstream-secret",
                &["gpt-test"],
            ))],
            vec![
                RuntimeLocalKey::unrestricted(key("key-1", "local-secret")),
                RuntimeLocalKey::unrestricted(key("key-2", "other-secret")),
            ],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();

        let authenticated = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        assert_eq!(authenticated.id, "key-1");
        assert!(runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer upstream-secret")))
            .is_none());
        assert!(!format!("{runtime:?}").contains("local-secret"));
        assert!(!format!("{runtime:?}").contains("upstream-secret"));
    }

    #[test]
    fn key_scope_and_prefix_filter_visible_models_without_scope_escalation() {
        let runtime = GatewayRuntime::from_pool(
            vec![
                RuntimeSource::unrestricted(source("source-a", "a", &["gpt-a"])),
                RuntimeSource::unrestricted(source("source-b", "b", &["gpt-b"])),
            ],
            vec![RuntimeLocalKey {
                key: key("key", "secret"),
                enabled: true,
                source_ids: Some(vec!["source-a".into()]),
                allowed_models: vec!["gpt-*".into()],
                excluded_models: vec!["gpt-b".into()],
                model_prefix: Some("team".into()),
            }],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let authenticated = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        assert_eq!(
            runtime.visible_models(&authenticated, &[WireApi::Responses], current_time_ms()),
            vec!["team/gpt-a"]
        );
        assert_eq!(
            runtime
                .resolve_model(&authenticated, "TEAM/gpt-a")
                .as_deref(),
            Some("gpt-a")
        );
    }

    #[test]
    fn explicit_empty_scope_cannot_start_a_gateway() {
        let error = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source-a",
                "a",
                &["gpt-a"],
            ))],
            vec![RuntimeLocalKey {
                key: key("key", "secret"),
                enabled: true,
                source_ids: Some(Vec::new()),
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                model_prefix: None,
            }],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("no enabled local key"));
    }
}
