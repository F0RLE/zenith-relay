use crate::accounts::{
    TokenAuthority, TokenDispatchRevision, TokenPersistenceAdapter, TokenRefreshAdapter,
};
use crate::model_metadata::ModelMetadataCatalogHandle;
use crate::pricing::PricingCatalog;
use crate::protocol::ClientWireApi;
use crate::providers::chatgpt::{AgentIdentityCredential, CodexIdentityEnvelope};
#[cfg(test)]
use crate::providers::chatgpt::{RuntimeChatGptAccount, RuntimeChatGptAuth};
use crate::quota::QuotaSnapshot;
use crate::scheduler::CooldownRequest;
use crate::sources::discover_models_with_client;
use crate::ProxyConfig;
#[cfg(test)]
use crate::SourceProtocolBinding;
use crate::{
    decode_codex_model_alias, CacheWriteTtl, CandidateScope, Error, LocalGatewayKey,
    MessagesReasoningMode, ModelRegistry, ModelRules, NativeResponsesReplayStore, PoolScheduler,
    ProviderSource, Result, RoutingDiagnostics, SourceAdapter, SourceConnector,
    SourceProtocolBindingKey, UsageCallback, WireApi,
};
#[cfg(test)]
use build::ReachabilityRequirement;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

#[cfg(test)]
use reqwest::StatusCode;
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
mod authentication;
mod authorization;
mod build;
mod candidates;
mod clients;
mod codex_metadata;
mod config;
mod control;
mod images;
mod models;
mod routing_cookies;
mod selection;
mod session_state;
mod settings;
mod source_metadata;
mod support;

pub(in crate::runtime) use support::{
    all_native_wire_apis, apply_candidate_policy, basis_points_headers, client_wire_apis_to_native,
    model_rules, normalize_client_wire_api, normalize_prefix, normalized_responses_url,
    normalized_set, parse_bearer, require_runtime_value, runtime_client, runtime_now_ms,
    runtime_websocket_client, strip_prefix_ignore_ascii_case,
};

use config::source_candidate_id;
pub use config::{
    changed_runtime_source_policy_updates, normalize_model_service_tier_overrides,
    pool_catalog_visibility_changed, pool_dispatch_permission_changed, DefaultServiceTier,
    GatewayRuntimeOptions, PoolAccess, PoolParticipant, ResponseAffinityBinding,
    ResponseAffinityStore, RuntimeActivitySnapshot, RuntimeCandidatePolicy, RuntimeLocalKey,
    RuntimeMixedLocalKey, RuntimeSource, RuntimeSourcePolicyRecord, RuntimeSourcePolicyUpdate,
    RESPONSE_AFFINITY_DELETE_CANDIDATE_SQL, RESPONSE_AFFINITY_DELETE_EXPIRED_SQL,
    RESPONSE_AFFINITY_DELETE_SQL, RESPONSE_AFFINITY_FIND_SQL, RESPONSE_AFFINITY_UPSERT_SQL,
};

pub(crate) use attempt::CandidateLease;
use attempt::CandidateLeaseLane;
pub use attempt::ExecutionFence;
use control::RuntimeControl;
pub(crate) use session_state::CodexTurnStateScope;
use session_state::CodexTurnStateStore;

#[cfg(test)]
use crate::{
    normalize_source_protocol_bindings, unix_time_ms as current_time_ms, CandidateKind, UsageEvent,
};
pub(crate) use images::is_image_model_id;
pub use images::normalize_image_base_model;
#[cfg(test)]
use images::{cheapest_image_main_model, select_image_main_model};

/// Relay's virtual Image API model for OAuth account routes.
///
/// The account bridge sends this model in the Responses `image_generation`
/// tool while the top-level Responses model remains the account's selected
/// text model. Keep the public virtual id aligned with the current stable
/// GPT Image model; provider routes still use the model selected by their own
/// source catalog.
pub(crate) const IMAGE_API_MODEL: &str = "gpt-image-2.5-sunburst";
const MAX_IDLE_CONNECTIONS_PER_HOST: usize = 256;
pub(crate) const WEBSOCKET_CAPABILITY_TTL_MS: u64 = 5 * 60 * 1_000;
const CHATGPT_TEAM_BREAKER_DEDUP_MS: u64 = 60 * 1_000;
static NEXT_ACTIVITY_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);

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
    hidden_models: Arc<RwLock<BTreeSet<String>>>,
    scheduler: Arc<Mutex<PoolScheduler>>,
    candidate_availability: Arc<tokio::sync::Notify>,
    admission: Mutex<admission::AdmissionQueue>,
    admission_changed: tokio::sync::Notify,
    registry: Mutex<ModelRegistry>,
    image_base_model: Option<String>,
    image_pricing_catalog: Option<Arc<PricingCatalog>>,
    codex_responses_lite_models: Mutex<BTreeSet<(String, String)>>,
    official_codex_ultra: Mutex<BTreeMap<String, Value>>,
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
        crate::poison::read(&self.scope).clone()
    }

    fn scope_read(&self) -> std::sync::RwLockReadGuard<'_, CandidateScope> {
        crate::poison::read(&self.scope)
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
    /// Client contract. The adapter maps it to the upstream protocol.
    pub(crate) client_wire_api: WireApi,
    pub(crate) adapter: SourceAdapter,
    pub(crate) reasoning_mode: MessagesReasoningMode,
    pub(crate) cache_write_ttl: CacheWriteTtl,
    pub(crate) service_tier: DefaultServiceTier,
    pub(crate) upstream_url: Url,
    pub(crate) upstream_headers: HeaderMap,
    pub(crate) account_transport: AccountTransport,
    pub(crate) client_transport: crate::UsageTransport,
    pub(crate) source_model: String,
    /// Source-local capability evidence for the selected route. OAuth account
    /// routes currently rely on the shared reference catalog and keep this
    /// unset until the account endpoint reports model capabilities.
    pub(crate) route_capability: Option<crate::ModelEndpointCapability>,
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
    /// Capability evidence for this exact upstream model route. Reference
    /// metadata remains separate and is merged only during request admission.
    capabilities: BTreeMap<String, crate::ModelEndpointCapability>,
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
    pub async fn discover_models(&self) -> Result<Vec<String>> {
        let source = self.sources.values().next().ok_or_else(|| {
            Error::Validation("at least one provider source is required".to_string())
        })?;
        discover_models_with_client(&self.discovery_client, source, source.protocol_bindings())
            .await
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
        crate::poison::mutex(&self.scheduler)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccountTransport {
    NativeResponses,
    ExcelBasisPoints,
}

impl ChatGptAccountExecutor {
    fn canonical_model(&self, model: &str) -> Option<String> {
        crate::poison::read(&self.model_inventory)
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

#[cfg(test)]
mod tests;
