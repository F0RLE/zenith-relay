use crate::accounts::{
    AccountAuthState, TokenAuthority, TokenAuthorityError, TokenPersistenceAdapter,
    TokenRefreshAdapter,
};
use crate::catalog::{
    apply_claude_reasoning_capability_fallback, normalize_model_reasoning_allowed_levels,
    source_context_windows, source_image_input_capabilities, source_reasoning_capabilities,
    union_source_reasoning_capabilities, SourceReasoningCapabilities,
};
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::{
    is_agent_identity_task_invalid_response, AgentIdentityCredential, AgentIdentityError,
    CodexIdentityEnvelope, RuntimeChatGptAccount, RuntimeChatGptAuth,
};
use crate::quota::QuotaSnapshot;
use crate::scheduler::{CooldownReason, CooldownRequest};
use crate::ProxyConfig;
use crate::{
    api_model_price, decode_codex_model_alias, normalize_source_protocol_bindings,
    normalize_subscription_plan_order, CandidateHealth, CandidateKind, CandidateQuota,
    CandidateScope, Error, LocalGatewayKey, MessagesReasoningMode, ModelRegistry, ModelRules,
    NativeResponsesReplayState, NativeResponsesReplayStore, PoolScheduler, ProviderSource, Result,
    RoutingDiagnostics, RoutingStrategy, RuntimeCandidate, Selection, SelectionRequest,
    SourceAdapter, SourceConnector, SourceProtocolBinding, SourceProtocolBindingKey, UsageCallback,
    UsageEvent, WireApi, RESPONSE_AFFINITY_TTL_MS,
};
use futures_util::{future::join_all, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::cmp::Ordering as CmpOrdering;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
const SOURCE_MODEL_METADATA_PREFETCH_INTERVAL_MS: u64 = 15 * 1_000;

#[derive(Default)]
pub(crate) struct CodexSourceModelMetadata {
    pub context_windows: BTreeMap<String, u64>,
    pub reasoning_catalog_templates: BTreeMap<String, Map<String, Value>>,
    pub image_models: BTreeSet<String>,
}

fn source_candidate_id(
    source_id: &str,
    binding: &SourceProtocolBinding,
    binding_count: usize,
) -> String {
    if binding_count == 1 {
        return source_id.to_string();
    }
    let suffix = binding.adapter.route_suffix(binding.wire_api);
    format!("{source_id}::{suffix}")
}

fn source_reasoning_for_route(
    mut capabilities: SourceReasoningCapabilities,
    adapter: SourceAdapter,
    reasoning_mode: MessagesReasoningMode,
) -> Option<SourceReasoningCapabilities> {
    if adapter.is_passthrough() {
        return Some(capabilities);
    }
    capabilities
        .retain_efforts(|effort| reasoning_mode.supports_effort(effort))
        .then_some(())?;
    capabilities.clear_summary_capabilities();
    Some(capabilities)
}

fn confirmed_source_reasoning_levels(
    efforts_by_model: &BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    previous_levels: &BTreeMap<String, Vec<String>>,
    preferred_levels: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Vec<String>> {
    let mut levels_by_model = BTreeMap::new();
    for (model, routes) in efforts_by_model {
        let supported = routes
            .values()
            .flat_map(|efforts| efforts.iter().cloned())
            .collect::<BTreeSet<_>>();
        if supported.is_empty() {
            continue;
        }
        let mut ordered = Vec::new();
        for levels in [preferred_levels.get(model), previous_levels.get(model)] {
            let Some(levels) = levels else {
                continue;
            };
            for effort in levels {
                if supported.contains(effort) && !ordered.contains(effort) {
                    ordered.push(effort.clone());
                }
            }
        }
        for effort in supported {
            if !ordered.contains(&effort) {
                ordered.push(effort);
            }
        }
        levels_by_model.insert(model.clone(), ordered);
    }
    levels_by_model
}

#[derive(Clone, Debug)]
pub struct RuntimeSource {
    pub source: ProviderSource,
    /// Client-facing contracts and explicit adapters, scoped to models
    /// verified for each route. An empty list preserves the legacy source-wide
    /// `wire_api`.
    pub protocol_bindings: Vec<SourceProtocolBinding>,
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
            protocol_bindings: vec![SourceProtocolBinding::legacy(
                source.wire_api,
                &source.models,
            )],
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

/// Mutable routing policy for an already configured candidate.
///
/// It deliberately excludes connection details and the configured model
/// routes. Those require a new runtime because executors are immutable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeCandidatePolicy {
    pub enabled: bool,
    pub draining: bool,
    pub priority: i32,
    pub weight: u32,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeSourcePolicyUpdate {
    pub source_id: String,
    pub policy: RuntimeCandidatePolicy,
    pub recovery_delay_seconds: u64,
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
    pub cooldown_after_failures: u8,
    pub keep_last_candidate_available: bool,
    pub routing_strategy: RoutingStrategy,
    pub subscription_plan_order: Vec<String>,
    pub hidden_models: Vec<String>,
    pub default_service_tier: DefaultServiceTier,
    pub quota_stale_after_ms: u64,
    /// Optional text model used as the Responses image-generation bridge.
    /// `None` selects the cheapest known compatible model per account.
    pub image_base_model: Option<String>,
    /// Optional source-model allow-lists for reasoning efforts. An absent
    /// model keeps every effort that its sources have confirmed.
    pub model_reasoning_allowed_levels: BTreeMap<String, Vec<String>>,
    pub response_affinity_store: Option<Arc<dyn ResponseAffinityStore>>,
    pub provider_storm_breaker: bool,
}

impl fmt::Debug for GatewayRuntimeOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayRuntimeOptions")
            .field("max_retry_candidates", &self.max_retry_candidates)
            .field("cooldown_after_failures", &self.cooldown_after_failures)
            .field(
                "keep_last_candidate_available",
                &self.keep_last_candidate_available,
            )
            .field("routing_strategy", &self.routing_strategy)
            .field("subscription_plan_order", &self.subscription_plan_order)
            .field("hidden_models", &self.hidden_models)
            .field("default_service_tier", &self.default_service_tier)
            .field("quota_stale_after_ms", &self.quota_stale_after_ms)
            .field("image_base_model", &self.image_base_model)
            .field(
                "model_reasoning_allowed_levels",
                &self.model_reasoning_allowed_levels,
            )
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
            cooldown_after_failures: crate::DEFAULT_COOLDOWN_AFTER_FAILURES,
            keep_last_candidate_available: crate::DEFAULT_KEEP_LAST_CANDIDATE_AVAILABLE,
            routing_strategy: RoutingStrategy::Adaptive,
            subscription_plan_order: Vec::new(),
            hidden_models: Vec::new(),
            default_service_tier: DefaultServiceTier::Standard,
            quota_stale_after_ms: crate::QUOTA_STALE_AFTER_MS,
            image_base_model: None,
            model_reasoning_allowed_levels: BTreeMap::new(),
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
    sources: BTreeMap<String, SourceConnector>,
    source_candidate_bindings: BTreeMap<String, SourceCandidateBinding>,
    source_endpoint_domains: BTreeMap<String, String>,
    source_recovery_delays_ms: Mutex<BTreeMap<String, u64>>,
    chatgpt_accounts: BTreeMap<String, ChatGptAccountExecutor>,
    keys: Vec<RuntimeKey>,
    scheduler: Arc<Mutex<PoolScheduler>>,
    candidate_availability: Arc<tokio::sync::Notify>,
    registry: ModelRegistry,
    codex_responses_lite_models: Mutex<BTreeSet<String>>,
    model_metadata: SourceModelMetadataState,
    model_reasoning_allowed_levels: Mutex<BTreeMap<String, Vec<String>>>,
    passive_quotas: Mutex<BTreeMap<String, PassiveQuotaState>>,
    messages_bridge_store: Mutex<crate::MessagesBridgeStore>,
    native_responses_replay_store: Mutex<NativeResponsesReplayStore>,
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
struct CachedModelManifest {
    value: Value,
    observed_at_ms: u64,
}

struct SourceModelMetadataState {
    /// Native Codex catalog rows returned by `/models?client_version=...`.
    codex_manifests: Mutex<BTreeMap<String, CachedModelManifest>>,
    /// Generic source `/models` rows used only for source-declared
    /// capabilities such as context and reasoning. This stays separate from
    /// the Codex catalog because providers can return different payloads.
    source_manifests: Mutex<BTreeMap<String, CachedModelManifest>>,
    /// Serializes generic discovery and throttles best-effort prefetches.
    refresh_lock: tokio::sync::Mutex<()>,
    prefetch_pending: AtomicBool,
    prefetch_not_before_ms: AtomicU64,
    /// Confirmed source route -> effort support. Native account metadata is
    /// intentionally kept out of this state.
    confirmed_reasoning_efforts: Mutex<BTreeMap<String, BTreeMap<String, BTreeSet<String>>>>,
    confirmed_reasoning_levels: Mutex<BTreeMap<String, Vec<String>>>,
}

impl Default for SourceModelMetadataState {
    fn default() -> Self {
        Self {
            codex_manifests: Mutex::new(BTreeMap::new()),
            source_manifests: Mutex::new(BTreeMap::new()),
            refresh_lock: tokio::sync::Mutex::new(()),
            prefetch_pending: AtomicBool::new(false),
            prefetch_not_before_ms: AtomicU64::new(0),
            confirmed_reasoning_efforts: Mutex::new(BTreeMap::new()),
            confirmed_reasoning_levels: Mutex::new(BTreeMap::new()),
        }
    }
}

struct SourceModelMetadataPrefetchGuard {
    runtime: Arc<GatewayRuntime>,
}

impl Drop for SourceModelMetadataPrefetchGuard {
    fn drop(&mut self) {
        self.runtime
            .model_metadata
            .prefetch_pending
            .store(false, Ordering::Release);
    }
}

pub(crate) struct CandidateLease {
    scheduler: Arc<Mutex<PoolScheduler>>,
    availability: Arc<tokio::sync::Notify>,
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
        let released = match self.lane {
            CandidateLeaseLane::Text => self
                .scheduler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .release_for(&self.candidate_id, Some(&self.model)),
            CandidateLeaseLane::Image => self
                .scheduler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .release_image_for(&self.candidate_id, Some(&self.model)),
        };
        if released {
            self.availability.notify_one();
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
    scope: Arc<RwLock<CandidateScope>>,
    pub(crate) model_rules: ModelRules,
    pub(crate) model_prefix: Option<String>,
    client_wire_apis: Option<Vec<ClientWireApi>>,
}

impl AuthenticatedKey {
    pub(crate) fn scope_snapshot(&self) -> CandidateScope {
        self.scope
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn scope_read(&self) -> std::sync::RwLockReadGuard<'_, CandidateScope> {
        self.scope
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
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
    pub(crate) scope: CandidateScope,
    pub(crate) allowed_protocols: Vec<WireApi>,
    pub(crate) wire_api: WireApi,
    pub(crate) adapter: SourceAdapter,
    pub(crate) reasoning_mode: MessagesReasoningMode,
    pub(crate) service_tier: DefaultServiceTier,
    pub(crate) upstream_url: Url,
    pub(crate) upstream_headers: HeaderMap,
    pub(crate) source_model: String,
    pub(crate) half_open_probe: bool,
    pub(crate) routing: Option<RoutingDiagnostics>,
}

#[derive(Clone, Debug)]
struct SourceCandidateBinding {
    source_id: String,
    binding_key: SourceProtocolBindingKey,
    wire_api: WireApi,
    adapter: SourceAdapter,
    reasoning_mode: MessagesReasoningMode,
}

pub(crate) struct PreparedAuthorization {
    pub(crate) header_name: HeaderName,
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
    scope: Arc<RwLock<CandidateScope>>,
    model_rules: ModelRules,
    model_prefix: Option<String>,
    client_wire_apis: Option<Vec<ClientWireApi>>,
}

#[derive(Clone, Copy)]
enum ReachabilityRequirement {
    RequireReachable,
    AllowUnroutable,
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
            ReachabilityRequirement::RequireReachable,
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
        Self::build(
            sources,
            accounts,
            keys,
            Some(account_auth),
            ReachabilityRequirement::RequireReachable,
            options,
            usage,
        )
    }

    /// Builds a locally managed gateway while its persisted configuration is
    /// temporarily unroutable.
    ///
    /// Desktop source and pool mutations can validly remove the final
    /// candidate. In that state the gateway must keep authenticating its
    /// gateway credentials and return an empty catalog instead of preventing the
    /// mutation from committing. Authentication, catalog visibility, and
    /// per-request candidate selection remain unchanged; callers validating a
    /// new configuration must use [`Self::from_mixed_pool`] instead.
    pub fn from_mixed_pool_allow_unroutable(
        sources: Vec<RuntimeSource>,
        accounts: Vec<RuntimeChatGptAccount>,
        keys: Vec<RuntimeMixedLocalKey>,
        account_auth: RuntimeChatGptAuth,
        options: GatewayRuntimeOptions,
        usage: UsageCallback,
    ) -> Result<Self> {
        Self::build(
            sources,
            accounts,
            keys,
            Some(account_auth),
            ReachabilityRequirement::AllowUnroutable,
            options,
            usage,
        )
    }

    fn build(
        sources: Vec<RuntimeSource>,
        accounts: Vec<RuntimeChatGptAccount>,
        keys: Vec<RuntimeMixedLocalKey>,
        account_auth: Option<RuntimeChatGptAuth>,
        reachability_requirement: ReachabilityRequirement,
        options: GatewayRuntimeOptions,
        usage: UsageCallback,
    ) -> Result<Self> {
        if !(1..=8).contains(&options.max_retry_candidates) {
            return Err(Error::Validation(
                "max retry candidates must be between 1 and 8".to_string(),
            ));
        }
        if !(1..=8).contains(&options.cooldown_after_failures) {
            return Err(Error::Validation(
                "cooldown after failures must be between 1 and 8".to_string(),
            ));
        }
        let model_reasoning_allowed_levels = normalize_model_reasoning_allowed_levels(
            options.model_reasoning_allowed_levels.clone(),
        )
        .map_err(|message| Error::Validation(message.to_string()))?;

        let client = runtime_client(None, false)?;
        let bounded_client = runtime_client(None, true)?;
        let websocket_client = runtime_websocket_client(None)?;
        let discovery_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        let mut scheduler = PoolScheduler::new();
        scheduler.set_cooldown_policy(
            options.cooldown_after_failures,
            options.keep_last_candidate_available,
        );
        scheduler.set_routing_strategy(options.routing_strategy);
        let subscription_plan_order =
            normalize_subscription_plan_order(options.subscription_plan_order.clone())
                .map_err(|message| Error::Validation(message.to_string()))?;
        scheduler.set_subscription_plan_order(&subscription_plan_order);
        scheduler.set_quota_stale_after_ms(options.quota_stale_after_ms);
        scheduler.set_provider_storm_breaker_enabled(options.provider_storm_breaker);
        let mut registry = ModelRegistry::default();
        let mut source_executors = BTreeMap::new();
        let mut source_candidate_bindings = BTreeMap::new();
        let mut source_endpoint_domains = BTreeMap::new();
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
            let bindings = normalize_source_protocol_bindings(
                source.protocol_bindings.clone(),
                source.source.wire_api,
                &source.source.models,
            )?;
            let source_endpoint_domain =
                crate::sources::normalized_base_url(&source.source.base_url)?.to_string();
            let source_id = source.source.id.clone();
            let connector = SourceConnector::new(&source.source, &bindings)?;
            let rules = model_rules(&source.allowed_models, &source.excluded_models);
            for binding in &bindings {
                let models = normalized_set(binding.model_ids.iter());
                if models.is_empty() {
                    continue;
                }
                let candidate_id = source_candidate_id(&source_id, binding, bindings.len());
                if source_candidate_bindings.contains_key(&candidate_id) {
                    return Err(Error::Validation(
                        "source protocol candidate ids must be unique".to_string(),
                    ));
                }
                source_endpoint_domains
                    .insert(candidate_id.clone(), source_endpoint_domain.clone());
                let candidate = RuntimeCandidate {
                    id: candidate_id.clone(),
                    kind: CandidateKind::ApiSource,
                    source_id: source_id.clone(),
                    account_id: None,
                    protocol: binding.wire_api,
                    enabled: source.enabled,
                    draining: source.draining,
                    priority: source.priority,
                    weight: source.weight,
                    models: models.clone(),
                    model_rules: rules.clone(),
                    health: CandidateHealth::Healthy,
                    quota: CandidateQuota::Unknown,
                    quota_updated_at_ms: None,
                    quota_reset_at_ms: None,
                    cooldowns: BTreeMap::new(),
                    last_used_at: source.last_used_at_ms,
                    consecutive_failures: 0,
                    secret_available: true,
                };
                registry.replace(candidate_id.clone(), binding.model_ids.iter());
                scheduler.upsert(candidate);
                if source.recovery_delay_seconds > 0 {
                    source_recovery_delays_ms.insert(
                        candidate_id.clone(),
                        source.recovery_delay_seconds.saturating_mul(1_000),
                    );
                }
                source_candidate_bindings.insert(
                    candidate_id,
                    SourceCandidateBinding {
                        source_id: source_id.clone(),
                        binding_key: binding.key(),
                        wire_api: binding.wire_api,
                        adapter: binding.adapter,
                        reasoning_mode: binding.reasoning_mode,
                    },
                );
            }
            source_executors.insert(source_id, connector);
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
                || source_candidate_bindings.contains_key(&account.id)
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
            let mut published_models = account.models.clone();
            let models = normalized_set(account.models.iter());
            let image_main_model = select_image_main_model(&models, image_base_model.as_deref());
            let mut candidate_models = models.clone();
            if image_main_model.is_some() {
                candidate_models.insert(IMAGE_API_MODEL.to_string());
                published_models.push(IMAGE_API_MODEL.to_string());
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
                model_rules: model_rules(&account.allowed_models, &account.excluded_models),
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
            registry.replace(candidate.id.clone(), published_models.iter());
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
                    "gateway credential ids must be unique".to_string(),
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
            let mut model_rules = base_model_rules.clone();
            model_rules.excluded.extend(hidden_models.iter().cloned());
            let client_wire_apis = key.wire_apis.map(|values| {
                values
                    .into_iter()
                    .map(normalize_client_wire_api)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            });
            if client_wire_apis.as_ref().is_some_and(Vec::is_empty) {
                return Err(Error::Validation(
                    "gateway credential protocol scope must not be empty".to_string(),
                ));
            }
            configured_key_rules.push((
                key.enabled,
                scope.clone(),
                base_model_rules.clone(),
                client_wire_apis.clone(),
            ));
            runtime_keys.push(RuntimeKey {
                id: key.key.id,
                enabled: key.enabled,
                secret_hash: Sha256::digest(key.key.secret.as_bytes()).into(),
                scope: Arc::new(RwLock::new(scope)),
                model_rules,
                model_prefix: normalize_prefix(key.model_prefix),
                client_wire_apis,
            });
        }

        if matches!(
            reachability_requirement,
            ReachabilityRequirement::RequireReachable
        ) {
            if source_executors.is_empty() && account_executors.is_empty() {
                return Err(Error::Validation(
                    "at least one provider source or OAuth account is required".to_string(),
                ));
            }
            if !runtime_keys.iter().any(|key| key.enabled) {
                return Err(Error::Validation(
                    "at least one enabled gateway credential is required".to_string(),
                ));
            }
            let has_usable_key = configured_key_rules
                .iter()
                .filter(|(enabled, _, _, _)| *enabled)
                .any(|(_, scope, model_rules, client_wire_apis)| {
                    let allowed_protocols = client_wire_apis
                        .as_deref()
                        .map_or_else(all_native_wire_apis, client_wire_apis_to_native);
                    scheduler.candidates().any(|candidate| {
                        candidate.models.iter().any(|model| {
                            model_rules.allows(model)
                                && candidate.is_configured(model, &allowed_protocols, scope)
                        })
                    })
                });
            if !has_usable_key {
                return Err(Error::Validation(
                    "no enabled gateway credential can reach a configured source candidate"
                        .to_string(),
                ));
            }
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
            source_candidate_bindings,
            source_endpoint_domains,
            source_recovery_delays_ms: Mutex::new(source_recovery_delays_ms),
            chatgpt_accounts: account_executors,
            keys: runtime_keys,
            scheduler: Arc::new(Mutex::new(scheduler)),
            candidate_availability: Arc::new(tokio::sync::Notify::new()),
            registry,
            codex_responses_lite_models: Mutex::new(BTreeSet::new()),
            model_metadata: SourceModelMetadataState::default(),
            model_reasoning_allowed_levels: Mutex::new(model_reasoning_allowed_levels),
            passive_quotas: Mutex::new(passive_quotas),
            messages_bridge_store: Mutex::new(crate::MessagesBridgeStore::default()),
            native_responses_replay_store: Mutex::new(NativeResponsesReplayStore::default()),
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
        discover_with(&self.discovery_client, source, source.protocol_bindings()).await
    }

    pub(crate) fn authenticate(
        &self,
        authorization: Option<&HeaderValue>,
    ) -> Option<AuthenticatedKey> {
        let secret = authorization
            .and_then(|value| value.to_str().ok())
            .and_then(parse_bearer)?;
        self.authenticate_secret(secret)
    }

    pub(crate) fn authenticate_secret(&self, secret: &str) -> Option<AuthenticatedKey> {
        if secret.is_empty() || secret.len() > 4_096 {
            return None;
        }
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
        let scope = key.scope_snapshot();
        let scheduler = self.lock_scheduler();
        let models = self
            .registry
            .visible_models(&scheduler, &scope, allowed_protocols, now_ms)
            .into_iter()
            .filter(|model| key.model_rules.allows(model))
            .collect::<Vec<_>>();
        crate::canonicalize_model_ids(models)
            .into_iter()
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
        let scope = key.scope_snapshot();
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
                                    &scope,
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

    /// Resolves source-provided model metadata that is safe to expose in the
    /// generated Codex catalog.
    ///
    /// Metadata is evaluated per eligible candidate route. A public model may
    /// have several source candidates behind it, so the catalog exposes the
    /// union of efforts confirmed by at least one source. When a client asks
    /// for one explicitly, request routing excludes sources that have not
    /// confirmed that effort.
    pub(crate) async fn codex_source_model_metadata(
        &self,
        key: &AuthenticatedKey,
        allowed_protocols: &[WireApi],
        now_ms: u64,
    ) -> CodexSourceModelMetadata {
        let scope = key.scope_snapshot();
        self.source_model_metadata(&key.model_rules, &scope, allowed_protocols, now_ms)
            .await
    }

    /// Starts a best-effort metadata refresh for the management UI. The result
    /// arrives on the next state poll and never delays the current one.
    pub fn prefetch_source_model_metadata(self: &Arc<Self>) {
        if runtime_now_ms()
            < self
                .model_metadata
                .prefetch_not_before_ms
                .load(Ordering::Acquire)
        {
            return;
        }
        if self
            .model_metadata
            .prefetch_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let pending_guard = SourceModelMetadataPrefetchGuard {
            runtime: Arc::clone(self),
        };
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let _pending_guard = pending_guard;
            let rules = ModelRules::default();
            let scope = runtime.management_source_metadata_scope();
            runtime
                .source_model_metadata(&rules, &scope, &[WireApi::Responses], runtime_now_ms())
                .await;
            runtime.model_metadata.prefetch_not_before_ms.store(
                runtime_now_ms().saturating_add(SOURCE_MODEL_METADATA_PREFETCH_INTERVAL_MS),
                Ordering::Release,
            );
        });
    }

    /// Builds the source portion of the scope represented by active gateway
    /// credentials. Management must not advertise an effort discovered on an
    /// API source that no active pool key can reach.
    fn management_source_metadata_scope(&self) -> CandidateScope {
        let mut source_ids = BTreeSet::new();
        for key in self.keys.iter().filter(|key| key.enabled) {
            let scope = key
                .scope
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if scope.source_ids.is_none() && scope.account_ids.is_none() {
                return CandidateScope::default();
            }
            if let Some(ids) = scope.source_ids.as_ref() {
                source_ids.extend(ids.iter().cloned());
            }
        }
        CandidateScope {
            source_ids: Some(source_ids),
            account_ids: Some(BTreeSet::new()),
            model_rules: ModelRules::default(),
        }
    }

    async fn source_model_metadata(
        &self,
        model_rules: &ModelRules,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
        now_ms: u64,
    ) -> CodexSourceModelMetadata {
        let _refresh_guard = self.model_metadata.refresh_lock.lock().await;
        let routes = {
            let scheduler = self.lock_scheduler();
            let mut routes = Vec::new();
            for (candidate_id, binding) in &self.source_candidate_bindings {
                if binding.wire_api != WireApi::Responses {
                    continue;
                }
                let Some(candidate) = scheduler.candidate(candidate_id) else {
                    continue;
                };
                let Some(source) = self.sources.get(&binding.source_id) else {
                    continue;
                };
                let Some(models) = source.models_for(binding.binding_key) else {
                    continue;
                };
                let mut configured_models = BTreeSet::new();
                for model in models {
                    if model_rules.allows(model)
                        && candidate.is_catalog_visible(model, allowed_protocols, scope)
                    {
                        configured_models.insert(model.clone());
                    }
                }
                if configured_models.is_empty() {
                    continue;
                }
                let Some(source_binding) = source.binding_for(binding.binding_key) else {
                    continue;
                };
                routes.push((
                    candidate_id.clone(),
                    source.models_url.clone(),
                    source.authorization_for_binding(source_binding),
                    source.protocol_headers_for_binding(source_binding),
                    configured_models,
                    binding.adapter,
                    binding.reasoning_mode,
                ));
            }
            routes
        };
        let manifests = join_all(routes.into_iter().map(
            |(
                candidate_id,
                models_url,
                (authorization_name, authorization),
                protocol_headers,
                configured_models,
                adapter,
                reasoning_mode,
            )| async move {
                let cached_manifest = self.cached_source_model_manifest(&candidate_id);
                let manifest = if cached_manifest.as_ref().is_some_and(|manifest| {
                    now_ms.saturating_sub(manifest.observed_at_ms)
                        <= CODEX_SOURCE_MODEL_MANIFEST_TTL_MS
                }) {
                    cached_manifest.map(|manifest| manifest.value)
                } else {
                    let fetched_manifest = async {
                        let response = self
                            .discovery_client
                            .get(models_url)
                            .headers(protocol_headers)
                            .header(authorization_name, authorization)
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
                        serde_json::from_slice::<Value>(&body).ok()
                    }
                    .await;
                    if let Some(value) = fetched_manifest {
                        self.remember_source_model_manifest(&candidate_id, value.clone(), now_ms);
                        Some(value)
                    } else {
                        // A transient discovery failure must not make a model
                        // appear to lose its previously confirmed capabilities.
                        // Candidate removal and configured model filters still
                        // take effect before this cache is considered.
                        cached_manifest.map(|manifest| manifest.value)
                    }
                };
                (
                    candidate_id,
                    configured_models,
                    adapter,
                    reasoning_mode,
                    manifest,
                )
            },
        ))
        .await;

        let mut metadata = CodexSourceModelMetadata::default();
        let mut reasoning_by_model = BTreeMap::<String, Vec<SourceReasoningCapabilities>>::new();
        let mut evaluated_reasoning = Vec::<(String, String, BTreeSet<String>)>::new();
        let mut image_support_by_model = BTreeMap::<String, Vec<bool>>::new();
        for (candidate_id, configured_models, adapter, reasoning_mode, manifest) in manifests {
            let reasoning = manifest
                .as_ref()
                .map(|manifest| source_reasoning_capabilities(manifest, &configured_models))
                .unwrap_or_default();
            let declared_image_support = manifest
                .as_ref()
                .map(|manifest| source_image_input_capabilities(manifest, &configured_models))
                .unwrap_or_default();
            if let Some(manifest) = manifest.as_ref() {
                for (model, context_window) in source_context_windows(manifest, &configured_models)
                {
                    metadata
                        .context_windows
                        .entry(model)
                        .and_modify(|existing| *existing = (*existing).min(context_window))
                        .or_insert(context_window);
                }
            }
            for model in &configured_models {
                let model_key = model.to_ascii_lowercase();
                let supports_image = matches!(adapter, SourceAdapter::ResponsesToMessages)
                    || declared_image_support
                        .get(&model_key)
                        .copied()
                        .unwrap_or(false);
                image_support_by_model
                    .entry(model_key.clone())
                    .or_default()
                    .push(supports_image);
                let capabilities = apply_claude_reasoning_capability_fallback(
                    model,
                    reasoning.get(&model_key).cloned(),
                )
                .and_then(|capabilities| {
                    source_reasoning_for_route(capabilities, adapter, reasoning_mode)
                });
                if manifest.is_some() {
                    evaluated_reasoning.push((
                        model_key.clone(),
                        candidate_id.clone(),
                        capabilities
                            .as_ref()
                            .map(|capabilities| {
                                capabilities
                                    .effort_ids()
                                    .map(str::to_ascii_lowercase)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    ));
                }
                if let Some(capabilities) = capabilities {
                    reasoning_by_model
                        .entry(model_key)
                        .or_default()
                        .push(capabilities);
                }
            }
        }
        let mut current_reasoning_levels = BTreeMap::new();
        for (model, capabilities) in reasoning_by_model {
            let Some(capabilities) = union_source_reasoning_capabilities(capabilities) else {
                continue;
            };
            current_reasoning_levels.insert(
                model.clone(),
                capabilities
                    .effort_ids()
                    .map(str::to_ascii_lowercase)
                    .collect(),
            );
            metadata
                .reasoning_catalog_templates
                .insert(model, capabilities.codex_catalog_template());
        }
        let previous_levels = self
            .model_metadata
            .confirmed_reasoning_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let confirmed_levels = {
            let mut confirmed_efforts = self
                .model_metadata
                .confirmed_reasoning_efforts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (model, candidate_id, efforts) in evaluated_reasoning {
                if efforts.is_empty() {
                    let remove_model = confirmed_efforts.get_mut(&model).is_some_and(|routes| {
                        routes.remove(&candidate_id);
                        routes.is_empty()
                    });
                    if remove_model {
                        confirmed_efforts.remove(&model);
                    }
                } else {
                    confirmed_efforts
                        .entry(model)
                        .or_default()
                        .insert(candidate_id, efforts);
                }
            }
            confirmed_source_reasoning_levels(
                &confirmed_efforts,
                &previous_levels,
                &current_reasoning_levels,
            )
        };
        *self
            .model_metadata
            .confirmed_reasoning_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = confirmed_levels;
        for (model, route_support) in image_support_by_model {
            if route_support.iter().all(|supports_image| *supports_image) {
                metadata.image_models.insert(model);
            }
        }
        metadata
    }

    pub(crate) fn set_codex_model_uses_responses_lite(&self, model: &str, enabled: bool) {
        let mut models = self
            .codex_responses_lite_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if enabled {
            models.insert(model.to_ascii_lowercase());
        } else {
            models.remove(&model.to_ascii_lowercase());
        }
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
        self.model_metadata
            .codex_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                candidate_id.to_string(),
                CachedModelManifest {
                    value,
                    observed_at_ms,
                },
            );
    }

    fn remember_source_model_manifest(
        &self,
        candidate_id: &str,
        value: Value,
        observed_at_ms: u64,
    ) {
        let scheduler = self.lock_scheduler();
        if scheduler.candidate(candidate_id).is_none() {
            return;
        }
        self.model_metadata
            .source_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                candidate_id.to_string(),
                CachedModelManifest {
                    value,
                    observed_at_ms,
                },
            );
    }

    fn cached_source_model_manifest(&self, candidate_id: &str) -> Option<CachedModelManifest> {
        self.model_metadata
            .source_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(candidate_id)
            .cloned()
    }

    pub(crate) fn stale_codex_model_manifests<'a>(
        &self,
        candidate_ids: impl IntoIterator<Item = &'a str>,
    ) -> Vec<Value> {
        let manifests = self
            .model_metadata
            .codex_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        candidate_ids
            .into_iter()
            .filter_map(|candidate_id| manifests.get(candidate_id))
            .map(|manifest| manifest.value.clone())
            .collect()
    }

    pub(crate) fn visible_account_models(&self, key: &AuthenticatedKey) -> Vec<String> {
        let scope = key.scope_snapshot();
        let scheduler = self.lock_scheduler();
        let mut models = BTreeSet::new();
        for account in self.chatgpt_accounts.values() {
            let Some(candidate) = scheduler.candidate(&account.id) else {
                continue;
            };
            for model in &account.configured_models {
                if key.model_rules.allows(model)
                    && candidate.is_catalog_visible(model, &[WireApi::Responses], &scope)
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

    /// A bare Codex model keeps native metadata whenever a configured
    /// ChatGPT account can serve it.  Generic sources may expose the same
    /// upstream-looking id, but they must not downgrade the native account
    /// entry in the pool catalog.
    ///
    /// This deliberately checks configured routes rather than current health:
    /// a temporary catalogue-discovery failure must not erase native
    /// capabilities from a saved ChatGPT manifest.
    pub(crate) fn codex_model_has_chatgpt_account(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> bool {
        let Some(model) = self.resolve_model(key, model) else {
            return false;
        };
        let scope = key.scope_snapshot();
        let scheduler = self.lock_scheduler();
        self.chatgpt_accounts.values().any(|account| {
            account
                .configured_models
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&model))
                && scheduler.candidate(&account.id).is_some_and(|candidate| {
                    candidate.is_configured(&model, &[WireApi::Responses], &scope)
                })
        })
    }

    pub(crate) fn api_source_candidate_ids(&self) -> HashSet<String> {
        self.source_candidate_bindings.keys().cloned().collect()
    }

    /// Excludes API routes that have not explicitly confirmed support for an
    /// effort. Native ChatGPT routes are deliberately not part of this set:
    /// their request and catalog semantics remain provider-owned.
    pub(crate) fn exclude_api_sources_without_reasoning_effort(
        &self,
        model: &str,
        effort: &str,
        tried: &mut HashSet<String>,
    ) {
        let model = model.trim().to_ascii_lowercase();
        let effort = effort.trim().to_ascii_lowercase();
        if model.is_empty() || effort.is_empty() || effort == "none" {
            return;
        }
        let capabilities = self
            .model_metadata
            .confirmed_reasoning_efforts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A catalog refresh can be absent or temporarily fail. Until at least
        // one route has confirmed capabilities for this model, preserve the
        // normal transparent fallback rather than excluding every source.
        let Some(confirmed) = capabilities.get(&model) else {
            return;
        };
        for candidate_id in self.source_candidate_bindings.keys() {
            if !confirmed
                .get(candidate_id)
                .is_some_and(|efforts| efforts.contains(&effort))
            {
                tried.insert(candidate_id.clone());
            }
        }
    }

    pub fn confirmed_source_reasoning_levels(&self, model: &str) -> Vec<String> {
        self.model_metadata
            .confirmed_reasoning_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&model.trim().to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    /// Returns a model's manually allowed reasoning levels. An empty result
    /// means the catalog remains automatic and exposes every confirmed level.
    pub fn model_reasoning_allowed_levels(&self, model: &str) -> Vec<String> {
        self.model_reasoning_allowed_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&model.trim().to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }

    pub fn model_reasoning_effort_is_allowed(&self, model: &str, effort: &str) -> bool {
        let model = model.trim().to_ascii_lowercase();
        let effort = effort.trim().to_ascii_lowercase();
        self.model_reasoning_allowed_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&model)
            .is_none_or(|allowed| allowed.iter().any(|level| level == &effort))
    }

    pub fn set_model_reasoning_allowed_levels(
        &self,
        allowed_levels: BTreeMap<String, Vec<String>>,
    ) -> Result<()> {
        let allowed_levels = normalize_model_reasoning_allowed_levels(allowed_levels)
            .map_err(|message| Error::Validation(message.to_string()))?;
        *self
            .model_reasoning_allowed_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = allowed_levels;
        Ok(())
    }

    pub fn supports_source_reasoning_effort(&self, model: &str, effort: &str) -> bool {
        self.model_metadata
            .confirmed_reasoning_efforts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&model.trim().to_ascii_lowercase())
            .is_some_and(|routes| {
                routes
                    .values()
                    .any(|efforts| efforts.contains(&effort.trim().to_ascii_lowercase()))
            })
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

    /// Applies source routing rules without rebuilding its HTTP executor.
    ///
    /// A source can have more than one protocol binding, so every matching
    /// candidate must receive the same policy atomically from the scheduler's
    /// point of view.
    pub fn update_source_policy(
        &self,
        source_id: &str,
        policy: RuntimeCandidatePolicy,
        recovery_delay_seconds: u64,
    ) -> bool {
        self.update_source_policies(&[RuntimeSourcePolicyUpdate {
            source_id: source_id.to_string(),
            policy,
            recovery_delay_seconds,
        }])
    }

    /// Applies several source policies as one scheduler update. This keeps a
    /// reordered fallback group consistent even when its sources expose
    /// multiple protocol bindings.
    pub fn update_source_policies(&self, updates: &[RuntimeSourcePolicyUpdate]) -> bool {
        if updates
            .iter()
            .any(|update| update.policy.weight == 0 || update.recovery_delay_seconds > 24 * 60 * 60)
        {
            return false;
        }
        let mut seen = BTreeSet::new();
        if updates
            .iter()
            .any(|update| !seen.insert(update.source_id.as_str()))
        {
            return false;
        }

        let mut scheduler = self.lock_scheduler();
        let mut candidates = Vec::new();
        let mut recovery_updates = Vec::new();
        for update in updates {
            let rules = model_rules(
                &update.policy.allowed_models,
                &update.policy.excluded_models,
            );
            let mut matched = false;
            for (candidate_id, binding) in &self.source_candidate_bindings {
                if binding.source_id != update.source_id {
                    continue;
                }
                matched = true;
                let Some(mut candidate) = scheduler.candidate(candidate_id).cloned() else {
                    return false;
                };
                if candidate.kind != CandidateKind::ApiSource
                    || candidate.source_id != update.source_id
                {
                    return false;
                }
                apply_candidate_policy(&mut candidate, &update.policy, &rules);
                recovery_updates.push((candidate_id.clone(), update.recovery_delay_seconds));
                candidates.push(candidate);
            }
            if !matched {
                return false;
            }
        }
        for candidate in candidates {
            scheduler.upsert(candidate);
        }
        drop(scheduler);

        let mut recovery_delays = self
            .source_recovery_delays_ms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (candidate_id, recovery_delay_seconds) in recovery_updates {
            if recovery_delay_seconds == 0 {
                recovery_delays.remove(&candidate_id);
            } else {
                recovery_delays.insert(candidate_id, recovery_delay_seconds.saturating_mul(1_000));
            }
        }
        drop(recovery_delays);
        self.candidate_availability.notify_waiters();
        true
    }

    /// Applies an account's scheduling policy without replacing its OAuth
    /// executor or interrupting in-flight streams.
    pub fn update_account_policy(&self, account_id: &str, policy: RuntimeCandidatePolicy) -> bool {
        if policy.weight == 0 {
            return false;
        }
        let rules = model_rules(&policy.allowed_models, &policy.excluded_models);
        let mut scheduler = self.lock_scheduler();
        let Some(mut candidate) = scheduler.candidate(account_id).cloned() else {
            return false;
        };
        if candidate.kind != CandidateKind::OAuthAccount
            || candidate.account_id.as_deref() != Some(account_id)
        {
            return false;
        }
        apply_candidate_policy(&mut candidate, &policy, &rules);
        scheduler.upsert(candidate);
        drop(scheduler);
        self.candidate_availability.notify_waiters();
        true
    }

    pub fn update_key_scope(&self, key_id: &str, scope: CandidateScope) -> bool {
        let Some(key) = self.keys.iter().find(|key| key.enabled && key.id == key_id) else {
            return false;
        };
        let mut current = key
            .scope
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *current == scope {
            return true;
        }
        *current = scope;
        drop(current);
        self.candidate_availability.notify_waiters();
        true
    }

    pub fn set_candidate_health(&self, candidate_id: &str, health: CandidateHealth) -> bool {
        self.lock_scheduler()
            .set_candidate_health(candidate_id, health)
    }

    pub fn remove_candidate(&self, candidate_id: &str) -> bool {
        let removed = self.lock_scheduler().remove(candidate_id).is_some();
        self.model_metadata
            .codex_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(candidate_id);
        self.model_metadata
            .source_manifests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(candidate_id);
        let previous_levels = self
            .model_metadata
            .confirmed_reasoning_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let confirmed_levels = {
            let mut efforts = self
                .model_metadata
                .confirmed_reasoning_efforts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for routes in efforts.values_mut() {
                routes.remove(candidate_id);
            }
            efforts.retain(|_, routes| !routes.is_empty());
            confirmed_source_reasoning_levels(&efforts, &previous_levels, &BTreeMap::new())
        };
        *self
            .model_metadata
            .confirmed_reasoning_levels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = confirmed_levels;
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

    pub(crate) async fn select_and_reserve(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        affinity_keys: (Option<&str>, Option<&str>),
        now_ms: u64,
    ) -> Option<(Selection, CandidateLease)> {
        let (response_affinity_key, prompt_affinity_key) = affinity_keys;
        let mut selection_now_ms = now_ms;
        loop {
            // Register before evaluating candidates so a completed request cannot
            // release the only OAuth account between the failed selection and wait.
            let notified = self.candidate_availability.notified();
            let (reserved, wait_for_release) = self.try_select_and_reserve_for(
                key,
                model,
                allowed_protocols,
                tried,
                response_affinity_key,
                prompt_affinity_key,
                selection_now_ms,
                CandidateLeaseLane::Text,
            );
            if let Some(reserved) = reserved {
                return Some(reserved);
            }
            if !wait_for_release {
                return None;
            }
            notified.await;
            selection_now_ms = runtime_now_ms();
        }
    }

    pub(crate) fn select_and_reserve_image(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        now_ms: u64,
    ) -> Option<(Selection, CandidateLease)> {
        self.try_select_and_reserve_for(
            key,
            model,
            allowed_protocols,
            tried,
            None,
            None,
            now_ms,
            CandidateLeaseLane::Image,
        )
        .0
    }

    #[allow(clippy::too_many_arguments)]
    fn try_select_and_reserve_for(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        response_affinity_key: Option<&str>,
        prompt_affinity_key: Option<&str>,
        now_ms: u64,
        lane: CandidateLeaseLane,
    ) -> (Option<(Selection, CandidateLease)>, bool) {
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
        // Keep authorization live through selection and reservation. A pool
        // mutation waits for this read lock, so it cannot race a stale scope
        // into a newly reserved lease.
        let scope = key.scope_read();
        let mut scheduler = self.lock_scheduler();
        let selection = match lane {
            CandidateLeaseLane::Text => scheduler.select(SelectionRequest {
                model,
                allowed_protocols,
                scope: &scope,
                tried,
                response_affinity_key,
                prompt_affinity_key,
                now_ms,
            }),
            CandidateLeaseLane::Image => scheduler.select_image(SelectionRequest {
                model,
                allowed_protocols,
                scope: &scope,
                tried,
                response_affinity_key,
                prompt_affinity_key,
                now_ms,
            }),
        };
        let reserved = selection.and_then(|selection| {
            let reserved = match lane {
                CandidateLeaseLane::Text => {
                    scheduler.reserve_for(&selection.candidate_id, model, now_ms)
                }
                CandidateLeaseLane::Image => {
                    scheduler.reserve_image_for(&selection.candidate_id, model, now_ms)
                }
            };
            reserved.then(|| {
                let lease = CandidateLease {
                    scheduler: self.scheduler.clone(),
                    availability: self.candidate_availability.clone(),
                    candidate_id: selection.candidate_id.clone(),
                    model: model.to_string(),
                    lane,
                    released: AtomicBool::new(false),
                };
                (selection, lease)
            })
        });
        let wait_for_release = matches!(lane, CandidateLeaseLane::Text)
            && reserved.is_none()
            && scheduler.has_waitable_text_candidate(SelectionRequest {
                model,
                allowed_protocols,
                scope: &scope,
                tried,
                response_affinity_key,
                prompt_affinity_key,
                now_ms,
            });
        drop(scheduler);
        drop(scope);
        if let (Some((selection, _)), Some(key)) = (reserved.as_ref(), response_affinity_key) {
            if selection.response_affinity_hit {
                self.persist_response_affinity(key, &selection.candidate_id, now_ms);
            }
        }
        (reserved, wait_for_release)
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
        let scope = key.scope_snapshot();
        self.lock_scheduler().earliest_retry_at(SelectionRequest {
            model,
            allowed_protocols,
            scope: &scope,
            tried,
            response_affinity_key,
            prompt_affinity_key: None,
            now_ms,
        })
    }

    pub(crate) fn all_applicable_cooldown(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        response_affinity_key: Option<&str>,
        now_ms: u64,
    ) -> Option<(u64, CooldownReason)> {
        let scope = key.scope_snapshot();
        self.lock_scheduler()
            .all_applicable_cooldown(SelectionRequest {
                model,
                allowed_protocols,
                scope: &scope,
                tried,
                response_affinity_key,
                prompt_affinity_key: None,
                now_ms,
            })
    }

    pub(crate) fn executor_route(
        &self,
        candidate_id: &str,
        model: &str,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
    ) -> Option<ExecutorRoute> {
        if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
            let source = self.sources.get(&binding.source_id)?;
            let source_binding = source.binding_for(binding.binding_key)?;
            let source_model = source.canonical_model_for(binding.binding_key, model)?;
            return Some(ExecutorRoute {
                candidate_id: candidate_id.to_string(),
                source_id: binding.source_id.clone(),
                account_id: None,
                scope: scope.clone(),
                allowed_protocols: allowed_protocols.to_vec(),
                wire_api: binding.wire_api,
                adapter: binding.adapter,
                reasoning_mode: binding.reasoning_mode,
                service_tier: DefaultServiceTier::Standard,
                upstream_url: source.endpoint(binding.binding_key)?.clone(),
                upstream_headers: source.protocol_headers_for_binding(source_binding),
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
            scope: scope.clone(),
            allowed_protocols: allowed_protocols.to_vec(),
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            service_tier: DefaultServiceTier::Standard,
            upstream_url: account.responses_url.clone(),
            upstream_headers: HeaderMap::new(),
            source_model,
            half_open_probe: false,
            routing: None,
        })
    }

    pub(crate) fn image_executor_route(
        &self,
        candidate_id: &str,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
    ) -> Option<ExecutorRoute> {
        if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
            if !binding.adapter.is_passthrough() {
                return None;
            }
            let source = self.sources.get(&binding.source_id)?;
            let source_binding = source.binding_for(binding.binding_key)?;
            let source_model = source.canonical_model_for(binding.binding_key, IMAGE_API_MODEL)?;
            return Some(ExecutorRoute {
                candidate_id: candidate_id.to_string(),
                source_id: binding.source_id.clone(),
                account_id: None,
                scope: scope.clone(),
                allowed_protocols: allowed_protocols.to_vec(),
                wire_api: binding.wire_api,
                adapter: binding.adapter,
                reasoning_mode: binding.reasoning_mode,
                service_tier: DefaultServiceTier::Standard,
                upstream_url: source.endpoint(binding.binding_key)?.clone(),
                upstream_headers: source.protocol_headers_for_binding(source_binding),
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
            scope: scope.clone(),
            allowed_protocols: allowed_protocols.to_vec(),
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            service_tier: DefaultServiceTier::Standard,
            upstream_url: account.responses_url.clone(),
            upstream_headers: HeaderMap::new(),
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
        if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
            let source = self
                .sources
                .get(&binding.source_id)
                .ok_or(ExecutorPrepareError::Authentication)?;
            let source_binding = source
                .binding_for(binding.binding_key)
                .ok_or(ExecutorPrepareError::Authentication)?;
            let (header_name, authorization) = source.authorization_for_binding(source_binding);
            return Ok(PreparedAuthorization {
                header_name,
                authorization,
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
                        header_name: AUTHORIZATION,
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
            header_name: AUTHORIZATION,
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
        // A direct API source may advertise a model while its upstream is
        // being replaced or temporarily unable to serve it. The request path
        // already applies a model-scoped cooldown for that failure; turning it
        // into a permanent capability block makes every later retry look like
        // there is no route at all. Native account capabilities are stable
        // enough to retain the explicit block until their catalog is refreshed.
        if event.account_id.is_some() && is_model_capability_failure(category) {
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
        self.source_recovery_delays_ms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(candidate_id)
            .copied()
    }

    /// Keeps a retry from fanning an endpoint-wide failure out to every
    /// credential configured for that same source endpoint. Only the failed
    /// candidate receives a cooldown; another credential remains eligible for
    /// later requests in case the failure was credential-specific.
    pub(crate) fn exclude_same_source_endpoint(
        &self,
        candidate_id: &str,
        tried: &mut HashSet<String>,
    ) {
        let Some(endpoint) = self.source_endpoint_domains.get(candidate_id) else {
            return;
        };
        tried.extend(
            self.source_endpoint_domains
                .iter()
                .filter(|(_, candidate_endpoint)| *candidate_endpoint == endpoint)
                .map(|(candidate_id, _)| candidate_id.clone()),
        );
    }

    pub fn set_default_service_tier(&self, tier: DefaultServiceTier) {
        self.default_service_tier_fast
            .store(tier == DefaultServiceTier::Fast, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn default_service_tier(&self) -> DefaultServiceTier {
        if self.default_service_tier_fast.load(Ordering::Relaxed) {
            DefaultServiceTier::Fast
        } else {
            DefaultServiceTier::Standard
        }
    }

    pub(crate) fn load_messages_bridge_state(
        &self,
        local_key_id: &str,
        response_id: &str,
        candidate_id: &str,
        now_ms: u64,
    ) -> crate::AdapterResult<crate::MessagesBridgeState> {
        self.messages_bridge_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(local_key_id, response_id, candidate_id, now_ms)
    }

    pub(crate) fn save_messages_bridge_response(
        &self,
        local_key_id: &str,
        candidate_id: &str,
        response: &crate::MessagesBridgeResponse,
        now_ms: u64,
    ) {
        self.messages_bridge_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                local_key_id,
                &response.response_id,
                candidate_id,
                response.continuation.clone(),
                now_ms,
            );
    }

    pub(crate) fn load_native_responses_replay(
        &self,
        local_key_id: &str,
        response_id: &str,
        candidate_id: &str,
        now_ms: u64,
    ) -> Option<NativeResponsesReplayState> {
        self.native_responses_replay_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(local_key_id, response_id, candidate_id, now_ms)
    }

    pub(crate) fn save_native_responses_replay(
        &self,
        local_key_id: &str,
        candidate_id: &str,
        response_id: &str,
        state: NativeResponsesReplayState,
        now_ms: u64,
    ) {
        self.native_responses_replay_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(local_key_id, response_id, candidate_id, state, now_ms);
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

    pub(crate) fn set_cooldown_with_reason_for_model_at(
        &self,
        candidate_id: &str,
        request: CooldownRequest<'_>,
    ) -> bool {
        self.lock_scheduler()
            .set_cooldown_with_reason_for_model_at(candidate_id, request)
    }

    fn lock_scheduler(&self) -> MutexGuard<'_, PoolScheduler> {
        self.scheduler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn normalize_client_wire_api(wire_api: ClientWireApi) -> ClientWireApi {
    // Older key records could carry `images` as if it were an independent
    // client protocol. Image requests are authorized by the
    // Chat-Completions-compatible surface, so retain backward compatibility
    // without exposing a dead standalone scope.
    match wire_api {
        ClientWireApi::Images => ClientWireApi::ChatCompletions,
        other => other,
    }
}

fn all_native_wire_apis() -> Vec<WireApi> {
    vec![
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
    ]
}

fn client_wire_apis_to_native(client_wire_apis: &[ClientWireApi]) -> Vec<WireApi> {
    client_wire_apis
        .iter()
        .map(|wire_api| match wire_api {
            ClientWireApi::Responses => WireApi::Responses,
            ClientWireApi::ChatCompletions | ClientWireApi::Images => WireApi::ChatCompletions,
            ClientWireApi::Messages => WireApi::Messages,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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
    let request = request.header(prepared.header_name.clone(), prepared.authorization.clone());
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

pub async fn discover_source_models(source: &ProviderSource) -> Result<Vec<String>> {
    discover_source_models_for_protocol_bindings(source, &[]).await
}

/// Discovers a source catalog using each explicitly configured binding.
/// Authentication and the discovery endpoint follow the binding's upstream
/// protocol. No request body or response format is adapted during discovery.
pub async fn discover_source_models_for_protocol_bindings(
    source: &ProviderSource,
    protocol_bindings: &[SourceProtocolBinding],
) -> Result<Vec<String>> {
    discover_source_models_and_protocol_bindings(source, protocol_bindings)
        .await
        .map(|discovery| discovery.models)
}

/// The result of binding-aware model discovery.
///
/// `models` is the de-duplicated union in upstream response order. Each
/// `protocol_bindings` entry contains the models advertised for that binding
/// after its explicit allow-list is applied. A failed or empty binding is
/// omitted. A successful `/models` response is catalog evidence, not a
/// completion capability probe; operators must assign a model only to routes
/// the upstream documents or has safely verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDiscovery {
    pub models: Vec<String>,
    pub protocol_bindings: Vec<SourceProtocolBinding>,
}

/// Discovers models independently for every configured binding.
///
/// Providers sometimes expose different model catalogs (and even different
/// credentials) on their Responses, Chat Completions, and Messages endpoints.
/// Discovery must therefore never reuse the first successful response for the
/// remaining bindings. A configured non-empty `model_ids` list is a strict
/// allow-list for that binding; an empty list means use the catalog returned
/// under that binding's authentication. The function succeeds when at least
/// one binding has a non-empty catalog.
pub async fn discover_source_models_and_protocol_bindings(
    source: &ProviderSource,
    protocol_bindings: &[SourceProtocolBinding],
) -> Result<SourceDiscovery> {
    source.validate()?;
    let bindings = normalize_source_protocol_bindings(
        protocol_bindings.to_vec(),
        source.wire_api,
        &source.models,
    )?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    discover_protocol_bindings_with(
        &client,
        &SourceConnector::new(source, &bindings)?,
        &bindings,
        protocol_bindings,
    )
    .await
}

async fn discover_with(
    client: &reqwest::Client,
    source: &SourceConnector,
    bindings: &[SourceProtocolBinding],
) -> Result<Vec<String>> {
    // GatewayRuntime only retains normalized bindings, so it cannot tell a
    // legacy expanded model list from an explicit per-protocol allow-list.
    // Keep this compatibility helper broad; management paths use the public
    // discovery API above and retain that distinction.
    discover_protocol_bindings_with(client, source, bindings, &[])
        .await
        .map(|discovery| discovery.models)
}

async fn discover_protocol_bindings_with(
    client: &reqwest::Client,
    source: &SourceConnector,
    bindings: &[SourceProtocolBinding],
    configured_bindings: &[SourceProtocolBinding],
) -> Result<SourceDiscovery> {
    let mut last_error = None;
    let mut discovered_models = Vec::new();
    let mut discovered_model_keys = HashSet::new();
    let mut discovered_bindings = Vec::new();

    for binding in bindings {
        let (authorization_name, authorization) = source.authorization_for_binding(binding);
        let request = client
            .get(source.models_url.clone())
            .headers(source.protocol_headers_for_binding(binding))
            .header(authorization_name, authorization);
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(Error::Upstream(error));
                continue;
            }
        };
        if !response.status().is_success() {
            last_error = Some(Error::InvalidUpstreamResponse(
                "upstream model discovery failed",
            ));
            continue;
        }
        let body = match collect_limited(response, MAX_MODELS_BODY_BYTES).await {
            Ok(body) => body,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };
        let body: Value = match serde_json::from_slice(&body) {
            Ok(body) => body,
            Err(_) => {
                last_error = Some(Error::InvalidUpstreamResponse(
                    "upstream model response is invalid",
                ));
                continue;
            }
        };
        let Some(data) = body.get("data").and_then(Value::as_array) else {
            last_error = Some(Error::InvalidUpstreamResponse(
                "upstream model response is invalid",
            ));
            continue;
        };
        let mut seen = HashSet::new();
        let upstream_models = data
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .filter(|model| seen.insert(model.to_ascii_lowercase()))
            .map(str::to_string)
            .collect::<Vec<_>>();

        // An explicitly supplied model list is scoped to this protocol. The
        // normalized legacy binding may contain source.models after expansion,
        // so use the original configured binding to distinguish an allow-list
        // from the legacy "discover everything" form.
        let explicit_models = configured_bindings
            .iter()
            .find(|configured| configured.key() == binding.key())
            .and_then(|configured| {
                let models = configured
                    .model_ids
                    .iter()
                    .map(|model| model.trim().to_ascii_lowercase())
                    .filter(|model| !model.is_empty())
                    .collect::<HashSet<_>>();
                (!models.is_empty()).then_some(models)
            });
        let models = upstream_models
            .into_iter()
            .filter(|model| {
                explicit_models
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(&model.to_ascii_lowercase()))
            })
            .collect::<Vec<_>>();
        if models.is_empty() {
            continue;
        }

        for model in &models {
            if discovered_model_keys.insert(model.to_ascii_lowercase()) {
                discovered_models.push(model.clone());
            }
        }
        discovered_bindings.push(SourceProtocolBinding {
            wire_api: binding.wire_api,
            adapter: binding.adapter,
            reasoning_mode: binding.reasoning_mode,
            model_ids: models,
        });
    }

    if discovered_bindings.is_empty() {
        return Err(last_error.unwrap_or(Error::InvalidUpstreamResponse(
            "source did not expose any confirmed models",
        )));
    }
    Ok(SourceDiscovery {
        models: discovered_models,
        protocol_bindings: discovered_bindings,
    })
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

fn model_rules(allowed: &[String], excluded: &[String]) -> ModelRules {
    ModelRules {
        allowed: normalized_set(allowed.iter()),
        excluded: normalized_set(excluded.iter()),
    }
}

fn apply_candidate_policy(
    candidate: &mut RuntimeCandidate,
    policy: &RuntimeCandidatePolicy,
    rules: &ModelRules,
) {
    candidate.enabled = policy.enabled;
    candidate.draining = policy.draining;
    candidate.priority = policy.priority;
    candidate.weight = policy.weight;
    candidate.model_rules = rules.clone();
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

fn runtime_client_builder(proxy: Option<&ProxyConfig>) -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(MAX_IDLE_CONNECTIONS_PER_HOST)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_nodelay(true)
        .redirect(reqwest::redirect::Policy::none());
    match proxy {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
}

fn runtime_client(proxy: Option<&ProxyConfig>, bounded: bool) -> Result<reqwest::Client> {
    let builder = runtime_client_builder(proxy).http2_adaptive_window(true);
    let builder = if bounded {
        builder.timeout(Duration::from_secs(900))
    } else {
        builder.read_timeout(Duration::from_secs(300))
    };
    builder.build().map_err(Error::from)
}

fn runtime_websocket_client(proxy: Option<&ProxyConfig>) -> Result<reqwest::Client> {
    runtime_client_builder(proxy)
        .http1_only()
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
    use crate::ToolUseDiagnostics;

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
    fn runtime_updates_source_policy_without_rebuilding_candidate_state() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source-1",
                "upstream-secret",
                &["model-a", "model-b"],
            ))],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let retry_at = current_time_ms() + 60_000;
        runtime.set_candidate_cooldown("source-1", "model-a", retry_at);

        assert!(runtime.update_source_policy(
            "source-1",
            RuntimeCandidatePolicy {
                enabled: true,
                draining: false,
                priority: 7,
                weight: 3,
                allowed_models: vec!["model-b".into()],
                excluded_models: Vec::new(),
            },
            30,
        ));
        assert_eq!(
            runtime.visible_models_for_secret(
                "local-secret",
                &[WireApi::Responses],
                current_time_ms()
            ),
            vec!["model-b"]
        );
        let candidate = runtime
            .lock_scheduler()
            .candidate("source-1")
            .cloned()
            .unwrap();
        assert_eq!(candidate.priority, 7);
        assert_eq!(candidate.weight, 3);
        assert_eq!(candidate.cooldowns.get("model-a"), Some(&retry_at));
        assert_eq!(runtime.source_recovery_delay_ms("source-1"), Some(30_000));
    }

    #[test]
    fn runtime_rejects_policy_updates_for_missing_candidates() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source-1",
                "upstream-secret",
                &["model-a"],
            ))],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let policy = RuntimeCandidatePolicy {
            enabled: true,
            draining: false,
            priority: 7,
            weight: 3,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
        };

        assert!(!runtime.update_source_policies(&[
            RuntimeSourcePolicyUpdate {
                source_id: "source-1".into(),
                policy: policy.clone(),
                recovery_delay_seconds: 30,
            },
            RuntimeSourcePolicyUpdate {
                source_id: "missing".into(),
                policy: policy.clone(),
                recovery_delay_seconds: 30,
            },
        ]));
        assert_eq!(
            runtime
                .lock_scheduler()
                .candidate("source-1")
                .expect("source candidate")
                .priority,
            0
        );
        assert!(!runtime.update_account_policy("missing", policy));
    }

    #[test]
    fn runtime_updates_key_scope_without_rebuild() {
        let runtime = GatewayRuntime::from_pool(
            vec![
                RuntimeSource::unrestricted(source("source-a", "a", &["model-a"])),
                RuntimeSource::unrestricted(source("source-b", "b", &["model-b"])),
            ],
            vec![RuntimeLocalKey {
                key: key("key-1", "local-secret"),
                enabled: true,
                source_ids: Some(vec!["source-a".into()]),
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                model_prefix: None,
            }],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();

        assert_eq!(
            runtime.visible_models_for_secret(
                "local-secret",
                &[WireApi::Responses],
                current_time_ms()
            ),
            vec!["model-a"]
        );
        assert!(runtime.update_key_scope(
            "key-1",
            CandidateScope {
                source_ids: Some(std::iter::once("source-b".to_string()).collect()),
                account_ids: Some(Default::default()),
                model_rules: ModelRules::default(),
            },
        ));
        assert_eq!(
            runtime.visible_models_for_secret(
                "local-secret",
                &[WireApi::Responses],
                current_time_ms()
            ),
            vec!["model-b"]
        );
    }

    #[tokio::test]
    async fn source_capability_failure_does_not_permanently_hide_a_declared_model() {
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
        let authenticated = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let now_ms = current_time_ms();
        runtime.apply_usage_event(
            &UsageEvent {
                request_id: "request".into(),
                attempt: 1,
                local_key_id: "key-1".into(),
                source_id: "source-1".into(),
                candidate_id: Some("source-1".into()),
                account_id: None,
                routing: None,
                requested_model: Some("gpt-test".into()),
                resolved_model: Some("gpt-test".into()),
                wire_api: WireApi::Responses,
                service_tier: DefaultServiceTier::Standard,
                applied_service_tier: None,
                success: false,
                http_status: StatusCode::BAD_REQUEST.as_u16(),
                error_category: Some("upstream_model_not_found".into()),
                tool_use: ToolUseDiagnostics::default(),
                cooldown_scope: Some("gpt-test".into()),
                retry_at_ms: Some(now_ms.saturating_add(60_000)),
                consecutive_failures: Some(1),
                latency_ms: 1,
                ttft_ms: None,
                generation_ms: None,
                input_tokens: None,
                cached_input_tokens: None,
                cache_write_input_tokens: None,
                reasoning_tokens: None,
                output_tokens: None,
                total_tokens: None,
                quota_snapshot: None,
            },
            now_ms,
        );

        let selection = runtime
            .select_and_reserve(
                &authenticated,
                "gpt-test",
                &[WireApi::Responses],
                &HashSet::new(),
                (None, None),
                now_ms,
            )
            .await;
        assert!(selection.is_some());
    }

    #[test]
    fn source_connector_preserves_normalized_binding_and_model_order() {
        let source = source(
            "source-1",
            "upstream-secret",
            &[
                "gpt-5.6-sol",
                "claude-opus-5",
                "gpt-5.4-mini",
                "claude-sonnet-5",
            ],
        );
        let bindings = normalize_source_protocol_bindings(
            vec![
                SourceProtocolBinding {
                    wire_api: WireApi::Messages,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    model_ids: vec!["claude-opus-5".into(), "claude-sonnet-5".into()],
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    model_ids: vec!["gpt-5.6-sol".into(), "gpt-5.4-mini".into()],
                },
            ],
            source.wire_api,
            &source.models,
        )
        .unwrap();

        let connector = SourceConnector::new(&source, &bindings).unwrap();

        assert_eq!(connector.protocol_bindings(), bindings.as_slice());
        assert_eq!(
            connector
                .canonical_model_for(bindings[1].key(), "GPT-5.4-MINI")
                .as_deref(),
            Some("gpt-5.4-mini")
        );
        assert!(connector
            .protocol_bindings()
            .iter()
            .find(|binding| binding.wire_api == WireApi::ChatCompletions)
            .is_none());
    }

    #[tokio::test]
    async fn generic_source_reasoning_metadata_survives_a_codex_catalog_cache_update() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source-1",
                "upstream-secret",
                &["provider/fable"],
            ))],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let now_ms = current_time_ms();

        runtime.remember_source_model_manifest(
            "source-1",
            serde_json::json!({
                "data": [{
                    "id": "provider/fable",
                    "context_window": 1_000_000,
                    "reasoningEffortModes": ["low", "high"],
                    "defaultReasoningLevel": "high"
                }]
            }),
            now_ms,
        );
        // Codex asks the same source for a different payload shape. It must
        // not overwrite the generic source metadata that powers the selector.
        runtime.remember_codex_model_manifest(
            "source-1",
            serde_json::json!({"models": [{"slug": "provider/fable"}]}),
            now_ms,
        );

        let metadata = runtime
            .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
            .await;

        assert_eq!(
            metadata.context_windows,
            BTreeMap::from([("provider/fable".to_string(), 1_000_000)])
        );
        assert_eq!(
            metadata.reasoning_catalog_templates["provider/fable"],
            serde_json::json!({
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "low"},
                    {"effort": "high", "description": "high"}
                ]
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }

    #[tokio::test]
    async fn management_prefetch_populates_reasoning_before_codex_catalog_request() {
        let runtime = Arc::new(
            GatewayRuntime::from_pool(
                vec![RuntimeSource::unrestricted(source(
                    "source-1",
                    "upstream-secret",
                    &["provider/fable"],
                ))],
                vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
                GatewayRuntimeOptions::default(),
                Arc::new(|_| {}),
            )
            .unwrap(),
        );
        runtime.remember_source_model_manifest(
            "source-1",
            serde_json::json!({
                "data": [{
                    "id": "provider/fable",
                    "reasoningEffortModes": ["low", "medium", "high"]
                }]
            }),
            current_time_ms(),
        );

        runtime.prefetch_source_model_metadata();
        tokio::time::timeout(Duration::from_secs(1), async {
            while runtime
                .confirmed_source_reasoning_levels("provider/fable")
                .is_empty()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("management prefetch completes");

        assert_eq!(
            runtime.confirmed_source_reasoning_levels("provider/fable"),
            vec!["low".to_string(), "medium".to_string(), "high".to_string()]
        );
        let not_before = runtime
            .model_metadata
            .prefetch_not_before_ms
            .load(Ordering::Acquire);
        assert!(not_before > runtime_now_ms());
        runtime.prefetch_source_model_metadata();
        assert_eq!(
            runtime
                .model_metadata
                .prefetch_not_before_ms
                .load(Ordering::Acquire),
            not_before
        );
    }

    #[tokio::test]
    async fn management_prefetch_ignores_sources_outside_the_active_key_scope() {
        let runtime = Arc::new(
            GatewayRuntime::from_pool(
                vec![
                    RuntimeSource::unrestricted(source(
                        "source-in-pool",
                        "in-pool-secret",
                        &["provider/fable"],
                    )),
                    RuntimeSource::unrestricted(source(
                        "source-outside-pool",
                        "outside-pool-secret",
                        &["provider/fable"],
                    )),
                ],
                vec![RuntimeLocalKey {
                    key: key("key-1", "local-secret"),
                    enabled: true,
                    source_ids: Some(vec!["source-in-pool".into()]),
                    allowed_models: Vec::new(),
                    excluded_models: Vec::new(),
                    model_prefix: None,
                }],
                GatewayRuntimeOptions::default(),
                Arc::new(|_| {}),
            )
            .unwrap(),
        );
        let now_ms = current_time_ms();
        runtime.remember_source_model_manifest(
            "source-in-pool",
            serde_json::json!({
                "data": [{
                    "id": "provider/fable",
                    "reasoningEffortModes": ["low"]
                }]
            }),
            now_ms,
        );
        runtime.remember_source_model_manifest(
            "source-outside-pool",
            serde_json::json!({
                "data": [{
                    "id": "provider/fable",
                    "reasoningEffortModes": ["ultra"]
                }]
            }),
            now_ms,
        );

        runtime.prefetch_source_model_metadata();
        tokio::time::timeout(Duration::from_secs(1), async {
            while runtime
                .confirmed_source_reasoning_levels("provider/fable")
                .is_empty()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("management prefetch completes");

        assert_eq!(
            runtime.confirmed_source_reasoning_levels("provider/fable"),
            vec!["low".to_string()]
        );
    }

    #[tokio::test]
    async fn scoped_catalog_refresh_keeps_reasoning_confirmed_by_another_route() {
        let runtime = GatewayRuntime::from_pool(
            vec![
                RuntimeSource::unrestricted(source(
                    "source-a",
                    "source-a-secret",
                    &["provider/fable"],
                )),
                RuntimeSource::unrestricted(source(
                    "source-b",
                    "source-b-secret",
                    &["provider/fable"],
                )),
            ],
            vec![
                RuntimeLocalKey::unrestricted(key("key-all", "all-secret")),
                RuntimeLocalKey {
                    key: key("key-a", "source-a-only-secret"),
                    enabled: true,
                    source_ids: Some(vec!["source-a".to_string()]),
                    allowed_models: Vec::new(),
                    excluded_models: Vec::new(),
                    model_prefix: None,
                },
            ],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let all_key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer all-secret")))
            .unwrap();
        let source_a_key = runtime
            .authenticate(Some(&HeaderValue::from_static(
                "Bearer source-a-only-secret",
            )))
            .unwrap();
        let now_ms = current_time_ms();
        runtime.remember_source_model_manifest(
            "source-a",
            serde_json::json!({
                "data": [{
                    "id": "provider/fable",
                    "reasoningEffortModes": ["low"]
                }]
            }),
            now_ms,
        );
        runtime.remember_source_model_manifest(
            "source-b",
            serde_json::json!({
                "data": [{
                    "id": "provider/fable",
                    "reasoningEffortModes": ["ultra"]
                }]
            }),
            now_ms,
        );

        runtime
            .codex_source_model_metadata(&all_key, &[WireApi::Responses], now_ms)
            .await;
        assert_eq!(
            runtime.confirmed_source_reasoning_levels("provider/fable"),
            vec!["low".to_string(), "ultra".to_string()]
        );

        let scoped_metadata = runtime
            .codex_source_model_metadata(&source_a_key, &[WireApi::Responses], now_ms)
            .await;
        assert_eq!(
            scoped_metadata.reasoning_catalog_templates["provider/fable"]
                ["supported_reasoning_levels"],
            serde_json::json!([{"effort": "low", "description": "low"}])
        );
        assert_eq!(
            runtime.confirmed_source_reasoning_levels("provider/fable"),
            vec!["low".to_string(), "ultra".to_string()]
        );
    }

    #[tokio::test]
    async fn stale_source_metadata_survives_a_transient_models_failure() {
        let mut unavailable_source = source("source-1", "upstream-secret", &["provider/fable"]);
        unavailable_source.base_url = "http://127.0.0.1:1/v1".to_string();
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(unavailable_source)],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let now_ms = current_time_ms();
        runtime.remember_source_model_manifest(
            "source-1",
            serde_json::json!({
                "data": [{
                    "id": "provider/fable",
                    "reasoningEffortModes": ["low", "medium", "high"]
                }]
            }),
            now_ms.saturating_sub(CODEX_SOURCE_MODEL_MANIFEST_TTL_MS + 1),
        );

        let metadata = runtime
            .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
            .await;

        assert!(metadata
            .reasoning_catalog_templates
            .contains_key("provider/fable"));
        assert_eq!(
            runtime.confirmed_source_reasoning_levels("provider/fable"),
            vec!["low".to_string(), "medium".to_string(), "high".to_string()]
        );
    }

    #[tokio::test]
    async fn source_reasoning_union_keeps_confirmed_route_and_excludes_unknown_route() {
        let runtime = GatewayRuntime::from_pool(
            vec![
                RuntimeSource::unrestricted(source(
                    "source-confirmed",
                    "confirmed-secret",
                    &["provider/fable"],
                )),
                RuntimeSource::unrestricted(source(
                    "source-unknown",
                    "unknown-secret",
                    &["provider/fable"],
                )),
            ],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let now_ms = current_time_ms();
        let mut before_catalog = HashSet::new();
        runtime.exclude_api_sources_without_reasoning_effort(
            "provider/fable",
            "high",
            &mut before_catalog,
        );
        assert!(before_catalog.is_empty());
        runtime.remember_source_model_manifest(
            "source-confirmed",
            serde_json::json!({
                "data": [{
                    "id": "provider/fable",
                    "reasoningEffortModes": ["low", "high"]
                }]
            }),
            now_ms,
        );
        runtime.remember_source_model_manifest(
            "source-unknown",
            serde_json::json!({"data": [{"id": "provider/fable"}]}),
            now_ms,
        );

        let metadata = runtime
            .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
            .await;

        assert!(metadata
            .reasoning_catalog_templates
            .contains_key("provider/fable"));
        assert_eq!(
            runtime.confirmed_source_reasoning_levels("provider/fable"),
            vec!["low".to_string(), "high".to_string()]
        );
        runtime
            .set_model_reasoning_allowed_levels(BTreeMap::from([(
                "provider/fable".to_string(),
                vec!["high".to_string()],
            )]))
            .unwrap();
        assert!(runtime.model_reasoning_effort_is_allowed("provider/fable", "high"));
        assert!(!runtime.model_reasoning_effort_is_allowed("provider/fable", "low"));
        let mut excluded = HashSet::new();
        runtime.exclude_api_sources_without_reasoning_effort(
            "provider/fable",
            "high",
            &mut excluded,
        );
        assert!(!excluded.contains("source-confirmed"));
        assert!(excluded.contains("source-unknown"));
    }

    #[tokio::test]
    async fn non_claude_source_catalog_preserves_source_declared_efforts_and_uses_medium_auto_default(
    ) {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source-1",
                "upstream-secret",
                &["grok-4.5", "glm-5.2"],
            ))],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let now_ms = current_time_ms();

        runtime.remember_source_model_manifest(
            "source-1",
            serde_json::json!({
                "data": [
                    {
                        "id": "grok-4.5",
                        "reasoningEffortModes": [
                            "low", "medium", "high", "xhigh", "max", "very_high"
                        ],
                        "defaultReasoningLevel": "very_high"
                    },
                    {
                        "id": "glm-5.2",
                        "reasoningEffortModes": ["low", "medium", "high", "xhigh", "max"],
                        "defaultReasoningLevel": "max"
                    }
                ]
            }),
            now_ms,
        );

        let metadata = runtime
            .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
            .await;

        for (model_id, expected) in [
            (
                "grok-4.5",
                serde_json::json!({
                    "supported_reasoning_levels": [
                        {"effort": "low", "description": "low"},
                        {"effort": "medium", "description": "medium"},
                        {"effort": "high", "description": "high"},
                        {"effort": "xhigh", "description": "xhigh"},
                        {"effort": "max", "description": "max"},
                        {"effort": "very_high", "description": "very_high"}
                    ],
                    "default_reasoning_level": "medium"
                }),
            ),
            (
                "glm-5.2",
                serde_json::json!({
                    "supported_reasoning_levels": [
                        {"effort": "low", "description": "low"},
                        {"effort": "medium", "description": "medium"},
                        {"effort": "high", "description": "high"},
                        {"effort": "xhigh", "description": "xhigh"},
                        {"effort": "max", "description": "max"}
                    ],
                    "default_reasoning_level": "medium"
                }),
            ),
        ] {
            assert_eq!(
                metadata.reasoning_catalog_templates[model_id],
                expected.as_object().unwrap().clone()
            );
        }
    }

    #[tokio::test]
    async fn source_catalog_does_not_cross_model_reasoning_metadata() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source-1",
                "upstream-secret",
                &["grok-4.5", "glm-5.2"],
            ))],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let now_ms = current_time_ms();

        runtime.remember_source_model_manifest(
            "source-1",
            serde_json::json!({
                "data": [
                    {
                        "id": "grok-4.5",
                        "reasoningEffortModes": ["low", "very_high"],
                        "defaultReasoningLevel": "very_high"
                    },
                    {"id": "glm-5.2"}
                ]
            }),
            now_ms,
        );

        let metadata = runtime
            .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
            .await;

        assert!(metadata
            .reasoning_catalog_templates
            .contains_key("grok-4.5"));
        assert!(!metadata.reasoning_catalog_templates.contains_key("glm-5.2"));
    }

    #[tokio::test]
    async fn manual_claude_effort_levels_are_published_when_provider_metadata_is_sparse() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source-1",
                "upstream-secret",
                &["vendor/claude-fable-5"],
            ))],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let now_ms = current_time_ms();

        runtime.remember_source_model_manifest(
            "source-1",
            serde_json::json!({
                "data": [{"id": "vendor/claude-fable-5"}]
            }),
            now_ms,
        );

        let metadata = runtime
            .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
            .await;

        assert_eq!(
            metadata.reasoning_catalog_templates["vendor/claude-fable-5"],
            serde_json::json!({
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "Low"},
                    {"effort": "medium", "description": "Medium"},
                    {"effort": "high", "description": "High"},
                    {"effort": "xhigh", "description": "Extra high"},
                    {"effort": "max", "description": "Maximum"},
                    {"effort": "ultra", "description": "Ultra"}
                ],
                "default_reasoning_level": "medium"
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }

    #[tokio::test]
    async fn codex_source_metadata_marks_bridge_images_but_requires_native_declaration() {
        let mut bridged = RuntimeSource::unrestricted(source(
            "source-bridge",
            "bridge-secret",
            &["vendor/claude-fable-5"],
        ));
        bridged.protocol_bindings = vec![SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode: MessagesReasoningMode::Disabled,
            model_ids: vec!["vendor/claude-fable-5".into()],
        }];
        let mut native = RuntimeSource::unrestricted(source(
            "source-native",
            "native-secret",
            &["provider/text-only"],
        ));
        native.protocol_bindings = vec![SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            model_ids: vec!["provider/text-only".into()],
        }];
        let runtime = GatewayRuntime::from_pool(
            vec![bridged, native],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let now_ms = current_time_ms();

        runtime.remember_source_model_manifest(
            "source-bridge",
            serde_json::json!({"data": [{"id": "vendor/claude-fable-5"}]}),
            now_ms,
        );
        runtime.remember_source_model_manifest(
            "source-native",
            serde_json::json!({
                "data": [{"id": "provider/text-only", "input_modalities": ["text"]}]
            }),
            now_ms,
        );

        let metadata = runtime
            .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
            .await;

        assert!(metadata.image_models.contains("vendor/claude-fable-5"));
        assert!(!metadata.image_models.contains("provider/text-only"));
    }

    #[test]
    fn messages_bridge_hides_provider_efforts_it_cannot_translate() {
        let configured_models = ["provider/fable"]
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        let manifest = serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["low", "provider-defined"],
                "supportsReasoningSummaryParameter": true,
                "supportsReasoningSummaries": true,
                "defaultReasoningSummary": "detailed"
            }]
        });
        let capabilities = source_reasoning_capabilities(&manifest, &configured_models)
            .remove("provider/fable")
            .unwrap();

        for reasoning_mode in [
            MessagesReasoningMode::Budget,
            MessagesReasoningMode::Adaptive,
        ] {
            let template = source_reasoning_for_route(
                capabilities.clone(),
                SourceAdapter::ResponsesToMessages,
                reasoning_mode,
            )
            .unwrap()
            .codex_catalog_template();

            assert_eq!(
                template,
                serde_json::json!({
                    "supported_reasoning_levels": [
                        {"effort": "low", "description": "low"}
                    ]
                })
                .as_object()
                .unwrap()
                .clone()
            );
        }
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
        assert!(error.to_string().contains("no enabled gateway credential"));
    }

    #[test]
    fn transition_runtime_allows_an_explicitly_empty_scope() {
        let runtime = GatewayRuntime::build(
            vec![RuntimeSource::unrestricted(source(
                "source-a",
                "a",
                &["gpt-a"],
            ))],
            Vec::new(),
            vec![RuntimeMixedLocalKey {
                key: key("key", "secret"),
                enabled: true,
                source_ids: Some(Vec::new()),
                account_ids: None,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                model_prefix: None,
                wire_apis: None,
            }],
            None,
            ReachabilityRequirement::AllowUnroutable,
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();

        assert!(runtime
            .visible_models_for_secret("secret", &[WireApi::Responses], current_time_ms())
            .is_empty());
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
