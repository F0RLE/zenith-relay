use super::Capabilities;
use crate::pricing::{
    ImageRequestPrice, PriceSource, PricingCatalog, PricingContext, PricingMetadata,
    PricingSourceSummary, ResolvedPrice,
};
use crate::{
    automations::{WakeHistory, WakeTask},
    codex_model_display_name, codex_model_is_picker_eligible, normalize_image_base_model,
    normalize_model_ids, normalize_model_price_overrides, normalize_model_reasoning_allowed_levels,
    normalize_model_service_tier_overrides, normalize_source_protocol_bindings,
    ApiModelPriceOverride, DefaultServiceTier, SourceProtocolBinding, TokenPrice, WireApi,
};
mod account;
mod model;
mod model_policy;
mod model_protocols;
mod preset_routing_reader;
mod routing;
mod usage;

pub use account::{
    api_equivalent_projection_window, model_has_native_account_route, AccountRefreshState,
    AccountSummary, QuotaWindowUsage, RefreshStatus, RemoteAccountLocation,
    RevealedAccountIdentity, SourceRefreshState, SourceSummary,
};
pub use model::{
    apply_member_model_display_order, apply_model_display_order,
    apply_model_display_order_with_catalog, apply_model_metadata, apply_model_reasoning_summary,
    apply_model_speed_summary, apply_pool_model_configuration, member_model_catalog,
    model_has_api_source_route, pool_candidate_count, pooled_source_runtime_available,
    source_runtime_available, GatewaySummary, ModelCatalogIdentity, ModelSummary,
};
pub use model_policy::{
    canonical_pool_model_id, complete_model_display_order, configured_source_model_ids,
    update_model_reasoning_policy, ModelPolicyError,
};
pub use model_protocols::{
    apply_model_protocol_routes, codex_catalog_supports_websockets, ModelProtocolRoute,
};
pub use routing::{
    account_candidate_enabled, account_operational_state, operational_status, pool_routing_summary,
    quota_refresh_status, AccountOperationalInput, AccountOperationalState,
    AccountRoutingBlockReason, OperationalStatus, ProxyMode, QuotaRefreshStatus,
};
pub use usage::{
    assign_bucket_equivalents, merge_model_equivalents, UsageBucket, UsageGroup, UsagePage,
    UsageQuery, UsageRange, UsageSummary, UsageTokenBreakdown, UsageTotals,
};

use serde::{ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

mod health;
mod pool;
mod presets;

pub use health::{
    local_gateway_client_wire_apis, valid_generated_id, ClientWireApi, HealthResponse,
    ProfileKeyRotation, RuntimeTargetSummary, PROFILE_KEY_ROTATION_SCHEMA_VERSION,
};
#[cfg(test)]
pub use pool::pool_model_summaries;
pub use pool::{pool_model_summaries_with_pricing, pool_pricing_source_summary};
pub use presets::{
    max_retry_candidates_in_range, merge_configuration_preset_settings,
    normalize_configuration_preset, quota_request_timeout_in_range,
    validate_resolved_configuration_preset_members, AccountPresetRule, ConfigurationPreset,
    ConfigurationPresetApplyInput, ConfigurationPresetApplyResult, ConfigurationPresetChange,
    ConfigurationPresetDocument, ConfigurationPresetPreview, ConfigurationPresetPreviewInput,
    ConfigurationPresetSettings, PresetQuotaPolicy, PresetRoutingPolicy, SourcePresetRule,
    CONFIGURATION_PRESET_FORMAT, CONFIGURATION_PRESET_SCHEMA_VERSION, DEFAULT_MAX_RETRY_CANDIDATES,
    DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS, MAX_MAX_RETRY_CANDIDATES,
    MAX_QUOTA_REQUEST_TIMEOUT_SECONDS, MIN_MAX_RETRY_CANDIDATES, MIN_QUOTA_REQUEST_TIMEOUT_SECONDS,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayDiagnostic {
    pub stream: bool,
    pub model: String,
    pub latency_ms: u64,
    pub bytes_received: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub stage: String,
    pub retryable: bool,
    pub request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ErrorEnvelope {
    pub error: ApiError,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStateSnapshot {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_revision: Option<String>,
    pub runtime_target: RuntimeTargetSummary,
    pub gateway: GatewaySummary,
    pub platform: String,
    pub capabilities: Capabilities,
    pub sources: Vec<SourceSummary>,
    pub accounts: Vec<AccountSummary>,
    pub automations: Vec<WakeTask>,
    pub wake_history: Vec<WakeHistory>,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub pricing: PricingMetadata,
}

#[cfg(test)]
mod tests;
