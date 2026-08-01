use crate::accounts::{
    AccountAuthState, TokenAuthority, TokenAuthorityError, TokenPersistenceAdapter,
    TokenRefreshAdapter,
};
use crate::catalog::source_context_windows;
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::{
    is_agent_identity_task_invalid_response, AgentIdentityCredential, AgentIdentityError,
    CodexIdentityEnvelope, RuntimeChatGptAccount, RuntimeChatGptAuth,
};
use crate::quota::QuotaSnapshot;
use crate::sources::normalized_base_url;
use crate::ProxyConfig;
use crate::{
    api_model_price, decode_codex_model_alias, normalize_subscription_plan_order, CandidateHealth,
    CandidateKind, CandidateQuota, CandidateScope, Error, LocalGatewayKey, ModelRegistry,
    ModelRules, PoolScheduler, ProviderSource, Result, RoutingDiagnostics, RoutingStrategy,
    RuntimeCandidate, Selection, SelectionRequest, UsageCallback, UsageEvent, WireApi,
    RESPONSE_AFFINITY_TTL_MS,
};
use futures_util::{future::join_all, StreamExt};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cmp::Ordering as CmpOrdering;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;
use subtle::ConstantTimeEq;
use url::Url;

pub(crate) const MAX_MODELS_BODY_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_NON_STREAM_BODY_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const IMAGE_API_MODEL: &str = "gpt-image-2";
const MAX_IDLE_CONNECTIONS_PER_HOST: usize = 256;
const PASSIVE_QUOTA_PERSIST_DEBOUNCE_MS: u64 = 5_000;
const CODEX_SOURCE_MODEL_MANIFEST_TTL_MS: u64 = 5 * 60 * 1_000;

#[derive(Clone, Debug)]
pub struct RuntimeSource {
    pub source: ProviderSource,
    pub enabled: bool,
    pub draining: bool,
    pub priority: i32,
    pub weight: u32,
    pub recovery_delay_seconds: u64,
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
            recovery_delay_seconds: 0,
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

#[derive(Clone, Debug)]
pub struct RuntimeMixedLocalKey {
    pub key: LocalGatewayKey,
    pub enabled: bool,
    pub source_ids: Option<Vec<String>>,
    pub account_ids: Option<Vec<String>>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub model_prefix: Option<String>,
    pub wire_apis: Option<Vec<ClientWireApi>>,
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
            wire_apis: None,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponseAffinityBinding {
    pub key: String,
    pub candidate_id: String,
    pub expires_at_ms: u64,
}

pub trait ResponseAffinityStore: Send + Sync {
    fn load(&self, now_ms: u64) -> std::result::Result<Vec<ResponseAffinityBinding>, String>;
    fn find(
        &self,
        key: &str,
        now_ms: u64,
    ) -> std::result::Result<Option<ResponseAffinityBinding>, String>;
    fn upsert(&self, binding: &ResponseAffinityBinding) -> std::result::Result<(), String>;
    fn delete(&self, key: &str) -> std::result::Result<(), String>;
    fn delete_candidate(&self, candidate_id: &str) -> std::result::Result<(), String>;
}

#[derive(Clone)]
pub struct GatewayRuntimeOptions {
    pub max_retry_candidates: usize,
    pub routing_strategy: RoutingStrategy,
    pub subscription_plan_order: Vec<String>,
    pub hidden_models: Vec<String>,
    pub default_service_tier: DefaultServiceTier,
    pub quota_stale_after_ms: u64,
    /// Optional text model used as the Responses image-generation bridge.
    /// `None` selects the cheapest known compatible model per account.
    pub image_base_model: Option<String>,
    pub response_affinity_store: Option<Arc<dyn ResponseAffinityStore>>,
    pub provider_storm_breaker: bool,
}

impl fmt::Debug for GatewayRuntimeOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayRuntimeOptions")
            .field("max_retry_candidates", &self.max_retry_candidates)
            .field("routing_strategy", &self.routing_strategy)
            .field("subscription_plan_order", &self.subscription_plan_order)
            .field("hidden_models", &self.hidden_models)
            .field("default_service_tier", &self.default_service_tier)
            .field("quota_stale_after_ms", &self.quota_stale_after_ms)
            .field("image_base_model", &self.image_base_model)
            .field(
                "response_affinity_store",
                &self.response_affinity_store.as_ref().map(|_| "configured"),
            )
            .field("provider_storm_breaker", &self.provider_storm_breaker)
            .finish()
    }
}

impl Default for GatewayRuntimeOptions {
    fn default() -> Self {
        Self {
            max_retry_candidates: 3,
            routing_strategy: RoutingStrategy::Adaptive,
            subscription_plan_order: Vec::new(),
            hidden_models: Vec::new(),
            default_service_tier: DefaultServiceTier::Standard,
            quota_stale_after_ms: crate::QUOTA_STALE_AFTER_MS,
            image_base_model: None,
            response_affinity_store: None,
            provider_storm_breaker: false,
        }
    }
}

pub struct GatewayRuntime {
    pub(crate) client: reqwest::Client,
    pub(crate) bounded_client: reqwest::Client,
    websocket_client: reqwest::Client,
    discovery_client: reqwest::Client,
    sources: BTreeMap<String, SourceExecutor>,
    source_recovery_delays_ms: BTreeMap<String, u64>,
    chatgpt_accounts: BTreeMap<String, ChatGptAccountExecutor>,
    keys: Vec<RuntimeKey>,
    scheduler: Arc<Mutex<PoolScheduler>>,
    registry: ModelRegistry,
    codex_responses_lite_models: Mutex<BTreeSet<String>>,
    codex_model_manifests: Mutex<BTreeMap<String, CachedCodexManifest>>,
    passive_quotas: Mutex<BTreeMap<String, PassiveQuotaState>>,
    max_retry_candidates: usize,
    quota_stale_after_ms: u64,
    default_service_tier_fast: AtomicBool,
    response_affinity_store: Option<Arc<dyn ResponseAffinityStore>>,
    pub(crate) usage: UsageCallback,
}

#[derive(Clone, Debug)]
struct PassiveQuotaState {
    snapshot: QuotaSnapshot,
    dirty: bool,
    force_persist: bool,
    last_persist_hint_ms: u64,
}

#[derive(Clone, Debug)]
struct CachedCodexManifest {
    value: Value,
    observed_at_ms: u64,
}

pub(crate) struct CandidateLease {
    scheduler: Arc<Mutex<PoolScheduler>>,
    candidate_id: String,
    model: String,
    lane: CandidateLeaseLane,
    released: AtomicBool,
}

pub(crate) struct ExecutionFence {
    scheduler: Arc<Mutex<PoolScheduler>>,
    candidate_id: String,
    released: AtomicBool,
}

#[derive(Clone, Copy)]
enum CandidateLeaseLane {
    Text,
    Image,
}

impl CandidateLease {
    pub(crate) fn release(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut scheduler = self
            .scheduler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match self.lane {
            CandidateLeaseLane::Text => {
                scheduler.release_for(&self.candidate_id, Some(&self.model));
            }
            CandidateLeaseLane::Image => {
                scheduler.release_image_for(&self.candidate_id, Some(&self.model));
            }
        }
    }
}

impl Drop for CandidateLease {
    fn drop(&mut self) {
        self.release();
    }
}

impl Drop for ExecutionFence {
    fn drop(&mut self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        self.scheduler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_execution_fence(&self.candidate_id, false);
    }
}

#[derive(Clone)]
pub(crate) struct AuthenticatedKey {
    pub(crate) id: String,
    pub(crate) scope: CandidateScope,
    pub(crate) model_rules: ModelRules,
    pub(crate) model_prefix: Option<String>,
    client_wire_apis: Option<Vec<ClientWireApi>>,
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

struct ChatGptAccountExecutor {
    id: String,
    source_id: String,
    identity: CodexIdentityEnvelope,
    responses_url: Url,
    configured_models: BTreeSet<String>,
    image_main_model: Option<String>,
    token_authority: Arc<TokenAuthority>,
    refresh_adapter: Arc<dyn TokenRefreshAdapter>,
    persistence_adapter: Arc<dyn TokenPersistenceAdapter>,
    refresh_skew_ms: u64,
    client: reqwest::Client,
    bounded_client: reqwest::Client,
    websocket_client: reqwest::Client,
    active: AtomicBool,
    agent_identity: RwLock<Option<AgentIdentityCredential>>,
    agent_task_lock: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
pub(crate) struct ExecutorRoute {
    pub(crate) candidate_id: String,
    pub(crate) source_id: String,
    pub(crate) account_id: Option<String>,
    pub(crate) wire_api: WireApi,
    pub(crate) service_tier: DefaultServiceTier,
    pub(crate) upstream_url: Url,
    pub(crate) source_model: String,
    pub(crate) half_open_probe: bool,
    pub(crate) routing: Option<RoutingDiagnostics>,
}

pub(crate) struct PreparedAuthorization {
    pub(crate) authorization: HeaderValue,
    pub(crate) identity: Option<CodexIdentityEnvelope>,
    pub(crate) token_generation: Option<u64>,
    pub(crate) agent_task_id: Option<String>,
}

#[derive(Debug)]
pub(crate) enum AuthorizedRequestError {
    Prepare(ExecutorPrepareError),
    Transport(reqwest::Error),
    NotReplayable,
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
    client_wire_apis: Option<Vec<ClientWireApi>>,
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
        accounts: Vec<RuntimeChatGptAccount>,
        keys: Vec<RuntimeMixedLocalKey>,
        account_auth: RuntimeChatGptAuth,
        options: GatewayRuntimeOptions,
        usage: UsageCallback,
    ) -> Result<Self> {
        Self::build(sources, accounts, keys, Some(account_auth), options, usage)
    }

    fn build(
        sources: Vec<RuntimeSource>,
        accounts: Vec<RuntimeChatGptAccount>,
        keys: Vec<RuntimeMixedLocalKey>,
        account_auth: Option<RuntimeChatGptAuth>,
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
        let subscription_plan_order =
            normalize_subscription_plan_order(options.subscription_plan_order.clone())
                .map_err(|message| Error::Validation(message.to_string()))?;
        scheduler.set_subscription_plan_order(&subscription_plan_order);
        scheduler.set_quota_stale_after_ms(options.quota_stale_after_ms);
        scheduler.set_provider_storm_breaker_enabled(options.provider_storm_breaker);
        let mut registry = ModelRegistry::default();
        let mut source_executors = BTreeMap::new();
        let mut source_recovery_delays_ms = BTreeMap::new();
        for source in sources {
            source.source.validate()?;
            if source.weight == 0 {
                return Err(Error::Validation(
                    "source weight must be at least one".to_string(),
                ));
            }
            if source.recovery_delay_seconds > 24 * 60 * 60 {
                return Err(Error::Validation(
                    "source recovery delay must not exceed 24 hours".to_string(),
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
                quota_updated_at_ms: None,
                quota_reset_at_ms: None,
                cooldowns: BTreeMap::new(),
                last_used_at: source.last_used_at_ms,
                consecutive_failures: 0,
                secret_available: true,
            };
            registry.replace(candidate.id.clone(), models.iter());
            scheduler.upsert(candidate);
            if source.recovery_delay_seconds > 0 {
                source_recovery_delays_ms.insert(
                    source.source.id.clone(),
                    source.recovery_delay_seconds.saturating_mul(1_000),
                );
            }
            source_executors.insert(source.source.id, executor);
        }

        let mut account_executors = BTreeMap::new();
        let mut passive_quotas = BTreeMap::new();
        let image_base_model = normalize_image_base_model(options.image_base_model.clone())?;
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
            passive_quotas.insert(
                account.id.clone(),
                PassiveQuotaState {
                    last_persist_hint_ms: account.quota_snapshot.updated_at_ms.unwrap_or_default(),
                    snapshot: account.quota_snapshot.clone(),
                    dirty: false,
                    force_persist: false,
                },
            );
            // OAuth identities must not share an HTTP/2 connection pool. A connection-level
            // failure for one account would otherwise abort concurrent streams on other accounts.
            let client = runtime_client(account.proxy.as_ref(), false)?;
            let bounded_client = runtime_client(account.proxy.as_ref(), true)?;
            let websocket_client = runtime_websocket_client(account.proxy.as_ref())?;
            let identity = CodexIdentityEnvelope::standard(&account.chatgpt_account_id)
                .map_err(|message| Error::Validation(message.to_string()))?;
            let models = normalized_set(account.models.iter());
            let image_main_model = select_image_main_model(&models, image_base_model.as_deref());
            let mut candidate_models = models.clone();
            if image_main_model.is_some() {
                candidate_models.insert(IMAGE_API_MODEL.to_string());
            }
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
                models: candidate_models.clone(),
                model_rules: model_rules(account.allowed_models, account.excluded_models),
                health: account.health,
                quota: account.quota,
                quota_updated_at_ms: account.quota_updated_at_ms,
                quota_reset_at_ms: account.quota_snapshot.limiting_reset_at_ms(),
                cooldowns: BTreeMap::new(),
                last_used_at: account.last_used_at_ms,
                consecutive_failures: 0,
                secret_available: true,
            };
            let auth = account_auth
                .as_ref()
                .expect("account auth was validated above");
            registry.replace(candidate.id.clone(), candidate_models.iter());
            let candidate_id = candidate.id.clone();
            scheduler.upsert(candidate);
            scheduler.set_candidate_subscription_expiry(
                &candidate_id,
                account.subscription_expires_at_ms,
            );
            scheduler.set_candidate_subscription_plan(
                &candidate_id,
                account.subscription_plan_type.as_deref(),
            );
            account_executors.insert(
                account.id.clone(),
                ChatGptAccountExecutor {
                    id: account.id,
                    source_id: account.source_id,
                    identity,
                    responses_url,
                    configured_models: models,
                    image_main_model,
                    token_authority: auth.token_authority.clone(),
                    refresh_adapter: auth.refresh_adapter.clone(),
                    persistence_adapter: auth.persistence_adapter.clone(),
                    refresh_skew_ms: auth.refresh_skew_ms,
                    client,
                    bounded_client,
                    websocket_client,
                    active: AtomicBool::new(true),
                    agent_identity: RwLock::new(auth.agent_identities.get(&candidate_id).cloned()),
                    agent_task_lock: tokio::sync::Mutex::new(()),
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
            let client_wire_apis = key.wire_apis.map(|values| {
                values
                    .into_iter()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            });
            if client_wire_apis.as_ref().is_some_and(Vec::is_empty) {
                return Err(Error::Validation(
                    "client key wire scope must not be empty".to_string(),
                ));
            }
            runtime_keys.push(RuntimeKey {
                id: key.key.id,
                enabled: key.enabled,
                secret_hash: Sha256::digest(key.key.secret.as_bytes()).into(),
                scope,
                model_rules,
                model_prefix: normalize_prefix(key.model_prefix),
                client_wire_apis,
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

        let affinity_store = options.response_affinity_store.clone();
        if let Some(store) = affinity_store.as_ref() {
            let now_ms = runtime_now_ms();
            if let Ok(bindings) = store.load(now_ms) {
                for binding in bindings {
                    if !scheduler.restore_response_affinity(
                        binding.key.clone(),
                        &binding.candidate_id,
                        binding.expires_at_ms,
                        now_ms,
                    ) {
                        let _ = store.delete(&binding.key);
                    }
                }
            }
        }

        Ok(Self {
            client,
            bounded_client,
            websocket_client,
            discovery_client,
            sources: source_executors,
            source_recovery_delays_ms,
            chatgpt_accounts: account_executors,
            keys: runtime_keys,
            scheduler: Arc::new(Mutex::new(scheduler)),
            registry,
            codex_responses_lite_models: Mutex::new(BTreeSet::new()),
            codex_model_manifests: Mutex::new(BTreeMap::new()),
            passive_quotas: Mutex::new(passive_quotas),
            max_retry_candidates: options.max_retry_candidates,
            quota_stale_after_ms: options.quota_stale_after_ms,
            default_service_tier_fast: AtomicBool::new(
                options.default_service_tier == DefaultServiceTier::Fast,
            ),
            response_affinity_store: affinity_store,
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
                client_wire_apis: key.client_wire_apis.clone(),
            })
    }

    pub(crate) fn allows_client_wire_api(
        &self,
        key: &AuthenticatedKey,
        wire_api: ClientWireApi,
    ) -> bool {
        key.client_wire_apis
            .as_ref()
            .is_none_or(|allowed| allowed.contains(&wire_api))
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

    pub(crate) fn resolve_visible_model(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        now_ms: u64,
    ) -> Option<String> {
        let visible = self.visible_models(key, allowed_protocols, now_ms);
        self.resolve_from_visible(key, model, &visible)
    }

    pub(crate) fn resolve_visible_account_model(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> Option<String> {
        self.resolve_from_visible(key, model, &self.visible_account_models(key))
    }

    fn resolve_from_visible(
        &self,
        key: &AuthenticatedKey,
        requested: &str,
        visible: &[String],
    ) -> Option<String> {
        let resolve = |candidate: &str| {
            let resolved = self.resolve_model(key, candidate)?;
            visible
                .iter()
                .filter_map(|visible| self.resolve_model(key, visible))
                .any(|visible| visible.eq_ignore_ascii_case(&resolved))
                .then_some(resolved)
        };
        resolve(requested)
            .or_else(|| decode_codex_model_alias(requested).and_then(|id| resolve(&id)))
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
            self.chatgpt_accounts
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
            let Some(account) = self.chatgpt_accounts.get(&account_id) else {
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

    pub(crate) async fn codex_source_context_windows(
        &self,
        key: &AuthenticatedKey,
        allowed_protocols: &[WireApi],
        now_ms: u64,
    ) -> BTreeMap<String, u64> {
        let routes = {
            let scheduler = self.lock_scheduler();
            self.sources
                .iter()
                .filter_map(|(source_id, source)| {
                    let candidate = scheduler.candidate(source_id)?;
                    source
                        .configured_models
                        .iter()
                        .any(|model| {
                            key.model_rules.allows(model)
                                && candidate.is_catalog_visible(
                                    model,
                                    allowed_protocols,
                                    &key.scope,
                                )
                        })
                        .then(|| {
                            (
                                source_id.clone(),
                                source.models_url.clone(),
                                source.source_authorization(),
                                source.configured_models.clone(),
                            )
                        })
                })
                .collect::<Vec<_>>()
        };
        let manifests = join_all(routes.into_iter().map(
            |(source_id, models_url, authorization, configured_models)| async move {
                if let Some(value) = self.fresh_codex_model_manifest(
                    &source_id,
                    now_ms,
                    CODEX_SOURCE_MODEL_MANIFEST_TTL_MS,
                ) {
                    return Some((configured_models, value));
                }
                let response = self
                    .discovery_client
                    .get(models_url)
                    .header(AUTHORIZATION, authorization)
                    .timeout(Duration::from_secs(10))
                    .send()
                    .await
                    .ok()?;
                if !response.status().is_success() {
                    return None;
                }
                let body = collect_limited(response, MAX_MODELS_BODY_BYTES)
                    .await
                    .ok()?;
                let value = serde_json::from_slice::<Value>(&body).ok()?;
                self.remember_codex_model_manifest(&source_id, value.clone(), now_ms);
                Some((configured_models, value))
            },
        ))
        .await;

        let mut windows: BTreeMap<String, u64> = BTreeMap::new();
        for (configured_models, manifest) in manifests.into_iter().flatten() {
            for (model, context_window) in source_context_windows(&manifest, &configured_models) {
                windows
                    .entry(model)
                    .and_modify(|existing| *existing = (*existing).min(context_window))
                    .or_insert(context_window);
            }
        }
        windows
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

    pub(crate) fn remember_codex_model_manifest(
        &self,
        candidate_id: &str,
        value: Value,
        observed_at_ms: u64,
    ) {
        let scheduler = self.lock_scheduler();
        if scheduler.candidate(candidate_id).is_none() {
            return;
        }
        self.codex_model_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                candidate_id.to_string(),
                CachedCodexManifest {
                    value,
                    observed_at_ms,
                },
            );
    }

    fn fresh_codex_model_manifest(
        &self,
        candidate_id: &str,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Option<Value> {
        self.codex_model_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(candidate_id)
            .filter(|manifest| now_ms.saturating_sub(manifest.observed_at_ms) <= ttl_ms)
            .map(|manifest| manifest.value.clone())
    }

    pub(crate) fn stale_codex_model_manifest<'a>(
        &self,
        candidate_ids: impl IntoIterator<Item = &'a str>,
    ) -> Option<Value> {
        let manifests = self
            .codex_model_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        candidate_ids
            .into_iter()
            .filter_map(|candidate_id| manifests.get(candidate_id))
            .max_by_key(|manifest| manifest.observed_at_ms)
            .map(|manifest| manifest.value.clone())
    }

    pub(crate) fn visible_account_models(&self, key: &AuthenticatedKey) -> Vec<String> {
        let scheduler = self.lock_scheduler();
        let mut models = BTreeSet::new();
        for account in self.chatgpt_accounts.values() {
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
                client_wire_apis: key.client_wire_apis.clone(),
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

    pub fn update_candidate_availability_at(
        &self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota: CandidateQuota,
        quota_updated_at_ms: Option<u64>,
    ) -> bool {
        self.lock_scheduler().update_candidate_availability_at(
            candidate_id,
            enabled,
            health,
            quota,
            quota_updated_at_ms,
        )
    }

    pub fn set_candidate_health(&self, candidate_id: &str, health: CandidateHealth) -> bool {
        self.lock_scheduler()
            .set_candidate_health(candidate_id, health)
    }

    pub fn remove_candidate(&self, candidate_id: &str) -> bool {
        let removed = self.lock_scheduler().remove(candidate_id).is_some();
        self.codex_model_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(candidate_id);
        self.passive_quotas
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(candidate_id);
        if let Some(account) = self.chatgpt_accounts.get(candidate_id) {
            account.active.store(false, Ordering::Release);
            *account
                .agent_identity
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        if let Some(store) = self.response_affinity_store.as_ref() {
            let _ = store.delete_candidate(candidate_id);
        }
        removed
    }

    pub fn candidate_runtime_order(&self) -> Vec<crate::CandidateRuntimeSnapshot> {
        self.lock_scheduler().runtime_order(runtime_now_ms())
    }

    pub(crate) fn account_candidate_is_active(&self, candidate_id: &str) -> bool {
        self.chatgpt_accounts
            .get(candidate_id)
            .is_some_and(|account| account.active.load(Ordering::Acquire))
    }

    pub fn set_protected_candidate(
        &self,
        candidate_id: Option<&str>,
        reserve_basis_points: u64,
    ) -> bool {
        self.lock_scheduler()
            .set_protected_candidate(candidate_id, reserve_basis_points)
    }

    pub fn clear_candidate_cooldown(&self, candidate_id: &str, model: &str) -> bool {
        self.lock_scheduler().clear_cooldown(candidate_id, model)
    }

    pub fn set_candidate_cooldown(
        &self,
        candidate_id: &str,
        model: &str,
        retry_at_ms: u64,
    ) -> bool {
        self.lock_scheduler()
            .set_cooldown(candidate_id, model, retry_at_ms)
    }

    pub fn reset_candidate_failures(&self, candidate_id: &str) -> bool {
        self.lock_scheduler().reset_failures(candidate_id)
    }

    pub(crate) fn select_and_reserve(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        affinity_keys: (Option<&str>, Option<&str>),
        now_ms: u64,
    ) -> Option<(Selection, CandidateLease)> {
        let (response_affinity_key, prompt_affinity_key) = affinity_keys;
        self.select_and_reserve_for(
            key,
            model,
            allowed_protocols,
            tried,
            response_affinity_key,
            prompt_affinity_key,
            now_ms,
            CandidateLeaseLane::Text,
        )
    }

    pub(crate) fn select_and_reserve_image(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        now_ms: u64,
    ) -> Option<(Selection, CandidateLease)> {
        self.select_and_reserve_for(
            key,
            model,
            allowed_protocols,
            tried,
            None,
            None,
            now_ms,
            CandidateLeaseLane::Image,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn select_and_reserve_for(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        response_affinity_key: Option<&str>,
        prompt_affinity_key: Option<&str>,
        now_ms: u64,
        lane: CandidateLeaseLane,
    ) -> Option<(Selection, CandidateLease)> {
        if let (Some(key), Some(store)) =
            (response_affinity_key, self.response_affinity_store.as_ref())
        {
            let cached = self.lock_scheduler().has_response_affinity(key, now_ms);
            if !cached {
                if let Ok(Some(binding)) = store.find(key, now_ms) {
                    self.lock_scheduler().restore_response_affinity(
                        binding.key,
                        &binding.candidate_id,
                        binding.expires_at_ms,
                        now_ms,
                    );
                }
            }
        }
        let mut scheduler = self.lock_scheduler();
        let request = SelectionRequest {
            model,
            allowed_protocols,
            scope: &key.scope,
            tried,
            response_affinity_key,
            prompt_affinity_key,
            now_ms,
        };
        let selection = match lane {
            CandidateLeaseLane::Text => scheduler.select(request),
            CandidateLeaseLane::Image => scheduler.select_image(request),
        }?;
        let reserved = match lane {
            CandidateLeaseLane::Text => {
                scheduler.reserve_for(&selection.candidate_id, model, now_ms)
            }
            CandidateLeaseLane::Image => {
                scheduler.reserve_image_for(&selection.candidate_id, model, now_ms)
            }
        }
        .then(|| {
            let lease = CandidateLease {
                scheduler: self.scheduler.clone(),
                candidate_id: selection.candidate_id.clone(),
                model: model.to_string(),
                lane,
                released: AtomicBool::new(false),
            };
            (selection, lease)
        });
        drop(scheduler);
        if let (Some((selection, _)), Some(key)) = (reserved.as_ref(), response_affinity_key) {
            if selection.response_affinity_hit {
                self.persist_response_affinity(key, &selection.candidate_id, now_ms);
            }
        }
        reserved
    }

    pub(crate) fn earliest_retry_at(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        response_affinity_key: Option<&str>,
        now_ms: u64,
    ) -> Option<u64> {
        self.lock_scheduler().earliest_retry_at(SelectionRequest {
            model,
            allowed_protocols,
            scope: &key.scope,
            tried,
            response_affinity_key,
            prompt_affinity_key: None,
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
                service_tier: DefaultServiceTier::Standard,
                upstream_url: source.endpoint(source.wire_api)?.clone(),
                source_model,
                half_open_probe: false,
                routing: None,
            });
        }
        let account = self.chatgpt_accounts.get(candidate_id)?;
        let source_model = account.canonical_model(model)?;
        Some(ExecutorRoute {
            candidate_id: account.id.clone(),
            source_id: account.source_id.clone(),
            account_id: Some(account.id.clone()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            upstream_url: account.responses_url.clone(),
            source_model,
            half_open_probe: false,
            routing: None,
        })
    }

    pub(crate) fn image_executor_route(&self, candidate_id: &str) -> Option<ExecutorRoute> {
        if let Some(source) = self.sources.get(candidate_id) {
            let source_model = source.canonical_model(IMAGE_API_MODEL)?;
            return Some(ExecutorRoute {
                candidate_id: source.id.clone(),
                source_id: source.id.clone(),
                account_id: None,
                wire_api: source.wire_api,
                service_tier: DefaultServiceTier::Standard,
                upstream_url: source.responses_url.clone(),
                source_model,
                half_open_probe: false,
                routing: None,
            });
        }
        let account = self.chatgpt_accounts.get(candidate_id)?;
        Some(ExecutorRoute {
            candidate_id: account.id.clone(),
            source_id: account.source_id.clone(),
            account_id: Some(account.id.clone()),
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            upstream_url: account.responses_url.clone(),
            source_model: account.image_main_model.clone()?,
            half_open_probe: false,
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
                identity: None,
                token_generation: None,
                agent_task_id: None,
            });
        }
        let account = self
            .chatgpt_accounts
            .get(candidate_id)
            .ok_or(ExecutorPrepareError::Authentication)?;
        if !account.active.load(Ordering::Acquire) {
            return Err(ExecutorPrepareError::Authentication);
        }
        if account
            .agent_identity
            .read()
            .map_err(|_| ExecutorPrepareError::Transient)?
            .is_some()
        {
            match account.ensure_agent_identity_task(None).await {
                Ok(agent) => {
                    return Ok(PreparedAuthorization {
                        authorization: agent
                            .authorization(now_ms)
                            .map_err(|_| ExecutorPrepareError::InvalidCredential)?,
                        identity: Some(account.identity.clone()),
                        token_generation: None,
                        agent_task_id: agent.task_id().map(str::to_string),
                    });
                }
                Err(error) if account.token_authority.tokens(&account.id).await.is_none() => {
                    return Err(error);
                }
                Err(_) => {}
            }
        }
        self.prepare_oauth_authorization(candidate_id, account, now_ms)
            .await
    }

    async fn prepare_oauth_authorization(
        &self,
        candidate_id: &str,
        account: &ChatGptAccountExecutor,
        now_ms: u64,
    ) -> std::result::Result<PreparedAuthorization, ExecutorPrepareError> {
        let prepared = match account
            .token_authority
            .prepare_and_persist(
                &account.id,
                now_ms,
                account.refresh_skew_ms,
                account.refresh_adapter.as_ref(),
                account.persistence_adapter.as_ref(),
            )
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let health = match &error {
                    TokenAuthorityError::RequiresReauth(_) => Some(CandidateHealth::ReauthRequired),
                    TokenAuthorityError::AccessTokenExpired
                    | TokenAuthorityError::AccountNotFound
                    | TokenAuthorityError::InvalidAccountId => Some(CandidateHealth::Unhealthy),
                    _ => None,
                };
                if let Some(health) = health {
                    self.set_candidate_health(candidate_id, health);
                }
                return Err(classify_token_authority_error(error));
            }
        };
        let mut authorization =
            match HeaderValue::from_str(&format!("Bearer {}", prepared.tokens.access_token())) {
                Ok(authorization) => authorization,
                Err(_) => {
                    self.set_candidate_health(candidate_id, CandidateHealth::Unhealthy);
                    return Err(ExecutorPrepareError::InvalidCredential);
                }
            };
        authorization.set_sensitive(true);
        Ok(PreparedAuthorization {
            authorization,
            identity: Some(account.identity.clone()),
            token_generation: Some(prepared.tokens.generation()),
            agent_task_id: None,
        })
    }

    pub(crate) async fn refresh_authorization_after_unauthorized(
        &self,
        candidate_id: &str,
        failed_generation: Option<u64>,
        now_ms: u64,
    ) -> std::result::Result<PreparedAuthorization, ExecutorPrepareError> {
        let account = self
            .chatgpt_accounts
            .get(candidate_id)
            .ok_or(ExecutorPrepareError::Authentication)?;
        if !account.active.load(Ordering::Acquire) {
            return Err(ExecutorPrepareError::Authentication);
        }
        account
            .token_authority
            .invalidate_access_generation_and_persist(
                &account.id,
                failed_generation,
                now_ms,
                account.persistence_adapter.as_ref(),
            )
            .await
            .map_err(classify_token_authority_error)?;
        self.prepare_authorization(candidate_id, now_ms).await
    }

    pub(crate) async fn refresh_agent_identity_task_after_unauthorized(
        &self,
        candidate_id: &str,
        expected_task_id: &str,
        now_ms: u64,
    ) -> std::result::Result<PreparedAuthorization, ExecutorPrepareError> {
        let account = self
            .chatgpt_accounts
            .get(candidate_id)
            .ok_or(ExecutorPrepareError::Authentication)?;
        if !account.active.load(Ordering::Acquire) {
            return Err(ExecutorPrepareError::Authentication);
        }
        match account
            .ensure_agent_identity_task(Some(expected_task_id))
            .await
        {
            Ok(_) => self.prepare_authorization(candidate_id, now_ms).await,
            Err(error) if account.token_authority.tokens(&account.id).await.is_none() => Err(error),
            Err(_) => {
                self.prepare_oauth_authorization(candidate_id, account, now_ms)
                    .await
            }
        }
    }

    pub(crate) async fn send_authorized_request(
        &self,
        candidate_id: &str,
        request: reqwest::RequestBuilder,
        client_version: Option<&str>,
    ) -> std::result::Result<reqwest::Response, AuthorizedRequestError> {
        let first_request = request
            .try_clone()
            .ok_or(AuthorizedRequestError::NotReplayable)?;
        let prepared = self
            .prepare_authorization(candidate_id, runtime_now_ms())
            .await
            .map_err(AuthorizedRequestError::Prepare)?;
        let response = apply_prepared_authorization(first_request, &prepared, client_version)?
            .send()
            .await
            .map_err(AuthorizedRequestError::Transport)?;
        if response.status() == StatusCode::UNAUTHORIZED {
            if let Some(task_id) = prepared.agent_task_id.as_deref() {
                let (response, invalid_task) =
                    inspect_agent_identity_unauthorized(response).await?;
                if !invalid_task {
                    self.observe_codex_quota_headers(
                        candidate_id,
                        response.status(),
                        response.headers(),
                        runtime_now_ms(),
                    );
                    return Ok(response);
                }
                drop(response);
                let refreshed = self
                    .refresh_agent_identity_task_after_unauthorized(
                        candidate_id,
                        task_id,
                        runtime_now_ms(),
                    )
                    .await
                    .map_err(AuthorizedRequestError::Prepare)?;
                let response = apply_prepared_authorization(request, &refreshed, client_version)?
                    .send()
                    .await
                    .map_err(AuthorizedRequestError::Transport)?;
                self.observe_codex_quota_headers(
                    candidate_id,
                    response.status(),
                    response.headers(),
                    runtime_now_ms(),
                );
                return Ok(response);
            }
        }
        if response.status() != StatusCode::UNAUTHORIZED || prepared.token_generation.is_none() {
            self.observe_codex_quota_headers(
                candidate_id,
                response.status(),
                response.headers(),
                runtime_now_ms(),
            );
            return Ok(response);
        }

        drop(response);
        let _fence = self.fence_execution(candidate_id);
        let refreshed = self
            .refresh_authorization_after_unauthorized(
                candidate_id,
                prepared.token_generation,
                runtime_now_ms(),
            )
            .await
            .map_err(AuthorizedRequestError::Prepare)?;
        let response = apply_prepared_authorization(request, &refreshed, client_version)?
            .send()
            .await
            .map_err(AuthorizedRequestError::Transport)?;
        self.observe_codex_quota_headers(
            candidate_id,
            response.status(),
            response.headers(),
            runtime_now_ms(),
        );
        Ok(response)
    }

    pub(crate) fn fence_execution(&self, candidate_id: &str) -> Option<ExecutionFence> {
        self.lock_scheduler()
            .set_execution_fence(candidate_id, true)
            .then(|| ExecutionFence {
                scheduler: self.scheduler.clone(),
                candidate_id: candidate_id.to_string(),
                released: AtomicBool::new(false),
            })
    }

    pub(crate) fn block_candidate_capability(&self, candidate_id: &str, model: &str) -> bool {
        self.lock_scheduler().block_capability(candidate_id, model)
    }

    pub(crate) fn clear_candidate_capability_blocks(&self, candidate_id: &str) -> bool {
        self.lock_scheduler().clear_capability_blocks(candidate_id)
    }

    pub(crate) fn record_provider_rate_limit(
        &self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
    ) -> bool {
        self.lock_scheduler()
            .record_provider_rate_limit(candidate_id, model, now_ms)
    }

    pub(crate) fn observe_codex_quota_headers(
        &self,
        candidate_id: &str,
        status: StatusCode,
        headers: &HeaderMap,
        observed_at_ms: u64,
    ) -> bool {
        if !(status.is_success()
            || status == StatusCode::SWITCHING_PROTOCOLS
            || status == StatusCode::TOO_MANY_REQUESTS)
            || !self.chatgpt_accounts.contains_key(candidate_id)
        {
            return false;
        }
        let mut quotas = self
            .passive_quotas
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = quotas.get_mut(candidate_id) else {
            return false;
        };
        let Some(merged) = crate::providers::chatgpt::merge_codex_quota_headers(
            &state.snapshot,
            headers,
            observed_at_ms,
        ) else {
            return false;
        };
        if merged == state.snapshot {
            return false;
        }
        let previous_quota = CandidateQuota::from_snapshot(
            &state.snapshot,
            observed_at_ms,
            self.quota_stale_after_ms,
        );
        let quota =
            CandidateQuota::from_snapshot(&merged, observed_at_ms, self.quota_stale_after_ms);
        state.force_persist |= previous_quota != quota
            && matches!(
                (previous_quota, quota),
                (CandidateQuota::Exhausted, _) | (_, CandidateQuota::Exhausted)
            );
        state.snapshot = merged;
        state.dirty = true;
        self.lock_scheduler().update_candidate_quota_at(
            candidate_id,
            quota,
            state.snapshot.updated_at_ms,
            state.snapshot.limiting_reset_at_ms(),
        )
    }

    pub(crate) fn take_passive_quota_snapshot(
        &self,
        candidate_id: &str,
        now_ms: u64,
    ) -> Option<QuotaSnapshot> {
        let mut quotas = self
            .passive_quotas
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = quotas.get_mut(candidate_id)?;
        if !state.dirty
            || (!state.force_persist
                && now_ms.saturating_sub(state.last_persist_hint_ms)
                    < PASSIVE_QUOTA_PERSIST_DEBOUNCE_MS)
        {
            return None;
        }
        state.dirty = false;
        state.force_persist = false;
        state.last_persist_hint_ms = now_ms;
        Some(state.snapshot.clone())
    }

    pub(crate) fn apply_usage_event(&self, event: &UsageEvent, observed_at_ms: u64) {
        let Some(candidate_id) = event.candidate_id.as_deref() else {
            return;
        };
        if let Some(snapshot) = event.quota_snapshot.as_ref() {
            self.lock_scheduler().update_candidate_quota_at(
                candidate_id,
                CandidateQuota::from_snapshot(snapshot, observed_at_ms, self.quota_stale_after_ms),
                snapshot.updated_at_ms,
                snapshot.limiting_reset_at_ms(),
            );
        }
        if event.success {
            self.set_candidate_health(candidate_id, CandidateHealth::Healthy);
            return;
        }

        let category = event.error_category.as_deref().unwrap_or_default();
        let model = if category == "image_generation_not_enabled" {
            event.requested_model.as_deref()
        } else {
            event
                .resolved_model
                .as_deref()
                .or(event.requested_model.as_deref())
        }
        .unwrap_or("*");
        if is_model_capability_failure(category) {
            self.block_candidate_capability(candidate_id, model);
            return;
        }
        if event.account_id.is_none() {
            if event.http_status == StatusCode::TOO_MANY_REQUESTS.as_u16() {
                self.record_provider_rate_limit(candidate_id, model, observed_at_ms);
            }
            return;
        }

        match category {
            "upstream_quota_exhausted" => {
                let reset_at_ms = {
                    let mut quotas = self
                        .passive_quotas
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    quotas.get_mut(candidate_id).and_then(|state| {
                        state.snapshot.limit_reached = true;
                        state.snapshot.updated_at_ms = Some(observed_at_ms);
                        state.snapshot.error = None;
                        state.dirty = true;
                        state.force_persist = true;
                        state.snapshot.limiting_reset_at_ms()
                    })
                };
                self.lock_scheduler().update_candidate_quota_at(
                    candidate_id,
                    CandidateQuota::Exhausted,
                    Some(observed_at_ms),
                    reset_at_ms,
                );
            }
            "upstream_unauthorized" | "account_auth" => {
                self.set_candidate_health(candidate_id, CandidateHealth::ReauthRequired);
            }
            "upstream_account_disabled" => {
                self.set_candidate_health(candidate_id, CandidateHealth::Blocked);
            }
            "upstream_account_verification_required" => {
                self.set_candidate_health(candidate_id, CandidateHealth::Checkpoint);
            }
            _ => {}
        }
    }

    pub(crate) fn request_client(
        &self,
        candidate_id: &str,
        upstream_stream: bool,
    ) -> &reqwest::Client {
        if let Some(account) = self.chatgpt_accounts.get(candidate_id) {
            return if upstream_stream {
                &account.client
            } else {
                &account.bounded_client
            };
        }
        if upstream_stream {
            &self.client
        } else {
            &self.bounded_client
        }
    }

    pub(crate) fn websocket_client(&self, candidate_id: &str) -> &reqwest::Client {
        self.chatgpt_accounts
            .get(candidate_id)
            .map(|account| &account.websocket_client)
            .unwrap_or(&self.websocket_client)
    }

    pub(crate) fn max_retry_candidates(&self) -> usize {
        self.max_retry_candidates
    }

    pub(crate) fn source_recovery_delay_ms(&self, candidate_id: &str) -> Option<u64> {
        self.source_recovery_delays_ms.get(candidate_id).copied()
    }

    pub fn set_default_service_tier(&self, tier: DefaultServiceTier) {
        self.default_service_tier_fast
            .store(tier == DefaultServiceTier::Fast, Ordering::Relaxed);
    }

    pub(crate) fn default_service_tier(&self) -> DefaultServiceTier {
        if self.default_service_tier_fast.load(Ordering::Relaxed) {
            DefaultServiceTier::Fast
        } else {
            DefaultServiceTier::Standard
        }
    }

    pub(crate) fn response_affinity_key(&self, response_id: Option<&str>) -> Option<String> {
        let response_id = response_id?.trim();
        if response_id.is_empty() {
            return None;
        }
        Some(hex::encode(Sha256::digest(
            format!("response\0{response_id}").as_bytes(),
        )))
    }

    pub(crate) fn prompt_affinity_key(
        &self,
        local_key_id: &str,
        model: &str,
        prompt_cache_key: Option<&str>,
    ) -> Option<String> {
        let prompt_cache_key = prompt_cache_key?.trim();
        if prompt_cache_key.is_empty() {
            return None;
        }
        Some(hex::encode(Sha256::digest(
            format!(
                "prompt\0{}\0{}\0{}",
                local_key_id,
                model.to_ascii_lowercase(),
                prompt_cache_key
            )
            .as_bytes(),
        )))
    }

    pub(crate) fn bind_prompt_affinity(&self, key: Option<&str>, candidate_id: &str, now_ms: u64) {
        if let Some(key) = key {
            self.lock_scheduler()
                .bind_prompt_affinity(key, candidate_id, now_ms);
        }
    }

    pub(crate) fn bind_response_affinity(
        &self,
        response_id: Option<&str>,
        candidate_id: &str,
        now_ms: u64,
    ) {
        if let Some(key) = self.response_affinity_key(response_id) {
            if self
                .lock_scheduler()
                .bind_response_affinity(key.clone(), candidate_id, now_ms)
            {
                self.persist_response_affinity(&key, candidate_id, now_ms);
            }
        }
    }

    pub(crate) fn invalidate_response_affinity(&self, key: Option<&str>) -> bool {
        key.is_some_and(|key| {
            let invalidated = self.lock_scheduler().invalidate_response_affinity(key);
            if invalidated {
                if let Some(store) = self.response_affinity_store.as_ref() {
                    let _ = store.delete(key);
                }
            }
            invalidated
        })
    }

    fn persist_response_affinity(&self, key: &str, candidate_id: &str, now_ms: u64) {
        if let Some(store) = self.response_affinity_store.as_ref() {
            let _ = store.upsert(&ResponseAffinityBinding {
                key: key.to_string(),
                candidate_id: candidate_id.to_string(),
                expires_at_ms: now_ms.saturating_add(RESPONSE_AFFINITY_TTL_MS),
            });
        }
    }

    pub(crate) fn record_success_with_metrics(
        &self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
        output_tokens: Option<u64>,
        latency_ms: u64,
    ) -> bool {
        self.lock_scheduler().record_success_with_metrics(
            candidate_id,
            model,
            now_ms,
            output_tokens,
            latency_ms,
        )
    }

    pub(crate) fn record_failure(&self, candidate_id: &str) -> u32 {
        self.lock_scheduler()
            .record_failure(candidate_id)
            .unwrap_or(1)
    }

    pub(crate) fn set_cooldown(&self, candidate_id: &str, model: &str, retry_at_ms: u64) -> bool {
        self.lock_scheduler()
            .set_cooldown(candidate_id, model, retry_at_ms)
    }

    fn lock_scheduler(&self) -> MutexGuard<'_, PoolScheduler> {
        self.scheduler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ChatGptAccountExecutor {
    async fn ensure_agent_identity_task(
        &self,
        expected_task_id: Option<&str>,
    ) -> std::result::Result<AgentIdentityCredential, ExecutorPrepareError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(ExecutorPrepareError::Authentication);
        }
        let current = self
            .agent_identity
            .read()
            .map_err(|_| ExecutorPrepareError::Transient)?
            .clone()
            .ok_or(ExecutorPrepareError::Authentication)?;
        if current.task_id().is_some()
            && expected_task_id.is_none_or(|expected| current.task_id() != Some(expected))
        {
            return Ok(current);
        }

        let _guard = self.agent_task_lock.lock().await;
        let current = self
            .agent_identity
            .read()
            .map_err(|_| ExecutorPrepareError::Transient)?
            .clone()
            .ok_or(ExecutorPrepareError::Authentication)?;
        if current.task_id().is_some()
            && expected_task_id.is_none_or(|expected| current.task_id() != Some(expected))
        {
            return Ok(current);
        }
        let task_id = current
            .register_task(&self.bounded_client)
            .await
            .map_err(classify_agent_identity_error)?;
        let task_id = self
            .persistence_adapter
            .persist_agent_task_id(&self.id, current.task_id(), &task_id)
            .await
            .map_err(|_| ExecutorPrepareError::Persistence)?;
        let updated = current
            .with_task_id(task_id)
            .map_err(|_| ExecutorPrepareError::InvalidCredential)?;
        if !self.active.load(Ordering::Acquire) {
            return Err(ExecutorPrepareError::Authentication);
        }
        *self
            .agent_identity
            .write()
            .map_err(|_| ExecutorPrepareError::Transient)? = Some(updated.clone());
        Ok(updated)
    }
}

fn classify_agent_identity_error(error: AgentIdentityError) -> ExecutorPrepareError {
    match error {
        AgentIdentityError::RegistrationTransport => ExecutorPrepareError::Transient,
        AgentIdentityError::RegistrationRejected => ExecutorPrepareError::Authentication,
        _ => ExecutorPrepareError::InvalidCredential,
    }
}

async fn inspect_agent_identity_unauthorized(
    response: reqwest::Response,
) -> std::result::Result<(reqwest::Response, bool), AuthorizedRequestError> {
    let status = response.status();
    let version = response.version();
    let headers = response.headers().clone();
    let body = response
        .bytes()
        .await
        .map_err(AuthorizedRequestError::Transport)?;
    let invalid = is_agent_identity_task_invalid_response(status.as_u16(), &body);
    let mut restored = axum::http::Response::builder()
        .status(status)
        .version(version)
        .body(reqwest::Body::from(body))
        .map_err(|_| AuthorizedRequestError::NotReplayable)?;
    *restored.headers_mut() = headers;
    Ok((reqwest::Response::from(restored), invalid))
}

fn apply_prepared_authorization(
    request: reqwest::RequestBuilder,
    prepared: &PreparedAuthorization,
    client_version: Option<&str>,
) -> std::result::Result<reqwest::RequestBuilder, AuthorizedRequestError> {
    let request = request.header(AUTHORIZATION, prepared.authorization.clone());
    let Some(identity) = prepared.identity.as_ref() else {
        return Ok(request);
    };
    let identity = match client_version {
        Some(version) => identity
            .with_client_version(version)
            .map_err(|_| AuthorizedRequestError::NotReplayable)?,
        None => identity.clone(),
    };
    Ok(identity.apply(request))
}

fn is_model_capability_failure(category: &str) -> bool {
    matches!(
        category,
        "upstream_model_not_found"
            | "upstream_model_unsupported"
            | "upstream_usage_not_included"
            | "image_generation_not_enabled"
    )
}

fn runtime_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
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

impl ChatGptAccountExecutor {
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
                &self.chatgpt_accounts.keys().collect::<Vec<_>>(),
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

pub fn normalize_image_base_model(value: Option<String>) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    if value.len() > 256 || value.chars().any(char::is_control) {
        return Err(Error::Validation(
            "image base model id is invalid".to_string(),
        ));
    }
    Ok(Some(value.to_string()))
}

fn select_image_main_model(models: &BTreeSet<String>, preferred: Option<&str>) -> Option<String> {
    match preferred
        .map(str::trim)
        .filter(|model| !model.is_empty() && !model.eq_ignore_ascii_case("auto"))
    {
        Some(preferred) => models
            .iter()
            .find(|model| {
                model.eq_ignore_ascii_case(preferred)
                    && !model.eq_ignore_ascii_case(IMAGE_API_MODEL)
            })
            .cloned(),
        None => cheapest_image_main_model(models),
    }
}

fn cheapest_image_main_model(models: &BTreeSet<String>) -> Option<String> {
    models
        .iter()
        .filter(|model| image_auto_model_is_supported(model))
        .min_by(|left, right| compare_image_main_models(left, right))
        .cloned()
}

fn image_main_model_is_compatible(model: &str) -> bool {
    let lower = model.trim().to_ascii_lowercase();
    !lower.is_empty()
        && lower != IMAGE_API_MODEL
        && [
            "image",
            "embedding",
            "moderation",
            "realtime",
            "transcribe",
            "tts",
            "audio",
        ]
        .iter()
        .all(|excluded| !lower.contains(excluded))
}

fn image_auto_model_is_supported(model: &str) -> bool {
    // OpenAI's image-generation guide currently requires GPT-5 or newer for the Responses tool.
    let lower = model.trim().to_ascii_lowercase();
    let Some(version) = lower.strip_prefix("gpt-") else {
        return false;
    };
    let major = version
        .split(|character: char| !character.is_ascii_digit())
        .next()
        .and_then(|value| value.parse::<u32>().ok());
    major.is_some_and(|major| major >= 5)
        && image_main_model_is_compatible(model)
        && api_model_price(model).is_some()
}

fn compare_image_main_models(left: &str, right: &str) -> CmpOrdering {
    match (api_model_price(left), api_model_price(right)) {
        (Some(left_price), Some(right_price)) => left_price
            .input_micro_usd_per_million
            .cmp(&right_price.input_micro_usd_per_million)
            .then_with(|| {
                left_price
                    .output_micro_usd_per_million
                    .cmp(&right_price.output_micro_usd_per_million)
            })
            .then_with(|| left_price.catalog_rank.cmp(&right_price.catalog_rank)),
        (Some(_), None) => CmpOrdering::Less,
        (None, Some(_)) => CmpOrdering::Greater,
        (None, None) => CmpOrdering::Equal,
    }
    .then_with(|| left.len().cmp(&right.len()))
    .then_with(|| left.cmp(right))
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
    fn runtime_updates_service_tier_and_removes_candidates_in_place() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source-1",
                "upstream-secret",
                &["gpt-test"],
            ))],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();

        assert_eq!(runtime.default_service_tier(), DefaultServiceTier::Standard);
        runtime.set_default_service_tier(DefaultServiceTier::Fast);
        assert_eq!(runtime.default_service_tier(), DefaultServiceTier::Fast);
        assert!(runtime.remove_candidate("source-1"));
        assert!(runtime.candidate_runtime_order().is_empty());
    }

    #[test]
    fn image_main_model_prefers_cheapest_tier_without_model_name_allowlist() {
        let models = normalized_set(
            [
                "gpt-5.6-terra".to_string(),
                "gpt-5.6-sol".to_string(),
                "gpt-5.4-mini".to_string(),
            ]
            .iter()
            .collect::<Vec<_>>(),
        );
        assert_eq!(
            cheapest_image_main_model(&models).as_deref(),
            Some("gpt-5.4-mini")
        );
        let terra = normalized_set(["gpt-5.6-terra".to_string()].iter());
        assert_eq!(
            cheapest_image_main_model(&terra).as_deref(),
            Some("gpt-5.6-terra")
        );
        let image = normalized_set([IMAGE_API_MODEL.to_string()].iter());
        assert!(cheapest_image_main_model(&image).is_none());
    }

    #[test]
    fn explicit_image_base_model_is_used_only_when_available() {
        let models = normalized_set(
            ["gpt-5.4-mini".to_string(), "gpt-5.6-sol".to_string()]
                .iter()
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            select_image_main_model(&models, Some("gpt-5.6-sol")).as_deref(),
            Some("gpt-5.6-sol")
        );
        assert_eq!(select_image_main_model(&models, Some("future-model")), None);
        let future = normalized_set(["gpt-future".to_string()].iter());
        assert!(cheapest_image_main_model(&future).is_none());
        assert_eq!(
            select_image_main_model(&future, Some("gpt-future")).as_deref(),
            Some("gpt-future")
        );
        let legacy = normalized_set(["gpt-4.1-mini".to_string()].iter());
        assert!(cheapest_image_main_model(&legacy).is_none());
        assert_eq!(
            normalize_image_base_model(Some(" auto ".into())).unwrap(),
            None
        );
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
    fn codex_aliases_resolve_without_shadowing_exact_model_ids() {
        let encoded = crate::codex_model_alias("vendor/model");
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source",
                "upstream-secret",
                &["vendor/model", &encoded],
            ))],
            vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let authenticated = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();

        assert_eq!(
            runtime
                .resolve_visible_model(
                    &authenticated,
                    &encoded,
                    &[WireApi::Responses],
                    current_time_ms(),
                )
                .as_deref(),
            Some(encoded.as_str())
        );

        let alias = crate::codex_model_alias("vendor/model");
        let without_collision = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source",
                "upstream-secret",
                &["vendor/model"],
            ))],
            vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let authenticated = without_collision
            .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
            .unwrap();
        assert_eq!(
            without_collision
                .resolve_visible_model(
                    &authenticated,
                    &alias,
                    &[WireApi::Responses],
                    current_time_ms(),
                )
                .as_deref(),
            Some("vendor/model")
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
