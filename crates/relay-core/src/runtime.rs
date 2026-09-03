use crate::accounts::{TokenAuthority, TokenPersistenceAdapter, TokenRefreshAdapter};
use crate::catalog::{normalize_model_reasoning_allowed_levels, SourceReasoningCapabilities};
use crate::pricing::PricingCatalog;
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::{
    AgentIdentityCredential, CodexIdentityEnvelope, RuntimeChatGptAccount, RuntimeChatGptAuth,
};
use crate::quota::QuotaSnapshot;
use crate::scheduler::CooldownRequest;
use crate::sources::{discover_models_with_client, is_loopback_url};
use crate::ProxyConfig;
use crate::{
    decode_codex_model_alias, is_valid_model_id, CacheWriteTtl, CandidateScope, Error,
    LocalGatewayKey, MessagesReasoningMode, ModelRegistry, ModelRules, NativeResponsesReplayStore,
    PoolScheduler, ProviderSource, Result, RoutingDiagnostics, RoutingStrategy, RuntimeCandidate,
    SourceAdapter, SourceConnector, SourceProtocolBinding, SourceProtocolBindingKey, UsageCallback,
    WireApi,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
#[cfg(test)]
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
mod codex_metadata;
mod control;
mod images;
mod selection;
mod session_state;
mod source_metadata;

use control::RuntimeControl;
use session_state::CodexTurnStateStore;

use build::{
    build_accounts, build_keys, build_sources, configure_scheduler, validate_reachability,
    validate_runtime_options, ReachabilityRequirement,
};

#[cfg(test)]
use crate::{
    normalize_source_protocol_bindings, unix_time_ms as current_time_ms, CandidateKind, UsageEvent,
};
pub(crate) use images::is_image_model_id;
pub use images::normalize_image_base_model;
#[cfg(test)]
use images::{cheapest_image_main_model, select_image_main_model};

pub(crate) const MAX_NON_STREAM_BODY_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const IMAGE_API_MODEL: &str = "gpt-image-2";
const MAX_IDLE_CONNECTIONS_PER_HOST: usize = 256;
const CODEX_SOURCE_MODEL_MANIFEST_TTL_MS: u64 = 8 * 60 * 60 * 1_000;
const SOURCE_MODEL_METADATA_PREFETCH_INTERVAL_MS: u64 = 8 * 60 * 60 * 1_000;
pub(crate) const WEBSOCKET_CAPABILITY_TTL_MS: u64 = 5 * 60 * 1_000;
const CHATGPT_TEAM_BREAKER_DEDUP_MS: u64 = 60 * 1_000;

/// A request activity delta for hosts that render the pool while requests are
/// in flight. It intentionally contains only routing identifiers and counts;
/// prompts, keys, and provider responses never cross this boundary.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeActivitySnapshot {
    pub revision: u64,
    pub candidate_id: String,
    pub in_flight: u32,
    pub active_request_count: u32,
    pub active_models: Vec<crate::ActiveModelRuntime>,
}

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

fn apply_model_display_order(models: &mut [String], saved_order: &[String]) {
    let positions = saved_order
        .iter()
        .enumerate()
        .map(|(position, model)| (model.to_ascii_lowercase(), position))
        .collect::<BTreeMap<_, _>>();
    models.sort_by_key(|model| {
        positions
            .get(&model.to_ascii_lowercase())
            .copied()
            .unwrap_or(usize::MAX)
    });
}

fn source_reasoning_for_route(
    mut capabilities: SourceReasoningCapabilities,
    adapter: SourceAdapter,
    reasoning_mode: MessagesReasoningMode,
) -> Option<SourceReasoningCapabilities> {
    if capabilities.is_empty() {
        return Some(capabilities);
    }
    if adapter.is_passthrough() {
        return Some(capabilities);
    }
    capabilities
        .retain_efforts(|effort| reasoning_mode.supports_effort(effort))
        .then_some(())?;
    capabilities.clear_summary_capabilities();
    Some(capabilities)
}

fn declared_source_reasoning_levels(
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

/// Supplies the mutable portion of a configured source route. Storage
/// records stay owned by desktop and server, while their live-update policy is
/// deliberately one shared contract.
pub trait RuntimeSourcePolicyRecord {
    fn runtime_source_policy_update(&self) -> RuntimeSourcePolicyUpdate;
}

/// Selects source policy changes that can be applied to an existing runtime.
/// Connection, secret, protocol, and model-route changes are intentionally
/// outside this function because their executors are immutable and require a
/// rebuild.
pub fn changed_runtime_source_policy_updates<T: RuntimeSourcePolicyRecord>(
    previous: &[T],
    next: &[T],
) -> Vec<RuntimeSourcePolicyUpdate> {
    let previous_updates = previous
        .iter()
        .map(RuntimeSourcePolicyRecord::runtime_source_policy_update)
        .collect::<Vec<_>>();
    next.iter()
        .filter_map(|record| {
            let update = record.runtime_source_policy_update();
            let changed = previous_updates
                .iter()
                .find(|previous| previous.source_id == update.source_id)
                .is_none_or(|previous| {
                    previous.policy != update.policy
                        || previous.recovery_delay_seconds != update.recovery_delay_seconds
                });
            changed.then_some(update)
        })
        .collect()
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

/// Normalizes the explicit per-model policy. Older configurations may omit a
/// model, in which case the legacy pool default remains the fallback.
pub fn normalize_model_service_tier_overrides(
    overrides: BTreeMap<String, DefaultServiceTier>,
) -> std::result::Result<BTreeMap<String, DefaultServiceTier>, &'static str> {
    let mut normalized = BTreeMap::new();
    for (model, tier) in overrides {
        let model = model.trim();
        if !is_valid_model_id(model) {
            return Err("model service tier override has an invalid model id");
        }
        normalized.insert(model.to_ascii_lowercase(), tier);
    }
    Ok(normalized)
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
    /// Immutable pricing snapshot used only to rank the automatic image
    /// bridge model. A missing or empty snapshot never blocks runtime build.
    pub image_pricing_catalog: Option<Arc<PricingCatalog>>,
    /// Manually enabled source-model reasoning efforts. An absent model
    /// exposes no reasoning selector for API sources.
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
                "image_pricing_catalog",
                &self.image_pricing_catalog.as_ref().map(|_| "configured"),
            )
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
            image_pricing_catalog: None,
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
    source_recovery_delays_ms: Mutex<BTreeMap<String, u64>>,
    chatgpt_accounts: BTreeMap<String, ChatGptAccountExecutor>,
    chatgpt_team_members: BTreeMap<String, BTreeSet<String>>,
    chatgpt_team_breaker_recent: Mutex<BTreeMap<String, u64>>,
    keys: Vec<RuntimeKey>,
    scheduler: Arc<Mutex<PoolScheduler>>,
    candidate_availability: Arc<tokio::sync::Notify>,
    registry: ModelRegistry,
    codex_responses_lite_models: Mutex<BTreeSet<(String, String)>>,
    websocket_http_only: Mutex<BTreeMap<(String, String), u64>>,
    model_metadata: SourceModelMetadataState,
    model_reasoning_allowed_levels: Mutex<BTreeMap<String, Vec<String>>>,
    model_service_tier_overrides: Mutex<BTreeMap<String, DefaultServiceTier>>,
    model_display_order: Mutex<Vec<String>>,
    passive_quotas: Mutex<BTreeMap<String, PassiveQuotaState>>,
    messages_bridge_store: Mutex<crate::MessagesBridgeStore>,
    native_responses_replay_store: Mutex<NativeResponsesReplayStore>,
    codex_turn_state_store: CodexTurnStateStore,
    control: RuntimeControl,
    max_retry_candidates: usize,
    quota_stale_after_ms: u64,
    default_service_tier_fast: AtomicBool,
    response_affinity_store: Option<Arc<dyn ResponseAffinityStore>>,
    activity_callback: Arc<Mutex<RuntimeActivityCallback>>,
    activity_revision: Arc<AtomicU64>,
    chatgpt_team_breaker_callback: Arc<Mutex<RuntimeTeamBreakerCallback>>,
    pub(crate) usage: UsageCallback,
}

type RuntimeActivityCallback = Arc<dyn Fn(RuntimeActivitySnapshot) + Send + Sync>;
type RuntimeTeamBreakerCallback = Arc<dyn Fn(Vec<String>) + Send + Sync>;

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
struct DeclaredSourceReasoning {
    efforts: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    empty_routes: BTreeMap<String, BTreeSet<String>>,
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
    /// Source-declared effort metadata and its derived model-level catalog.
    /// Native account metadata is intentionally kept out of this state. This
    /// state is presentation metadata only; it is never request admission
    /// evidence.
    declared_reasoning: Mutex<DeclaredSourceReasoning>,
}

impl Default for SourceModelMetadataState {
    fn default() -> Self {
        Self {
            codex_manifests: Mutex::new(BTreeMap::new()),
            source_manifests: Mutex::new(BTreeMap::new()),
            refresh_lock: tokio::sync::Mutex::new(()),
            prefetch_pending: AtomicBool::new(false),
            prefetch_not_before_ms: AtomicU64::new(0),
            declared_reasoning: Mutex::new(DeclaredSourceReasoning::default()),
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
    activity_callback: Arc<Mutex<RuntimeActivityCallback>>,
    activity_revision: Arc<AtomicU64>,
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
        let activity = {
            let mut scheduler = self
                .scheduler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let released = match self.lane {
                CandidateLeaseLane::Text => {
                    scheduler.release_for(&self.candidate_id, Some(&self.model))
                }
                CandidateLeaseLane::Image => {
                    scheduler.release_image_for(&self.candidate_id, Some(&self.model))
                }
            };
            if !released {
                None
            } else {
                let (in_flight, active_request_count, active_models) =
                    scheduler.runtime_activity_for(&self.candidate_id);
                Some(RuntimeActivitySnapshot {
                    revision: self.activity_revision.fetch_add(1, Ordering::AcqRel) + 1,
                    candidate_id: self.candidate_id.clone(),
                    in_flight,
                    active_request_count,
                    active_models,
                })
            }
        };
        if let Some(activity) = activity {
            self.availability.notify_one();
            let callback = self
                .activity_callback
                .lock()
                .ok()
                .map(|callback| callback.clone());
            if let Some(callback) = callback {
                callback(activity);
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
    pub(crate) client_context_id: Option<String>,
    pub(crate) scope: CandidateScope,
    pub(crate) allowed_protocols: Vec<WireApi>,
    pub(crate) wire_api: WireApi,
    pub(crate) adapter: SourceAdapter,
    pub(crate) reasoning_mode: MessagesReasoningMode,
    pub(crate) cache_write_ttl: CacheWriteTtl,
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
    cache_write_ttl: CacheWriteTtl,
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
        let image_pricing_catalog = options.image_pricing_catalog.as_deref();
        let source_parts = build_sources(sources, &mut registry, &mut scheduler)?;
        let account_parts = build_accounts(
            accounts,
            account_auth.as_ref(),
            image_base_model.as_deref(),
            image_pricing_catalog,
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
                    let restored = if binding.key.starts_with("cache:")
                        || binding.key.starts_with("session:")
                    {
                        scheduler.restore_prompt_affinity(
                            binding.key.clone(),
                            &binding.candidate_id,
                            binding.expires_at_ms,
                            now_ms,
                        )
                    } else {
                        scheduler.restore_response_affinity(
                            binding.key.clone(),
                            &binding.candidate_id,
                            binding.expires_at_ms,
                            now_ms,
                        )
                    };
                    if !restored {
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
            source_recovery_delays_ms: Mutex::new(source_parts.recovery_delays_ms),
            chatgpt_accounts: account_parts.executors,
            chatgpt_team_members: account_parts.team_members,
            chatgpt_team_breaker_recent: Mutex::new(BTreeMap::new()),
            keys: key_parts.runtime_keys,
            scheduler: Arc::new(Mutex::new(scheduler)),
            candidate_availability: Arc::new(tokio::sync::Notify::new()),
            registry,
            codex_responses_lite_models: Mutex::new(BTreeSet::new()),
            websocket_http_only: Mutex::new(BTreeMap::new()),
            model_metadata: SourceModelMetadataState::default(),
            model_reasoning_allowed_levels: Mutex::new(model_reasoning_allowed_levels),
            model_service_tier_overrides: Mutex::new(BTreeMap::new()),
            model_display_order: Mutex::new(Vec::new()),
            passive_quotas: Mutex::new(account_parts.passive_quotas),
            messages_bridge_store: Mutex::new(crate::MessagesBridgeStore::default()),
            native_responses_replay_store: Mutex::new(NativeResponsesReplayStore::default()),
            codex_turn_state_store: CodexTurnStateStore::default(),
            control: RuntimeControl::default(),
            max_retry_candidates: options.max_retry_candidates,
            quota_stale_after_ms: options.quota_stale_after_ms,
            default_service_tier_fast: AtomicBool::new(
                options.default_service_tier == DefaultServiceTier::Fast,
            ),
            response_affinity_store: affinity_store,
            activity_callback: Arc::new(Mutex::new(Arc::new(|_| {}))),
            activity_revision: Arc::new(AtomicU64::new(0)),
            chatgpt_team_breaker_callback: Arc::new(Mutex::new(Arc::new(|_| {}))),
            usage,
        })
    }

    /// Installs a lightweight observer for request start/end activity.
    /// The callback carries only routing identifiers and live counts; request
    /// data and provider responses never cross the host boundary.
    pub fn set_activity_callback(
        &self,
        callback: impl Fn(RuntimeActivitySnapshot) + Send + Sync + 'static,
    ) {
        if let Ok(mut current) = self.activity_callback.lock() {
            *current = Arc::new(callback);
        }
    }

    /// Installs the host-specific persistence hook for Team breaker siblings.
    /// The callback receives only candidate ids; local and server pools keep
    /// their own account stores and may persist the block independently.
    pub fn set_chatgpt_team_breaker_callback(
        &self,
        callback: impl Fn(Vec<String>) + Send + Sync + 'static,
    ) {
        if let Ok(mut current) = self.chatgpt_team_breaker_callback.lock() {
            *current = Arc::new(callback);
        }
    }

    pub(crate) fn emit_activity_changed(&self, activity: RuntimeActivitySnapshot) {
        let callback = self
            .activity_callback
            .lock()
            .ok()
            .map(|callback| callback.clone());
        if let Some(callback) = callback {
            callback(activity);
        }
    }

    pub fn codex_background_tasks_enabled(&self) -> bool {
        self.control.codex_background_tasks_enabled()
    }

    pub fn set_codex_background_tasks_enabled(&self, enabled: bool) {
        self.control.set_codex_background_tasks_enabled(enabled);
    }

    pub fn codex_websockets_enabled(&self) -> bool {
        self.control.codex_websockets_enabled()
    }

    pub fn set_codex_websockets_enabled(&self, enabled: bool) {
        self.control.set_codex_websockets_enabled(enabled);
    }

    pub(crate) fn mark_request_origin(&self, request_id: &str, origin: &'static str) {
        self.control.mark_request_origin(request_id, origin);
    }

    pub(crate) fn request_origin(&self, request_id: &str) -> Option<&'static str> {
        self.control.request_origin(request_id)
    }

    pub(crate) fn blocked_codex_background_event(
        &self,
        request_id: &str,
        local_key_id: &str,
        requested_model: &str,
        wire_api: WireApi,
        origin: &'static str,
    ) {
        self.control.blocked_codex_background_event(
            &self.usage,
            request_id,
            local_key_id,
            requested_model,
            wire_api,
            origin,
        );
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
            .map(|key| self.authenticated_key(key))
    }

    fn authenticated_key(&self, key: &RuntimeKey) -> AuthenticatedKey {
        AuthenticatedKey {
            id: key.id.clone(),
            scope: key.scope.clone(),
            model_rules: key.model_rules.clone(),
            model_prefix: key.model_prefix.clone(),
            client_wire_apis: key.client_wire_apis.clone(),
        }
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
        let mut models = self
            .registry
            .visible_models(&scheduler, &scope, allowed_protocols, now_ms)
            .into_iter()
            .filter(|model| key.model_rules.allows(model))
            .collect::<Vec<_>>();
        let order = self
            .model_display_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        apply_model_display_order(&mut models, &order);
        models
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
            let can_prepare =
                auth_state.is_none_or(|auth_state| !auth_state.requires_fresh_login());
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
    /// union of efforts declared by at least one source. Discovery metadata
    /// only informs the picker: source routing stays model-based and request
    /// admission never depends on reasoning metadata.
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
        let Some(key) = self.authenticate_secret(secret) else {
            return Vec::new();
        };
        self.visible_models(&key, allowed_protocols, now_ms)
    }

    pub(crate) fn executor_route(
        &self,
        candidate_id: &str,
        model: &str,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
        upstream_stream: bool,
    ) -> Option<ExecutorRoute> {
        if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
            let source = self.sources.get(&binding.source_id)?;
            let source_binding = source.binding_for(binding.binding_key)?;
            let source_model = source.canonical_model_for(binding.binding_key, model)?;
            return Some(ExecutorRoute {
                candidate_id: candidate_id.to_string(),
                source_id: binding.source_id.clone(),
                account_id: None,
                client_context_id: None,
                scope: scope.clone(),
                allowed_protocols: allowed_protocols.to_vec(),
                wire_api: binding.wire_api,
                adapter: binding.adapter,
                reasoning_mode: binding.reasoning_mode,
                cache_write_ttl: binding.cache_write_ttl,
                service_tier: DefaultServiceTier::Standard,
                upstream_url: source.endpoint(
                    binding.binding_key,
                    &source_model,
                    upstream_stream,
                )?,
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
            client_context_id: None,
            scope: scope.clone(),
            allowed_protocols: allowed_protocols.to_vec(),
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: CacheWriteTtl::Provider,
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
        model: &str,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
    ) -> Option<ExecutorRoute> {
        if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
            if !binding.adapter.is_passthrough() {
                return None;
            }
            let source = self.sources.get(&binding.source_id)?;
            let source_binding = source.binding_for(binding.binding_key)?;
            let source_model = source.canonical_model_for(binding.binding_key, model)?;
            return Some(ExecutorRoute {
                candidate_id: candidate_id.to_string(),
                source_id: binding.source_id.clone(),
                account_id: None,
                client_context_id: None,
                scope: scope.clone(),
                allowed_protocols: allowed_protocols.to_vec(),
                wire_api: binding.wire_api,
                adapter: binding.adapter,
                reasoning_mode: binding.reasoning_mode,
                cache_write_ttl: binding.cache_write_ttl,
                service_tier: DefaultServiceTier::Standard,
                upstream_url: source.endpoint(binding.binding_key, &source_model, false)?,
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
            client_context_id: None,
            scope: scope.clone(),
            allowed_protocols: allowed_protocols.to_vec(),
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: CacheWriteTtl::Provider,
            service_tier: DefaultServiceTier::Standard,
            upstream_url: account.responses_url.clone(),
            upstream_headers: HeaderMap::new(),
            source_model: account.image_main_model.clone()?,
            half_open_probe: false,
            routing: None,
        })
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

    pub(crate) fn websocket_is_http_only(
        &self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
    ) -> bool {
        self.websocket_http_only
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(candidate_id.to_string(), model.to_string()))
            .is_some_and(|observed_at| {
                now_ms.saturating_sub(*observed_at) < WEBSOCKET_CAPABILITY_TTL_MS
            })
    }

    pub(crate) fn mark_websocket_supported(&self, candidate_id: &str, model: &str) {
        self.websocket_http_only
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(candidate_id.to_string(), model.to_string()));
    }

    pub(crate) fn mark_websocket_http_only(&self, candidate_id: &str, model: &str, now_ms: u64) {
        self.websocket_http_only
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((candidate_id.to_string(), model.to_string()), now_ms);
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

    /// Applies the operator-selected two-speed policy. Client-owned API
    /// requests retain an explicit tier at the gateway boundary.
    pub fn set_model_service_tier_overrides(
        &self,
        overrides: BTreeMap<String, DefaultServiceTier>,
    ) -> Result<()> {
        let overrides = normalize_model_service_tier_overrides(overrides)
            .map_err(|message| Error::Validation(message.to_string()))?;
        *self
            .model_service_tier_overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = overrides;
        Ok(())
    }

    pub(crate) fn model_service_tier(&self, model: &str) -> DefaultServiceTier {
        self.model_service_tier_overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&model.trim().to_ascii_lowercase())
            .copied()
            .unwrap_or_else(|| self.default_service_tier())
    }

    pub fn set_model_display_order(&self, models: Vec<String>) {
        *self
            .model_display_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            crate::normalize_model_ids(models);
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
        WireApi::Gemini,
    ]
}

fn client_wire_apis_to_native(client_wire_apis: &[ClientWireApi]) -> Vec<WireApi> {
    client_wire_apis
        .iter()
        .map(|wire_api| match wire_api {
            ClientWireApi::Responses => WireApi::Responses,
            ClientWireApi::ChatCompletions | ClientWireApi::Images => WireApi::ChatCompletions,
            ClientWireApi::Messages => WireApi::Messages,
            ClientWireApi::Gemini => WireApi::Gemini,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn runtime_now_ms() -> u64 {
    crate::unix_time_ms()
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
mod tests;
