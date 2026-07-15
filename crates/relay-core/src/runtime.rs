use crate::accounts::{
    AccountAuthState, TokenAuthority, TokenAuthorityError, TokenPersistenceAdapter,
    TokenRefreshAdapter,
};
use crate::sources::normalized_base_url;
use crate::ProxyConfig;
use crate::{
    CandidateHealth, CandidateKind, CandidateQuota, CandidateScope, Error, LocalGatewayKey,
    ModelRegistry, ModelRules, PoolScheduler, ProviderSource, Result, RoutingDiagnostics,
    RoutingStrategy, RuntimeCandidate, Selection, SelectionRequest, UsageCallback, WireApi,
};
use futures_util::StreamExt;
use reqwest::header::{HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use subtle::ConstantTimeEq;
use url::Url;

pub(crate) const MAX_MODELS_BODY_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_NON_STREAM_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_IDLE_CONNECTIONS_PER_HOST: usize = 256;

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

#[derive(Clone)]
pub struct RuntimeAccount {
    pub id: String,
    pub source_id: String,
    pub chatgpt_account_id: String,
    pub responses_url: String,
    pub models: Vec<String>,
    pub enabled: bool,
    pub draining: bool,
    pub priority: i32,
    pub weight: u32,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub health: CandidateHealth,
    pub quota: CandidateQuota,
    pub created_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
    pub cooldowns: BTreeMap<String, u64>,
    pub consecutive_failures: u32,
    pub proxy: Option<ProxyConfig>,
}

impl fmt::Debug for RuntimeAccount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeAccount")
            .field("id", &self.id)
            .field("source_id", &self.source_id)
            .field("chatgpt_account_id", &"[redacted]")
            .field("responses_url", &redacted_runtime_url(&self.responses_url))
            .field("models", &self.models)
            .field("enabled", &self.enabled)
            .field("draining", &self.draining)
            .field("priority", &self.priority)
            .field("weight", &self.weight)
            .field("allowed_models", &self.allowed_models)
            .field("excluded_models", &self.excluded_models)
            .field("health", &self.health)
            .field("quota", &self.quota)
            .field("created_at_ms", &self.created_at_ms)
            .field("last_used_at_ms", &self.last_used_at_ms)
            .field("cooldowns", &self.cooldowns)
            .field("consecutive_failures", &self.consecutive_failures)
            .field("proxy_configured", &self.proxy.is_some())
            .finish()
    }
}

pub struct RuntimeAccountAuth {
    pub token_authority: Arc<TokenAuthority>,
    pub refresh_adapter: Arc<dyn TokenRefreshAdapter>,
    pub persistence_adapter: Arc<dyn TokenPersistenceAdapter>,
    pub refresh_skew_ms: u64,
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

#[derive(Clone, Debug)]
pub struct RuntimeMixedLocalKey {
    pub key: LocalGatewayKey,
    pub enabled: bool,
    pub source_ids: Option<Vec<String>>,
    pub account_ids: Option<Vec<String>>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub model_prefix: Option<String>,
}

impl From<RuntimeLocalKey> for RuntimeMixedLocalKey {
    fn from(key: RuntimeLocalKey) -> Self {
        Self {
            key: key.key,
            enabled: key.enabled,
            source_ids: key.source_ids,
            account_ids: None,
            allowed_models: key.allowed_models,
            excluded_models: key.excluded_models,
            model_prefix: key.model_prefix,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultServiceTier {
    #[default]
    Standard,
    Fast,
}

impl DefaultServiceTier {
    pub(crate) fn upstream_value(self) -> Option<&'static str> {
        match self {
            Self::Standard => None,
            Self::Fast => Some("priority"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct GatewayRuntimeOptions {
    pub max_retry_candidates: usize,
    pub routing_strategy: RoutingStrategy,
    pub hidden_models: Vec<String>,
    pub default_service_tier: DefaultServiceTier,
}

impl Default for GatewayRuntimeOptions {
    fn default() -> Self {
        Self {
            max_retry_candidates: 3,
            routing_strategy: RoutingStrategy::Adaptive,
            hidden_models: Vec::new(),
            default_service_tier: DefaultServiceTier::Standard,
        }
    }
}

pub struct GatewayRuntime {
    pub(crate) client: reqwest::Client,
    pub(crate) bounded_client: reqwest::Client,
    websocket_client: reqwest::Client,
    discovery_client: reqwest::Client,
    sources: BTreeMap<String, SourceExecutor>,
    accounts: BTreeMap<String, AccountExecutor>,
    keys: Vec<RuntimeKey>,
    scheduler: Arc<Mutex<PoolScheduler>>,
    registry: ModelRegistry,
    codex_responses_lite_models: Mutex<BTreeSet<String>>,
    max_retry_candidates: usize,
    default_service_tier: DefaultServiceTier,
    pub(crate) usage: UsageCallback,
}

pub(crate) struct CandidateLease {
    scheduler: Arc<Mutex<PoolScheduler>>,
    candidate_id: String,
    released: AtomicBool,
}

impl CandidateLease {
    pub(crate) fn release(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        self.scheduler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .release(&self.candidate_id);
    }
}

impl Drop for CandidateLease {
    fn drop(&mut self) {
        self.release();
    }
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

struct AccountExecutor {
    id: String,
    source_id: String,
    chatgpt_account_id: HeaderValue,
    responses_url: Url,
    configured_models: BTreeSet<String>,
    token_authority: Arc<TokenAuthority>,
    refresh_adapter: Arc<dyn TokenRefreshAdapter>,
    persistence_adapter: Arc<dyn TokenPersistenceAdapter>,
    refresh_skew_ms: u64,
    client: Option<reqwest::Client>,
    bounded_client: Option<reqwest::Client>,
    websocket_client: Option<reqwest::Client>,
}

#[derive(Clone)]
pub(crate) struct ExecutorRoute {
    pub(crate) candidate_id: String,
    pub(crate) source_id: String,
    pub(crate) account_id: Option<String>,
    pub(crate) wire_api: WireApi,
    pub(crate) upstream_url: Url,
    pub(crate) source_model: String,
    pub(crate) routing: Option<RoutingDiagnostics>,
}

pub(crate) struct PreparedAuthorization {
    pub(crate) authorization: HeaderValue,
    pub(crate) chatgpt_account_id: Option<HeaderValue>,
    pub(crate) originator: Option<HeaderValue>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecutorPrepareError {
    Authentication,
    Persistence,
    Transient,
    InvalidCredential,
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
        Self::build(
            sources,
            Vec::new(),
            keys.into_iter().map(Into::into).collect(),
            None,
            options,
            usage,
        )
    }

    pub fn from_mixed_pool(
        sources: Vec<RuntimeSource>,
        accounts: Vec<RuntimeAccount>,
        keys: Vec<RuntimeMixedLocalKey>,
        account_auth: RuntimeAccountAuth,
        options: GatewayRuntimeOptions,
        usage: UsageCallback,
    ) -> Result<Self> {
        Self::build(sources, accounts, keys, Some(account_auth), options, usage)
    }

    fn build(
        sources: Vec<RuntimeSource>,
        accounts: Vec<RuntimeAccount>,
        keys: Vec<RuntimeMixedLocalKey>,
        account_auth: Option<RuntimeAccountAuth>,
        options: GatewayRuntimeOptions,
        usage: UsageCallback,
    ) -> Result<Self> {
        if !(1..=8).contains(&options.max_retry_candidates) {
            return Err(Error::Validation(
                "max retry candidates must be between 1 and 8".to_string(),
            ));
        }

        let client = runtime_client(None, false)?;
        let bounded_client = runtime_client(None, true)?;
        let websocket_client = runtime_websocket_client(None)?;
        let discovery_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        let mut scheduler = PoolScheduler::new();
        scheduler.set_routing_strategy(options.routing_strategy);
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

        let mut account_executors = BTreeMap::new();
        if !accounts.is_empty() && account_auth.is_none() {
            return Err(Error::Validation(
                "OAuth accounts require token authority adapters".to_string(),
            ));
        }
        for account in accounts {
            require_runtime_value("account candidate id", &account.id)?;
            require_runtime_value("account source id", &account.source_id)?;
            require_runtime_value("ChatGPT account id", &account.chatgpt_account_id)?;
            if account.weight == 0 {
                return Err(Error::Validation(
                    "account weight must be at least one".to_string(),
                ));
            }
            if source_executors.contains_key(&account.id)
                || account_executors.contains_key(&account.id)
            {
                return Err(Error::Validation(
                    "runtime candidate ids must be unique".to_string(),
                ));
            }
            let responses_url = normalized_responses_url(&account.responses_url)?;
            let client = account
                .proxy
                .as_ref()
                .map(|proxy| runtime_client(Some(proxy), false))
                .transpose()?;
            let bounded_client = account
                .proxy
                .as_ref()
                .map(|proxy| runtime_client(Some(proxy), true))
                .transpose()?;
            let websocket_client = account
                .proxy
                .as_ref()
                .map(|proxy| runtime_websocket_client(Some(proxy)))
                .transpose()?;
            let mut chatgpt_account_id = HeaderValue::from_str(&account.chatgpt_account_id)
                .map_err(|_| {
                    Error::Validation(
                        "ChatGPT account id contains invalid header characters".to_string(),
                    )
                })?;
            chatgpt_account_id.set_sensitive(true);
            let models = normalized_set(account.models.iter());
            let candidate = RuntimeCandidate {
                id: account.id.clone(),
                kind: CandidateKind::OAuthAccount,
                source_id: account.source_id.clone(),
                account_id: Some(account.id.clone()),
                protocol: WireApi::Responses,
                enabled: account.enabled,
                draining: account.draining,
                priority: account.priority,
                weight: account.weight,
                models: models.clone(),
                model_rules: model_rules(account.allowed_models, account.excluded_models),
                health: account.health,
                quota: account.quota,
                cooldowns: account.cooldowns,
                last_used_at: account.last_used_at_ms,
                consecutive_failures: account.consecutive_failures,
                secret_available: true,
            };
            let auth = account_auth
                .as_ref()
                .expect("account auth was validated above");
            registry.replace(candidate.id.clone(), models.iter());
            let candidate_id = candidate.id.clone();
            scheduler.upsert(candidate);
            scheduler.set_candidate_created_at(&candidate_id, account.created_at_ms);
            account_executors.insert(
                account.id.clone(),
                AccountExecutor {
                    id: account.id,
                    source_id: account.source_id,
                    chatgpt_account_id,
                    responses_url,
                    configured_models: models,
                    token_authority: auth.token_authority.clone(),
                    refresh_adapter: auth.refresh_adapter.clone(),
                    persistence_adapter: auth.persistence_adapter.clone(),
                    refresh_skew_ms: auth.refresh_skew_ms,
                    client,
                    bounded_client,
                    websocket_client,
                },
            );
        }

        let hidden_models = normalized_set(options.hidden_models.iter());
        let mut runtime_keys = Vec::new();
        let mut configured_key_rules = Vec::new();
        let mut key_ids = HashSet::new();
        for key in keys {
            key.key.validate()?;
            if !key_ids.insert(key.key.id.clone()) {
                return Err(Error::Validation(
                    "local gateway key ids must be unique".to_string(),
                ));
            }
            let scope = CandidateScope {
                source_ids: key.source_ids.map(|ids| normalized_set(ids.iter())),
                account_ids: key.account_ids.map(|ids| normalized_set(ids.iter())),
                model_rules: ModelRules::default(),
            };
            let base_model_rules = ModelRules {
                allowed: normalized_set(key.allowed_models.iter()),
                excluded: normalized_set(key.excluded_models.iter()),
            };
            configured_key_rules.push((key.enabled, scope.clone(), base_model_rules.clone()));
            let mut model_rules = base_model_rules;
            model_rules.excluded.extend(hidden_models.iter().cloned());
            runtime_keys.push(RuntimeKey {
                id: key.key.id,
                enabled: key.enabled,
                secret_hash: Sha256::digest(key.key.secret.as_bytes()).into(),
                scope,
                model_rules,
                model_prefix: normalize_prefix(key.model_prefix),
            });
        }

        if source_executors.is_empty() && account_executors.is_empty() {
            return Err(Error::Validation(
                "at least one provider source or OAuth account is required".to_string(),
            ));
        }
        if !runtime_keys.iter().any(|key| key.enabled) {
            return Err(Error::Validation(
                "at least one enabled local gateway key is required".to_string(),
            ));
        }
        let protocols = [WireApi::Responses, WireApi::ChatCompletions];
        let has_usable_key = configured_key_rules
            .iter()
            .filter(|(enabled, _, _)| *enabled)
            .any(|(_, scope, model_rules)| {
                scheduler.candidates().any(|candidate| {
                    candidate.models.iter().any(|model| {
                        model_rules.allows(model)
                            && candidate.is_configured(model, &protocols, scope)
                    })
                })
            });
        if !has_usable_key {
            return Err(Error::Validation(
                "no enabled local key can reach a configured Responses candidate".to_string(),
            ));
        }

        Ok(Self {
            client,
            bounded_client,
            websocket_client,
            discovery_client,
            sources: source_executors,
            accounts: account_executors,
            keys: runtime_keys,
            scheduler: Arc::new(Mutex::new(scheduler)),
            registry,
            codex_responses_lite_models: Mutex::new(BTreeSet::new()),
            max_retry_candidates: options.max_retry_candidates,
            default_service_tier: options.default_service_tier,
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

    pub(crate) async fn codex_models_routes(
        &self,
        key: &AuthenticatedKey,
        now_ms: u64,
    ) -> Vec<(String, Url)> {
        let routes = {
            let scheduler = self.lock_scheduler();
            self.accounts
                .values()
                .filter_map(|account| {
                    let candidate = scheduler.candidate(&account.id)?;
                    let visible_models = account
                        .configured_models
                        .iter()
                        .filter(|model| {
                            key.model_rules.allows(model)
                                && candidate.is_catalog_visible(
                                    model,
                                    &[WireApi::Responses],
                                    &key.scope,
                                )
                        })
                        .count();
                    if visible_models == 0 {
                        return None;
                    }
                    let mut url = account.responses_url.clone();
                    let mut segments = url.path_segments_mut().ok()?;
                    segments.pop_if_empty().pop().push("models");
                    drop(segments);
                    Some((account.id.clone(), url, visible_models))
                })
                .collect::<Vec<_>>()
        };
        let mut ranked = Vec::with_capacity(routes.len());
        for (account_id, url, visible_models) in routes {
            let Some(account) = self.accounts.get(&account_id) else {
                continue;
            };
            let auth_state = account.token_authority.auth_state(&account_id).await;
            let tokens = account.token_authority.tokens(&account_id).await;
            let can_prepare = !matches!(auth_state, Some(AccountAuthState::RequiresReauth(_)));
            let token_rank = match tokens {
                Some(tokens)
                    if can_prepare && tokens.is_access_usable(now_ms, account.refresh_skew_ms) =>
                {
                    2_u8
                }
                Some(tokens) if can_prepare && tokens.refresh_token().is_some() => 1_u8,
                _ => 0_u8,
            };
            ranked.push((account_id, url, visible_models, token_rank));
        }
        ranked.sort_by(|left, right| {
            right
                .3
                .cmp(&left.3)
                .then_with(|| right.2.cmp(&left.2))
                .then_with(|| left.0.cmp(&right.0))
        });
        ranked
            .into_iter()
            .map(|(account_id, url, _, _)| (account_id, url))
            .collect()
    }

    pub(crate) fn remember_codex_responses_lite_model(&self, model: &str) {
        self.codex_responses_lite_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(model.to_ascii_lowercase());
    }

    pub(crate) fn codex_model_uses_responses_lite(&self, model: &str) -> bool {
        self.codex_responses_lite_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&model.to_ascii_lowercase())
    }

    pub(crate) fn visible_account_models(&self, key: &AuthenticatedKey) -> Vec<String> {
        let scheduler = self.lock_scheduler();
        let mut models = BTreeSet::new();
        for account in self.accounts.values() {
            let Some(candidate) = scheduler.candidate(&account.id) else {
                continue;
            };
            for model in &account.configured_models {
                if key.model_rules.allows(model)
                    && candidate.is_catalog_visible(model, &[WireApi::Responses], &key.scope)
                {
                    models.insert(match key.model_prefix.as_deref() {
                        Some(prefix) => format!("{prefix}/{model}"),
                        None => model.clone(),
                    });
                }
            }
        }
        models.into_iter().collect()
    }

    pub(crate) fn api_source_candidate_ids(&self) -> HashSet<String> {
        self.sources.keys().cloned().collect()
    }

    pub fn visible_models_for_secret(
        &self,
        secret: &str,
        allowed_protocols: &[WireApi],
        now_ms: u64,
    ) -> Vec<String> {
        let candidate: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
        let Some(key) = self
            .keys
            .iter()
            .find(|key| key.enabled && bool::from(candidate.ct_eq(&key.secret_hash)))
        else {
            return Vec::new();
        };
        self.visible_models(
            &AuthenticatedKey {
                id: key.id.clone(),
                scope: key.scope.clone(),
                model_rules: key.model_rules.clone(),
                model_prefix: key.model_prefix.clone(),
            },
            allowed_protocols,
            now_ms,
        )
    }

    pub fn update_candidate_availability(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota: CandidateQuota,
    ) -> bool {
        self.lock_scheduler()
            .update_candidate_availability(candidate_id, enabled, health, quota)
    }

    pub fn set_protected_candidate(
        &self,
        candidate_id: Option<&str>,
        reserve_basis_points: u64,
    ) -> bool {
        self.lock_scheduler()
            .set_protected_candidate(candidate_id, reserve_basis_points)
    }

    pub(crate) fn select_and_reserve(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        response_affinity_key: Option<&str>,
        now_ms: u64,
    ) -> Option<(Selection, CandidateLease)> {
        let mut scheduler = self.lock_scheduler();
        let selection = scheduler.select(SelectionRequest {
            model,
            allowed_protocols,
            scope: &key.scope,
            tried,
            response_affinity_key,
            now_ms,
        })?;
        scheduler.reserve(&selection.candidate_id).then(|| {
            let lease = CandidateLease {
                scheduler: self.scheduler.clone(),
                candidate_id: selection.candidate_id.clone(),
                released: AtomicBool::new(false),
            };
            (selection, lease)
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
            response_affinity_key: None,
            now_ms,
        })
    }

    pub(crate) fn executor_route(&self, candidate_id: &str, model: &str) -> Option<ExecutorRoute> {
        if let Some(source) = self.sources.get(candidate_id) {
            let source_model = source.canonical_model(model)?;
            return Some(ExecutorRoute {
                candidate_id: source.id.clone(),
                source_id: source.id.clone(),
                account_id: None,
                wire_api: source.wire_api,
                upstream_url: source.endpoint(source.wire_api)?.clone(),
                source_model,
                routing: None,
            });
        }
        let account = self.accounts.get(candidate_id)?;
        let source_model = account.canonical_model(model)?;
        Some(ExecutorRoute {
            candidate_id: account.id.clone(),
            source_id: account.source_id.clone(),
            account_id: Some(account.id.clone()),
            wire_api: WireApi::Responses,
            upstream_url: account.responses_url.clone(),
            source_model,
            routing: None,
        })
    }

    pub(crate) async fn prepare_authorization(
        &self,
        candidate_id: &str,
        now_ms: u64,
    ) -> std::result::Result<PreparedAuthorization, ExecutorPrepareError> {
        if let Some(source) = self.sources.get(candidate_id) {
            return Ok(PreparedAuthorization {
                authorization: source.source_authorization(),
                chatgpt_account_id: None,
                originator: None,
            });
        }
        let account = self
            .accounts
            .get(candidate_id)
            .ok_or(ExecutorPrepareError::Authentication)?;
        let prepared = account
            .token_authority
            .prepare_and_persist(
                &account.id,
                now_ms,
                account.refresh_skew_ms,
                account.refresh_adapter.as_ref(),
                account.persistence_adapter.as_ref(),
            )
            .await
            .map_err(classify_token_authority_error)?;
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {}", prepared.tokens.access_token()))
                .map_err(|_| ExecutorPrepareError::InvalidCredential)?;
        authorization.set_sensitive(true);
        Ok(PreparedAuthorization {
            authorization,
            chatgpt_account_id: Some(account.chatgpt_account_id.clone()),
            originator: Some(HeaderValue::from_static("codex_cli_rs")),
        })
    }

    pub(crate) fn request_client(
        &self,
        candidate_id: &str,
        upstream_stream: bool,
    ) -> &reqwest::Client {
        if let Some(account) = self.accounts.get(candidate_id) {
            let client = if upstream_stream {
                account.client.as_ref()
            } else {
                account.bounded_client.as_ref()
            };
            if let Some(client) = client {
                return client;
            }
        }
        if upstream_stream {
            &self.client
        } else {
            &self.bounded_client
        }
    }

    pub(crate) fn websocket_client(&self, candidate_id: &str) -> &reqwest::Client {
        self.accounts
            .get(candidate_id)
            .and_then(|account| account.websocket_client.as_ref())
            .unwrap_or(&self.websocket_client)
    }

    pub(crate) fn max_retry_candidates(&self) -> usize {
        self.max_retry_candidates
    }

    pub(crate) fn default_service_tier(&self) -> DefaultServiceTier {
        self.default_service_tier
    }

    pub(crate) fn response_affinity_key(&self, response_id: Option<&str>) -> Option<String> {
        let response_id = response_id?.trim();
        if response_id.is_empty() {
            return None;
        }
        Some(format!(
            "{:x}",
            Sha256::digest(format!("response\0{response_id}").as_bytes())
        ))
    }

    pub(crate) fn bind_response_affinity(
        &self,
        response_id: Option<&str>,
        candidate_id: &str,
        now_ms: u64,
    ) {
        if let Some(key) = self.response_affinity_key(response_id) {
            self.lock_scheduler()
                .bind_response_affinity(key, candidate_id, now_ms);
        }
    }

    pub(crate) fn record_success_with_metrics(
        &self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
        output_tokens: Option<u64>,
        latency_ms: u64,
    ) {
        self.lock_scheduler().record_success_with_metrics(
            candidate_id,
            model,
            now_ms,
            output_tokens,
            latency_ms,
        );
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

impl AccountExecutor {
    fn canonical_model(&self, model: &str) -> Option<String> {
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
            .field(
                "account_candidate_ids",
                &self.accounts.keys().collect::<Vec<_>>(),
            )
            .field("local_key_count", &self.keys.len())
            .field("max_retry_candidates", &self.max_retry_candidates)
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

fn normalized_responses_url(value: &str) -> Result<Url> {
    let url = Url::parse(value.trim())
        .map_err(|_| Error::Validation("account Responses URL is invalid".to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(Error::Validation(
            "account Responses URL must use HTTP or HTTPS".to_string(),
        ));
    }
    if url.scheme() == "http" && !is_loopback_url(&url) {
        return Err(Error::Validation(
            "unencrypted account Responses URLs are allowed only on loopback".to_string(),
        ));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Validation(
            "account Responses URL must not contain credentials, query, or fragment".to_string(),
        ));
    }
    Ok(url)
}

fn runtime_client(proxy: Option<&ProxyConfig>, bounded: bool) -> Result<reqwest::Client> {
    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(MAX_IDLE_CONNECTIONS_PER_HOST)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_nodelay(true)
        .http2_adaptive_window(true)
        .redirect(reqwest::redirect::Policy::none());
    let builder = if bounded {
        builder.timeout(Duration::from_secs(900))
    } else {
        builder.read_timeout(Duration::from_secs(300))
    };
    let builder = match proxy {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    };
    builder.build().map_err(Error::from)
}

fn runtime_websocket_client(proxy: Option<&ProxyConfig>) -> Result<reqwest::Client> {
    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(MAX_IDLE_CONNECTIONS_PER_HOST)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_nodelay(true)
        .http1_only()
        .redirect(reqwest::redirect::Policy::none());
    match proxy {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(Error::from)
}

fn redacted_runtime_url(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return "[invalid]".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn is_loopback_url(url: &Url) -> bool {
    url.host().is_some_and(|host| match host {
        url::Host::Domain(host) => host.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    })
}

fn require_runtime_value(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(Error::Validation(format!("{name} must not be empty")))
    } else {
        Ok(())
    }
}

fn classify_token_authority_error(error: TokenAuthorityError) -> ExecutorPrepareError {
    match error {
        TokenAuthorityError::AccessTokenExpired
        | TokenAuthorityError::RequiresReauth(_)
        | TokenAuthorityError::AccountNotFound
        | TokenAuthorityError::InvalidAccountId => ExecutorPrepareError::Authentication,
        TokenAuthorityError::PersistenceRequired | TokenAuthorityError::PersistenceFailed(_) => {
            ExecutorPrepareError::Persistence
        }
        TokenAuthorityError::RefreshFailed(_)
        | TokenAuthorityError::InvalidCapacity
        | TokenAuthorityError::CapacityReached => ExecutorPrepareError::Transient,
    }
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .then(|| &value[prefix.len()..])
}

#[cfg(test)]
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
            runtime.visible_models_for_secret("secret", &[WireApi::Responses], current_time_ms()),
            vec!["team/gpt-a"]
        );
        assert!(runtime
            .visible_models_for_secret("wrong", &[WireApi::Responses], current_time_ms())
            .is_empty());
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

    #[test]
    fn global_hidden_models_apply_to_listing_and_requests() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source-a",
                "a",
                &["gpt-new", "gpt-old"],
            ))],
            vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
            GatewayRuntimeOptions {
                hidden_models: vec!["GPT-OLD".into()],
                ..GatewayRuntimeOptions::default()
            },
            Arc::new(|_| {}),
        )
        .unwrap();
        let authenticated = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();

        assert_eq!(
            runtime.visible_models(&authenticated, &[WireApi::Responses], current_time_ms()),
            vec!["gpt-new"]
        );
        assert!(runtime.resolve_model(&authenticated, "gpt-old").is_none());
    }
}
