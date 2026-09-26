use crate::accounts::{
    TokenAuthority, TokenDispatchRevision, TokenPersistenceAdapter, TokenRefreshAdapter,
};
use crate::catalog::normalize_model_reasoning_allowed_levels;
use crate::model_metadata::ModelMetadataCatalogHandle;
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
    PoolScheduler, ProviderSource, Result, RoutingDiagnostics, RuntimeCandidate, SourceAdapter,
    SourceConnector, SourceProtocolBinding, SourceProtocolBindingKey, UsageCallback, WireApi,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
#[cfg(test)]
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;
use subtle::ConstantTimeEq;
use url::Url;

mod admission;
mod attempt;
mod authorization;
mod build;
mod candidates;
mod codex_metadata;
mod control;
mod images;
mod model_speed;
mod routing_cookies;
mod selection;
mod session_state;
mod source_metadata;

pub(crate) use attempt::CandidateLease;
use attempt::CandidateLeaseLane;
pub use attempt::ExecutionFence;
use control::RuntimeControl;
pub(crate) use session_state::CodexTurnStateScope;
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
pub(crate) const WEBSOCKET_CAPABILITY_TTL_MS: u64 = 5 * 60 * 1_000;
const CHATGPT_TEAM_BREAKER_DEDUP_MS: u64 = 60 * 1_000;
static NEXT_ACTIVITY_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);

/// A request activity delta for hosts that render the pool while requests are
/// in flight. It intentionally contains only routing identifiers and counts;
/// prompts, keys, and provider responses never cross this boundary.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeActivitySnapshot {
    #[serde(default)]
    pub runtime_id: u64,
    pub revision: u64,
    pub candidate_id: String,
    /// Physical refresh member; a source may have several protocol candidates.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub member_key: String,
    pub in_flight: u32,
    pub active_request_count: u32,
    pub active_models: Vec<crate::ActiveModelRuntime>,
}

fn source_candidate_id(
    source_id: &str,
    binding: &SourceProtocolBinding,
    legacy_protocol: WireApi,
) -> String {
    if binding.adapter.is_passthrough() && binding.wire_api == legacy_protocol {
        return source_id.to_string();
    }
    let suffix = binding.adapter.route_suffix(binding.wire_api);
    format!("{source_id}::{suffix}")
}

#[derive(Clone, Debug)]
pub struct RuntimeSource {
    pub source: ProviderSource,
    /// Legacy persisted bindings retained for backward-compatible reads.
    /// Runtime routes are derived automatically from protocol evidence and
    /// fall back to the source-wide `wire_api` when evidence is absent.
    pub protocol_bindings: Vec<SourceProtocolBinding>,
    pub protocol_config: crate::SourceProtocolConfig,
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
            protocol_config: crate::SourceProtocolConfig::default(),
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
/// routes. Source routes and connection changes require a new runtime;
/// discovered OAuth model inventory has its own in-place reconciliation.
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
    /// OpenAI's access-controlled lowest-latency tier.
    Ultrafast,
}

/// Normalizes the explicit per-model policy. Capability is resolved only from
/// a live route's confirmed upstream manifest, so persistence must retain a
/// valid model ID without inferring support from its spelling.
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
            Self::Ultrafast => "ultrafast",
        }
    }

    /// Parses the durable service-tier spelling, including the legacy Codex
    /// `priority` alias for Relay's fast tier.
    pub fn from_storage_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "ultrafast" => Self::Ultrafast,
            "fast" | "priority" => Self::Fast,
            _ => Self::Standard,
        }
    }

    pub(crate) const fn atomic_value(self) -> u8 {
        match self {
            Self::Standard => 0,
            Self::Fast => 1,
            Self::Ultrafast => 2,
        }
    }

    pub(crate) const fn from_atomic_value(value: u8) -> Self {
        match value {
            1 => Self::Fast,
            2 => Self::Ultrafast,
            _ => Self::Standard,
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
    pub tool_policy: crate::ToolPolicy,
    pub max_retry_candidates: usize,
    pub pool_routing: Option<crate::PoolRoutingPolicy>,
    pub hidden_models: Vec<String>,
    pub default_service_tier: DefaultServiceTier,
    pub quota_stale_after_ms: u64,
    /// Optional text model used as the Responses image-generation bridge.
    /// `None` selects the cheapest known compatible model per account.
    pub image_base_model: Option<String>,
    /// Immutable pricing snapshot used only to rank the automatic image
    /// bridge model. A missing or empty snapshot never blocks runtime build.
    pub image_pricing_catalog: Option<Arc<PricingCatalog>>,
    /// Advisory presentation metadata. It only orders model-list responses;
    /// admission and routing remain owned by the runtime registry.
    pub model_metadata_catalog: Option<ModelMetadataCatalogHandle>,
    /// Manually enabled source-model reasoning efforts. An absent model
    /// exposes no reasoning selector for API sources.
    pub model_reasoning_allowed_levels: BTreeMap<String, Vec<String>>,
    pub response_affinity_store: Option<Arc<dyn ResponseAffinityStore>>,
}

impl fmt::Debug for GatewayRuntimeOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayRuntimeOptions")
            .field("tool_policy_mode", &self.tool_policy.mode)
            .field("max_retry_candidates", &self.max_retry_candidates)
            .field("hidden_models", &self.hidden_models)
            .field("default_service_tier", &self.default_service_tier)
            .field("quota_stale_after_ms", &self.quota_stale_after_ms)
            .field("image_base_model", &self.image_base_model)
            .field(
                "image_pricing_catalog",
                &self.image_pricing_catalog.as_ref().map(|_| "configured"),
            )
            .field(
                "model_metadata_catalog",
                &self.model_metadata_catalog.as_ref().map(|_| "configured"),
            )
            .field(
                "model_reasoning_allowed_levels",
                &self.model_reasoning_allowed_levels,
            )
            .field(
                "response_affinity_store",
                &self.response_affinity_store.as_ref().map(|_| "configured"),
            )
            .finish()
    }
}

impl Default for GatewayRuntimeOptions {
    fn default() -> Self {
        Self {
            tool_policy: crate::ToolPolicy::default(),
            max_retry_candidates: 3,
            pool_routing: None,
            hidden_models: Vec::new(),
            default_service_tier: DefaultServiceTier::Standard,
            quota_stale_after_ms: crate::QUOTA_STALE_AFTER_MS,
            image_base_model: None,
            image_pricing_catalog: None,
            model_metadata_catalog: None,
            model_reasoning_allowed_levels: BTreeMap::new(),
            response_affinity_store: None,
        }
    }
}

pub struct GatewayRuntime {
    tool_policy: RwLock<crate::ToolPolicy>,
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
    admission: Mutex<admission::AdmissionQueue>,
    admission_changed: tokio::sync::Notify,
    registry: Mutex<ModelRegistry>,
    image_base_model: Option<String>,
    image_pricing_catalog: Option<Arc<PricingCatalog>>,
    codex_responses_lite_models: Mutex<BTreeSet<(String, String)>>,
    websocket_http_only: Mutex<BTreeMap<(String, String), u64>>,
    model_metadata: SourceModelMetadataState,
    model_reasoning_allowed_levels: Mutex<BTreeMap<String, Vec<String>>>,
    model_service_tier_overrides: Mutex<BTreeMap<String, DefaultServiceTier>>,
    model_display_order: Mutex<Vec<String>>,
    model_metadata_catalog: Option<ModelMetadataCatalogHandle>,
    passive_quotas: Mutex<BTreeMap<String, PassiveQuotaState>>,
    messages_bridge_store: Mutex<crate::MessagesBridgeStore>,
    native_responses_replay_store: Mutex<NativeResponsesReplayStore>,
    codex_turn_state_store: CodexTurnStateStore,
    control: RuntimeControl,
    max_retry_candidates: std::sync::atomic::AtomicUsize,
    quota_stale_after_ms: u64,
    default_service_tier_value: AtomicU8,
    response_affinity_store: Option<Arc<dyn ResponseAffinityStore>>,
    activity_callback: Arc<Mutex<RuntimeActivityCallback>>,
    activity_runtime_id: u64,
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
}

#[derive(Default)]
struct SourceModelMetadataState {
    /// Native Codex transport cards. Model capabilities use the reference catalog.
    codex_manifests: Mutex<BTreeMap<String, CachedModelManifest>>,
}

#[derive(Clone)]
pub(crate) struct AuthenticatedKey {
    pub(crate) id: String,
    scope: Arc<RwLock<CandidateScope>>,
    scope_revision: Arc<AtomicU64>,
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
    chatgpt_account_id: String,
    identity: CodexIdentityEnvelope,
    responses_url: Url,
    basis_points_url: Url,
    basis_points_enabled: AtomicBool,
    model_inventory: RwLock<AccountModelInventory>,
    image_bridge_revision: Arc<AtomicU64>,
    token_authority: Arc<TokenAuthority>,
    refresh_adapter: Arc<dyn TokenRefreshAdapter>,
    persistence_adapter: Arc<dyn TokenPersistenceAdapter>,
    refresh_skew_ms: u64,
    clients: RuntimeHttpClients,
    active: AtomicBool,
    agent_identity: RwLock<Option<AgentIdentityCredential>>,
    agent_identity_revision: AtomicU64,
    agent_task_lock: tokio::sync::Mutex<()>,
    routing_cookies: routing_cookies::RoutingCookies,
}

struct AccountModelInventory {
    configured_models: BTreeSet<String>,
    image_main_model: Option<String>,
}

#[derive(Clone)]
pub(crate) struct ExecutorRoute {
    pub(crate) candidate_id: String,
    pub(crate) source_id: String,
    pub(crate) account_id: Option<String>,
    /// The exact OAuth token generation that was used for this upstream
    /// request. This is request-local provenance, not route configuration.
    pub(crate) account_token_generation: Option<u64>,
    pub(crate) client_context_id: Option<String>,
    pub(crate) wire_api: WireApi,
    pub(crate) adapter: SourceAdapter,
    pub(crate) reasoning_mode: MessagesReasoningMode,
    pub(crate) cache_write_ttl: CacheWriteTtl,
    pub(crate) service_tier: DefaultServiceTier,
    pub(crate) upstream_url: Url,
    pub(crate) upstream_headers: HeaderMap,
    pub(crate) account_transport: AccountTransport,
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
    pub(crate) token_revision: Option<TokenDispatchRevision>,
    pub(crate) agent_task_id: Option<String>,
    pub(crate) agent_credential_fingerprint: Option<[u8; 32]>,
    pub(crate) agent_identity_revision: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AuthorizationIncarnation {
    Source,
    OAuth(TokenDispatchRevision),
    Agent(u64),
}

/// The upstream response together with the exact OAuth credential generation
/// that authorized it. Keeping this alongside the response lets delayed usage
/// callbacks distinguish an old 401 from a failure of a newer login.
pub(crate) struct AuthorizedResponse {
    pub(crate) response: reqwest::Response,
    pub(crate) account_token_generation: Option<u64>,
}

#[derive(Debug)]
pub(crate) enum AuthorizedRequestError {
    Prepare(ExecutorPrepareError),
    Transport(reqwest::Error),
    NotReplayable,
    DispatchBudgetExhausted,
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
    scope_revision: Arc<AtomicU64>,
    model_rules: ModelRules,
    model_prefix: Option<String>,
    client_wire_apis: Option<Vec<ClientWireApi>>,
}

struct RuntimeHttpClients {
    http: reqwest::Client,
    websocket: reqwest::Client,
}

impl RuntimeHttpClients {
    fn new(proxy: Option<&ProxyConfig>) -> Result<Self> {
        Ok(Self {
            http: runtime_client(proxy)?,
            websocket: runtime_websocket_client(proxy)?,
        })
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
        let tool_policy = options
            .tool_policy
            .clone()
            .normalized()
            .map_err(|message| Error::Validation(message.to_string()))?;
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
        // All callers use pool rotation. Older settings migrate once to the
        // same explicit policy used by desktop and server.
        let policy = options
            .pool_routing
            .clone()
            .unwrap_or_else(|| scheduler.migrated_pool_routing());
        scheduler.set_pool_routing(policy)?;
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
            tool_policy: RwLock::new(tool_policy),
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
            admission: Mutex::default(),
            admission_changed: tokio::sync::Notify::new(),
            registry: Mutex::new(registry),
            image_base_model,
            image_pricing_catalog: options.image_pricing_catalog.clone(),
            codex_responses_lite_models: Mutex::new(BTreeSet::new()),
            websocket_http_only: Mutex::new(BTreeMap::new()),
            model_metadata: SourceModelMetadataState::default(),
            model_reasoning_allowed_levels: Mutex::new(model_reasoning_allowed_levels),
            model_service_tier_overrides: Mutex::new(BTreeMap::new()),
            model_display_order: Mutex::new(Vec::new()),
            model_metadata_catalog: options.model_metadata_catalog.clone(),
            passive_quotas: Mutex::new(account_parts.passive_quotas),
            messages_bridge_store: Mutex::new(crate::MessagesBridgeStore::default()),
            native_responses_replay_store: Mutex::new(NativeResponsesReplayStore::default()),
            codex_turn_state_store: CodexTurnStateStore::default(),
            control: RuntimeControl::default(),
            max_retry_candidates: std::sync::atomic::AtomicUsize::new(options.max_retry_candidates),
            quota_stale_after_ms: options.quota_stale_after_ms,
            default_service_tier_value: AtomicU8::new(options.default_service_tier.atomic_value()),
            response_affinity_store: affinity_store,
            activity_callback: Arc::new(Mutex::new(Arc::new(|_| {}))),
            activity_runtime_id: NEXT_ACTIVITY_RUNTIME_ID.fetch_add(1, Ordering::Relaxed),
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

    /// Requests snapshot this value once. Hot updates never rebuild the
    /// listener or change an already admitted request's policy during retry.
    pub fn tool_policy(&self) -> crate::ToolPolicy {
        self.tool_policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_tool_policy(&self, policy: crate::ToolPolicy) -> Result<()> {
        let policy = policy
            .normalized()
            .map_err(|message| Error::Validation(message.to_string()))?;
        *self
            .tool_policy
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = policy;
        Ok(())
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

    pub fn route_recovery_enabled(&self) -> bool {
        self.control.route_recovery_enabled()
    }

    pub fn set_route_recovery_enabled(&self, enabled: bool) {
        self.control.set_route_recovery_enabled(enabled);
        self.candidate_availability.notify_waiters();
    }

    /// The bounded retry window for gateway requests without persistent route
    /// recovery. It starts at the first replay-safe rejection, not dispatch.
    pub fn route_recovery_window_ms(&self) -> u64 {
        self.control.route_recovery_window_ms()
    }

    pub fn set_route_recovery_window_ms(&self, value: u64) {
        self.control.set_route_recovery_window_ms(value);
        self.candidate_availability.notify_waiters();
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

    /// Creates an ephemeral key scope for scheduler-owned work on exactly one
    /// OAuth account.  Background probes must use the same scheduler,
    /// cooldowns, token authority, and usage callback as normal gateway
    /// requests, but they must never inherit the user's broad pool scope or
    /// fall back to a different account.
    pub(crate) fn internal_account_key(
        &self,
        local_key_id: &str,
        account_id: &str,
    ) -> Option<AuthenticatedKey> {
        let local_key_id = local_key_id.trim();
        let account_id = account_id.trim();
        if local_key_id.is_empty() || account_id.is_empty() {
            return None;
        }
        self.chatgpt_accounts.get(account_id)?;
        Some(AuthenticatedKey {
            id: local_key_id.to_string(),
            scope: Arc::new(RwLock::new(CandidateScope {
                // An explicit empty source set prevents a synthetic internal
                // key from selecting an API source while the account set below
                // pins selection to the requested OAuth candidate.
                source_ids: Some(BTreeSet::new()),
                account_ids: Some(BTreeSet::from([account_id.to_string()])),
                model_rules: ModelRules::default(),
            })),
            scope_revision: Arc::new(AtomicU64::new(0)),
            model_rules: ModelRules::default(),
            model_prefix: None,
            client_wire_apis: Some(vec![ClientWireApi::Responses]),
        })
    }

    fn authenticated_key(&self, key: &RuntimeKey) -> AuthenticatedKey {
        AuthenticatedKey {
            id: key.id.clone(),
            scope: key.scope.clone(),
            scope_revision: key.scope_revision.clone(),
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

    /// Resolves a model that belongs to at least one configured route even
    /// when every such route is temporarily hidden by runtime health. This is
    /// deliberately narrower than `resolve_model`: unknown model ids must
    /// still fail admission instead of occupying a retry window.
    pub(crate) fn resolve_configured_model(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
    ) -> Option<String> {
        let scope = key.scope_snapshot();
        let scheduler = self.lock_scheduler();
        let resolve = |candidate: &str| {
            let resolved = self.resolve_model(key, candidate)?;
            scheduler
                .candidates()
                .any(|candidate| candidate.is_configured(&resolved, allowed_protocols, &scope))
                .then_some(resolved)
        };
        resolve(model).or_else(|| decode_codex_model_alias(model).and_then(|id| resolve(&id)))
    }

    pub(crate) fn resolve_visible_account_model(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> Option<String> {
        self.resolve_from_visible(key, model, &self.visible_account_models(key))
    }

    pub(crate) fn resolve_configured_account_model(
        &self,
        key: &AuthenticatedKey,
        model: &str,
    ) -> Option<String> {
        let resolve = |candidate: &str| {
            let resolved = self.resolve_model(key, candidate)?;
            (!self
                .codex_model_chatgpt_account_ids_for_resolved(key, &resolved)
                .is_empty())
            .then_some(resolved)
        };
        resolve(model).or_else(|| decode_codex_model_alias(model).and_then(|id| resolve(&id)))
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
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .visible_models(&scheduler, &scope, allowed_protocols, now_ms)
            .into_iter()
            .filter(|model| key.model_rules.allows(model))
            .collect::<Vec<_>>();
        let order = self
            .model_display_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        models = self.model_metadata_catalog.as_ref().map_or_else(
            || crate::normalize_model_ids(models.iter()),
            |catalog| {
                catalog
                    .snapshot()
                    .merge_display_order(models.iter(), &order)
            },
        );
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
                    let inventory = account
                        .model_inventory
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let visible_models = inventory
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
        if !self
            .lock_scheduler()
            .candidate(candidate_id)
            .is_some_and(|candidate| candidate.is_configured(model, allowed_protocols, scope))
        {
            return None;
        }
        if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
            return self.source_executor_route(candidate_id, binding, model, upstream_stream);
        }
        let account = self.chatgpt_accounts.get(candidate_id)?;
        let source_model = account.canonical_model(model)?;
        Some(Self::account_executor_route(
            account,
            source_model,
            allowed_protocols,
        ))
    }

    pub(crate) fn image_executor_route(
        &self,
        candidate_id: &str,
        model: &str,
        scope: &CandidateScope,
        allowed_protocols: &[WireApi],
    ) -> Option<ExecutorRoute> {
        if !self
            .lock_scheduler()
            .candidate(candidate_id)
            .is_some_and(|candidate| candidate.is_configured(model, allowed_protocols, scope))
        {
            return None;
        }
        if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
            if !binding.adapter.is_passthrough() {
                return None;
            }
            return self.source_executor_route(candidate_id, binding, model, false);
        }
        let account = self.chatgpt_accounts.get(candidate_id)?;
        Some(Self::account_executor_route(
            account,
            account
                .model_inventory
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .image_main_model
                .clone()?,
            allowed_protocols,
        ))
    }

    fn source_executor_route(
        &self,
        candidate_id: &str,
        binding: &SourceCandidateBinding,
        model: &str,
        upstream_stream: bool,
    ) -> Option<ExecutorRoute> {
        let source = self.sources.get(&binding.source_id)?;
        let source_binding = source.binding_for(binding.binding_key)?;
        let source_model = source.canonical_model_for(binding.binding_key, model)?;
        Some(ExecutorRoute {
            candidate_id: candidate_id.to_string(),
            source_id: binding.source_id.clone(),
            account_id: None,
            account_token_generation: None,
            client_context_id: None,
            wire_api: binding.wire_api,
            adapter: binding.adapter,
            reasoning_mode: binding.reasoning_mode,
            cache_write_ttl: binding.cache_write_ttl,
            service_tier: DefaultServiceTier::Standard,
            upstream_url: source.endpoint(binding.binding_key, &source_model, upstream_stream)?,
            upstream_headers: source.protocol_headers_for_binding(source_binding),
            account_transport: AccountTransport::NativeResponses,
            source_model,
            half_open_probe: false,
            routing: None,
        })
    }

    fn account_executor_route(
        account: &ChatGptAccountExecutor,
        source_model: String,
        allowed_protocols: &[WireApi],
    ) -> ExecutorRoute {
        let wire_api = allowed_protocols
            .first()
            .copied()
            .unwrap_or(WireApi::Responses);
        let adapter =
            SourceAdapter::between(wire_api, WireApi::Responses).expect("registered account route");
        let account_transport = if account.basis_points_enabled.load(Ordering::Relaxed)
            && account
                .agent_identity
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none()
        {
            AccountTransport::ExcelBasisPoints
        } else {
            AccountTransport::NativeResponses
        };
        ExecutorRoute {
            candidate_id: account.id.clone(),
            source_id: account.source_id.clone(),
            account_id: Some(account.id.clone()),
            account_token_generation: None,
            client_context_id: None,
            wire_api,
            adapter,
            reasoning_mode: if adapter.is_passthrough() {
                MessagesReasoningMode::Disabled
            } else {
                MessagesReasoningMode::Adaptive
            },
            cache_write_ttl: CacheWriteTtl::Provider,
            service_tier: DefaultServiceTier::Standard,
            upstream_url: match account_transport {
                AccountTransport::NativeResponses => account.responses_url.clone(),
                AccountTransport::ExcelBasisPoints => account.basis_points_url.clone(),
            },
            upstream_headers: match account_transport {
                AccountTransport::NativeResponses => HeaderMap::new(),
                AccountTransport::ExcelBasisPoints => {
                    basis_points_headers(&account.chatgpt_account_id)
                }
            },
            account_transport,
            source_model,
            half_open_probe: false,
            routing: None,
        }
    }

    pub(crate) fn request_client(&self, candidate_id: &str) -> &reqwest::Client {
        if let Some(account) = self.chatgpt_accounts.get(candidate_id) {
            return &account.clients.http;
        }
        &self.clients.http
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

    /// The saved dispatch limit applies to the entire incoming request.
    pub(crate) fn request_dispatch_budget(&self) -> usize {
        self.max_retry_candidates.load(Ordering::Relaxed)
    }

    pub fn set_pool_routing_policy(
        &self,
        policy: crate::PoolRoutingPolicy,
        max_retry_candidates: u8,
    ) -> Result<()> {
        self.set_pool_routing_policy_with_key_scopes(policy, max_retry_candidates, &[])?;
        Ok(())
    }

    /// Apply host membership and the corresponding internal key scopes as one
    /// routing transaction. A final dispatch holds scope -> scheduler locks;
    /// taking them in the same order here prevents a send in the gap between
    /// replacing the policy and revoking a removed member's key permission.
    /// Missing keys abort without changing either the policy or any scope.
    pub fn set_pool_routing_policy_with_key_scopes(
        &self,
        policy: crate::PoolRoutingPolicy,
        max_retry_candidates: u8,
        key_scopes: &[(String, CandidateScope)],
    ) -> Result<bool> {
        policy
            .validate_activation()
            .map_err(|message| Error::Validation(message.into()))?;
        if !(1..=8).contains(&max_retry_candidates) {
            return Err(Error::Validation(
                "max retry candidates must be between 1 and 8".into(),
            ));
        }
        let mut updates = key_scopes.iter().collect::<Vec<_>>();
        updates.sort_by(|(left, _), (right, _)| left.cmp(right));
        let mut seen = BTreeSet::new();
        let mut keys = Vec::with_capacity(updates.len());
        for (id, scope) in updates {
            if !seen.insert(id) {
                return Err(Error::Validation(
                    "duplicate gateway key scope update".into(),
                ));
            }
            let Some(key) = self.keys.iter().find(|key| key.enabled && key.id == *id) else {
                return Ok(false);
            };
            keys.push((key, scope));
        }
        let mut locked = Vec::with_capacity(keys.len());
        for (key, scope) in keys {
            locked.push((
                key,
                scope,
                key.scope
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            ));
        }
        let mut scheduler = self.lock_scheduler();
        scheduler.set_pool_routing(policy)?;
        self.max_retry_candidates
            .store(usize::from(max_retry_candidates), Ordering::Relaxed);
        for (key, scope, mut current) in locked {
            if *current != *scope {
                *current = scope.clone();
                key.scope_revision.fetch_add(1, Ordering::AcqRel);
            }
        }
        drop(scheduler);
        self.candidate_availability.notify_waiters();
        Ok(true)
    }

    pub(crate) fn source_recovery_delay_ms(&self, candidate_id: &str) -> Option<u64> {
        self.source_recovery_delays_ms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(candidate_id)
            .copied()
    }

    pub fn set_default_service_tier(&self, tier: DefaultServiceTier) {
        self.default_service_tier_value
            .store(tier.atomic_value(), Ordering::Relaxed);
    }

    pub(crate) fn default_service_tier(&self) -> DefaultServiceTier {
        DefaultServiceTier::from_atomic_value(
            self.default_service_tier_value.load(Ordering::Relaxed),
        )
    }

    /// Applies the operator-selected speed policy. Client-owned API requests
    /// retain an explicit tier at the gateway boundary.
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

    pub(crate) fn model_effective_service_tier(&self, model: &str) -> DefaultServiceTier {
        let requested = self
            .model_service_tier_overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&model.trim().to_ascii_lowercase())
            .copied()
            .unwrap_or_else(|| self.default_service_tier());
        self.project_service_tier_for_model(model, requested)
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

    fn lock_scheduler(&self) -> MutexGuard<'_, PoolScheduler> {
        self.scheduler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Switches the explicitly labelled Excel/Basis Points transport for OAuth
    /// accounts without rebuilding the scheduler. Agent Identity accounts
    /// cannot use this transport. The account candidate, quota state and
    /// concurrency reservation remain unchanged.
    pub fn set_basis_points_enabled(&self, enabled: bool) {
        for account in self.chatgpt_accounts.values() {
            let oauth = account
                .agent_identity
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_none();
            account
                .basis_points_enabled
                .store(enabled && oauth, Ordering::Relaxed);
        }
    }

    /// A host replaces this runtime's routing graph without waiting for
    /// already-served streams to finish. Serialize retirement with final rotation
    /// dispatch, then wake admissions so they do not wait on dead capacity.
    pub fn retire_for_replacement(&self) {
        self.lock_scheduler().retire_for_replacement();
        self.candidate_availability.notify_waiters();
        self.admission_changed.notify_waiters();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccountTransport {
    NativeResponses,
    ExcelBasisPoints,
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

fn basis_points_headers(account_id: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(account_id) {
        let mut value = value;
        value.set_sensitive(true);
        headers.insert(
            HeaderName::from_static("x-openai-account-id"),
            value.clone(),
        );
        headers.insert(HeaderName::from_static("chatgpt-account-id"), value);
    }
    for (name, value) in [
        ("x-basispoints-auth-mode", "chatgpt"),
        ("origin", "https://bps.openai.com"),
        (
            "x-openai-internal-basispoints-client-agent-profile",
            "excel",
        ),
        ("x-openai-internal-basispoints-client-editor", "excel"),
        ("x-openai-internal-basispoints-client-host", "office"),
        ("x-openai-internal-basispoints-client-platform", "excel"),
        ("x-openai-internal-basispoints-client-platform-class", "PC"),
        (
            "x-openai-internal-basispoints-client-product",
            "basispoints-excel-plugin",
        ),
        ("x-openai-internal-basispoints-client-runtime", "desktop"),
        ("x-openai-internal-basispoints-office-host", "Excel"),
        ("x-openai-internal-basispoints-office-platform", "PC"),
        ("x-stainless-arch", "unknown"),
        ("x-stainless-lang", "js"),
        ("x-stainless-os", "Unknown"),
        ("x-stainless-package-version", "6.31.0"),
        ("x-stainless-retry-count", "0"),
        ("x-stainless-runtime", "browser:chrome"),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    headers.insert(
        HeaderName::from_static("user-agent"),
        HeaderValue::from_static("zenith-relay-basispoints"),
    );
    headers.insert(
        HeaderName::from_static("accept-encoding"),
        HeaderValue::from_static("identity"),
    );
    headers
}

impl ChatGptAccountExecutor {
    fn canonical_model(&self, model: &str) -> Option<String> {
        self.model_inventory
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .configured_models
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

fn runtime_client(proxy: Option<&ProxyConfig>) -> Result<reqwest::Client> {
    // A quiet or long generation is still an active request. Reqwest's default
    // has no response/read deadline; retain only the connection timeout above.
    // Metadata and credential operations set their own request-level timeout.
    runtime_client_builder(proxy)
        .http2_adaptive_window(true)
        .build()
        .map_err(Error::from)
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
