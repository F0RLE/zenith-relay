use crate::model_metadata::ModelMetadataCatalogHandle;
use crate::pricing::PricingCatalog;
use crate::protocol::ClientWireApi;
use crate::{
    is_valid_model_id, model_id_key, LocalGatewayKey, ProviderSource, SourceProtocolBinding,
    WireApi,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

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

pub(super) fn source_candidate_id(
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

/// Account or API-source fields that control dispatch and catalog visibility.
/// Priority and weight are routing policy, not permission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PoolAccess<'a> {
    pub enabled: bool,
    pub in_pool: bool,
    pub draining: bool,
    pub allowed_models: &'a [String],
    pub excluded_models: &'a [String],
}

/// Storage records expose the fields that control dispatch and catalog visibility.
pub trait PoolParticipant {
    fn pool_access(&self) -> PoolAccess<'_>;
}

/// Pending work must be fenced when membership or model permission changes.
pub fn pool_dispatch_permission_changed(previous: PoolAccess<'_>, next: PoolAccess<'_>) -> bool {
    previous.enabled != next.enabled
        || previous.in_pool != next.in_pool
        || previous.draining != next.draining
        || previous.allowed_models != next.allowed_models
        || previous.excluded_models != next.excluded_models
}

/// A client catalog changes only for a participant that is or was in the pool.
pub fn pool_catalog_visibility_changed(previous: PoolAccess<'_>, next: PoolAccess<'_>) -> bool {
    (previous.in_pool || next.in_pool) && pool_dispatch_permission_changed(previous, next)
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
        normalized.insert(model_id_key(model), tier);
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

impl ResponseAffinityBinding {
    /// Builds a binding from a stored SQLite expiry.
    /// A negative or overflowing value becomes zero, matching other counters.
    pub fn from_stored_expiry(key: String, candidate_id: String, expires_at_ms: i64) -> Self {
        Self {
            key,
            candidate_id,
            expires_at_ms: crate::usage::sql_count_u64(expires_at_ms),
        }
    }
}

pub const RESPONSE_AFFINITY_DELETE_EXPIRED_SQL: &str =
    "DELETE FROM response_affinity WHERE expires_at_ms <= ?1";
pub const RESPONSE_AFFINITY_FIND_SQL: &str =
    "SELECT response_key, candidate_id, expires_at_ms FROM response_affinity WHERE response_key = ?1 AND expires_at_ms > ?2";
pub const RESPONSE_AFFINITY_UPSERT_SQL: &str =
    "INSERT INTO response_affinity(response_key, candidate_id, expires_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(response_key) DO UPDATE SET candidate_id = excluded.candidate_id, expires_at_ms = excluded.expires_at_ms, updated_at_ms = excluded.updated_at_ms";
pub const RESPONSE_AFFINITY_DELETE_SQL: &str =
    "DELETE FROM response_affinity WHERE response_key = ?1";
pub const RESPONSE_AFFINITY_DELETE_CANDIDATE_SQL: &str =
    "DELETE FROM response_affinity WHERE candidate_id = ?1";

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

#[cfg(test)]
mod pool_access_tests {
    use super::{pool_catalog_visibility_changed, pool_dispatch_permission_changed, PoolAccess};

    fn access<'a>(in_pool: bool, allowed: &'a [String]) -> PoolAccess<'a> {
        PoolAccess {
            enabled: true,
            in_pool,
            draining: false,
            allowed_models: allowed,
            excluded_models: &[],
        }
    }

    #[test]
    fn pool_permission_changes_dispatch_and_only_visible_members_change_catalog() {
        let allowed = vec!["gpt-test".to_string()];
        let outside = access(false, &allowed);
        let inside = access(true, &allowed);
        assert!(pool_dispatch_permission_changed(outside, inside));
        assert!(pool_catalog_visibility_changed(outside, inside));

        let renamed = PoolAccess {
            enabled: false,
            ..outside
        };
        assert!(pool_dispatch_permission_changed(outside, renamed));
        assert!(!pool_catalog_visibility_changed(outside, renamed));
    }
}
