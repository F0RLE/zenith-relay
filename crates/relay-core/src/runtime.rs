use crate::accounts::{
    AccountAuthState, TokenAuthority, TokenPersistenceAdapter, TokenRefreshAdapter,
};
use crate::catalog::{normalize_model_reasoning_allowed_levels, SourceReasoningCapabilities};
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::{
    AgentIdentityCredential, CodexIdentityEnvelope, RuntimeChatGptAccount, RuntimeChatGptAuth,
};
use crate::quota::QuotaSnapshot;
use crate::scheduler::{CooldownReason, CooldownRequest};
use crate::sources::{discover_models_with_client, is_loopback_url};
use crate::ProxyConfig;
use crate::{
    decode_codex_model_alias, CandidateHealth, CandidateQuota, CandidateScope, Error,
    LocalGatewayKey, MessagesReasoningMode, ModelRegistry, ModelRules, NativeResponsesReplayState,
    NativeResponsesReplayStore, PoolScheduler, ProviderSource, Result, RoutingDiagnostics,
    RoutingStrategy, RuntimeCandidate, Selection, SelectionRequest, SourceAdapter, SourceConnector,
    SourceProtocolBinding, SourceProtocolBindingKey, UsageCallback, UsageEvent, WireApi,
    RESPONSE_AFFINITY_TTL_MS,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;
use subtle::ConstantTimeEq;
use url::Url;

mod authorization;
mod build;
mod candidates;
mod images;
mod source_metadata;

use build::{
    build_accounts, build_keys, build_sources, configure_scheduler, validate_reachability,
    validate_runtime_options, ReachabilityRequirement,
};

#[cfg(test)]
use crate::{normalize_source_protocol_bindings, CandidateKind};
pub use images::normalize_image_base_model;
#[cfg(test)]
use images::{cheapest_image_main_model, select_image_main_model};

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

impl DefaultServiceTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Fast => "fast",
        }
    }

    /// Parses the durable service-tier spelling, including the legacy Codex
    /// `priority` alias for Relay's fast tier.
    pub fn from_storage_value(value: &str) -> Self {
        if value.eq_ignore_ascii_case("fast") || value.eq_ignore_ascii_case("priority") {
            Self::Fast
        } else {
            Self::Standard
        }
    }
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
    clients: RuntimeHttpClients,
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

#[derive(Default)]
struct ConfirmedSourceReasoning {
    efforts: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    levels: BTreeMap<String, Vec<String>>,
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
    /// Confirmed source route -> effort support and its derived model-level
    /// catalog. Native account metadata is intentionally kept out of this state.
    confirmed_reasoning: Mutex<ConfirmedSourceReasoning>,
}

impl Default for SourceModelMetadataState {
    fn default() -> Self {
        Self {
            codex_manifests: Mutex::new(BTreeMap::new()),
            source_manifests: Mutex::new(BTreeMap::new()),
            refresh_lock: tokio::sync::Mutex::new(()),
            prefetch_pending: AtomicBool::new(false),
            prefetch_not_before_ms: AtomicU64::new(0),
            confirmed_reasoning: Mutex::new(ConfirmedSourceReasoning::default()),
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
    clients: RuntimeHttpClients,
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

struct RuntimeHttpClients {
    streaming: reqwest::Client,
    bounded: reqwest::Client,
    websocket: reqwest::Client,
}

impl RuntimeHttpClients {
    fn new(proxy: Option<&ProxyConfig>) -> Result<Self> {
        Ok(Self {
            streaming: runtime_client(proxy, false)?,
            bounded: runtime_client(proxy, true)?,
            websocket: runtime_websocket_client(proxy)?,
        })
    }

    fn request(&self, upstream_stream: bool) -> &reqwest::Client {
        if upstream_stream {
            &self.streaming
        } else {
            &self.bounded
        }
    }
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
        validate_runtime_options(&options)?;
        let model_reasoning_allowed_levels = normalize_model_reasoning_allowed_levels(
            options.model_reasoning_allowed_levels.clone(),
        )
        .map_err(|message| Error::Validation(message.to_string()))?;

        let clients = RuntimeHttpClients::new(None)?;
        let discovery_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        let mut scheduler = configure_scheduler(&options)?;
        let mut registry = ModelRegistry::default();
        let image_base_model = normalize_image_base_model(options.image_base_model.clone())?;
        let source_parts = build_sources(sources, &mut registry, &mut scheduler)?;
        let account_parts = build_accounts(
            accounts,
            account_auth.as_ref(),
            image_base_model.as_deref(),
            &source_parts,
            &mut registry,
            &mut scheduler,
        )?;
        let hidden_models = normalized_set(options.hidden_models.iter());
        let key_parts = build_keys(keys, &hidden_models)?;
        validate_reachability(
            reachability_requirement,
            &source_parts,
            &account_parts,
            &key_parts,
            &scheduler,
        )?;

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
            clients,
            discovery_client,
            sources: source_parts.executors,
            source_candidate_bindings: source_parts.candidate_bindings,
            source_endpoint_domains: source_parts.endpoint_domains,
            source_recovery_delays_ms: Mutex::new(source_parts.recovery_delays_ms),
            chatgpt_accounts: account_parts.executors,
            keys: key_parts.runtime_keys,
            scheduler: Arc::new(Mutex::new(scheduler)),
            candidate_availability: Arc::new(tokio::sync::Notify::new()),
            registry,
            codex_responses_lite_models: Mutex::new(BTreeSet::new()),
            model_metadata: SourceModelMetadataState::default(),
            model_reasoning_allowed_levels: Mutex::new(model_reasoning_allowed_levels),
            passive_quotas: Mutex::new(account_parts.passive_quotas),
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
        discover_models_with_client(&self.discovery_client, source, source.protocol_bindings())
            .await
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
            return account.clients.request(upstream_stream);
        }
        self.clients.request(upstream_stream)
    }

    pub(crate) fn websocket_client(&self, candidate_id: &str) -> &reqwest::Client {
        self.chatgpt_accounts
            .get(candidate_id)
            .map(|account| &account.clients.websocket)
            .unwrap_or(&self.clients.websocket)
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

fn require_runtime_value(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(Error::Validation(format!("{name} must not be empty")))
    } else {
        Ok(())
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
    use crate::catalog::source_reasoning_capabilities;
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
    fn service_tier_storage_values_keep_fast_aliases_compatible() {
        assert_eq!(DefaultServiceTier::Standard.as_str(), "standard");
        assert_eq!(DefaultServiceTier::Fast.as_str(), "fast");
        assert_eq!(
            DefaultServiceTier::from_storage_value("priority"),
            DefaultServiceTier::Fast
        );
        assert_eq!(
            DefaultServiceTier::from_storage_value("unknown"),
            DefaultServiceTier::Standard
        );
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

    #[test]
    fn active_responses_scope_uses_live_candidate_policy() {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source-a",
                "upstream-secret",
                &["model-a"],
            ))],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let mut account = runtime
            .lock_scheduler()
            .candidate("source-a")
            .cloned()
            .unwrap();
        account.id = "account-a".into();
        account.kind = CandidateKind::OAuthAccount;
        account.source_id = "codex".into();
        account.account_id = Some("account-a".into());
        runtime.lock_scheduler().upsert(account);

        let source_ids = BTreeSet::from(["source-a".to_string()]);
        let account_ids = BTreeSet::from(["account-a".to_string()]);
        assert_eq!(
            runtime.active_responses_scope(&source_ids, &account_ids),
            CandidateScope {
                source_ids: Some(source_ids.clone()),
                account_ids: Some(account_ids.clone()),
                model_rules: ModelRules::default(),
            }
        );

        assert!(runtime.update_source_policy(
            "source-a",
            RuntimeCandidatePolicy {
                enabled: false,
                draining: false,
                priority: 0,
                weight: 1,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
            },
            0,
        ));
        assert!(runtime.update_account_policy(
            "account-a",
            RuntimeCandidatePolicy {
                enabled: false,
                draining: false,
                priority: 0,
                weight: 1,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
            },
        ));
        let scope = runtime.active_responses_scope(&source_ids, &account_ids);
        assert_eq!(scope.source_ids, Some(BTreeSet::new()));
        assert_eq!(scope.account_ids, Some(BTreeSet::new()));
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
    async fn fresh_source_metadata_does_not_wait_for_another_refresh() {
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
                    "reasoningEffortModes": ["low", "high"]
                }]
            }),
            now_ms,
        );

        let guard = runtime.model_metadata.refresh_lock.lock().await;
        let metadata = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            runtime.codex_source_model_metadata(&key, &[WireApi::Responses], now_ms),
        )
        .await
        .expect("fresh metadata must not wait for the refresh lock");
        drop(guard);

        assert!(metadata
            .reasoning_catalog_templates
            .contains_key("provider/fable"));
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
