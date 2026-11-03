use crate::pricing::{
    ImageRequestPrice, PriceSource, PricingCatalog, PricingContext, PricingMetadata,
    PricingSourceSummary, ResolvedPrice,
};
use crate::{
    ApiModelPriceOverride, DefaultServiceTier, SourceProtocolBinding, TokenPrice, WireApi,
    automations::{WakeHistory, WakeTask},
    codex_model_display_name, codex_model_is_picker_eligible, normalize_image_base_model,
    normalize_model_ids, normalize_model_price_overrides, normalize_model_reasoning_allowed_levels,
    normalize_model_service_tier_overrides, normalize_source_protocol_bindings,
};
mod account;
mod desktop_capabilities;
mod model;
mod model_policy;
mod model_protocols;
mod preset_routing_reader;
mod routing;
mod usage;

pub use account::{
    AccountRefreshState, AccountSummary, CredentialDiagnostics, CredentialRecovery,
    CredentialStatus, QuotaWindowUsage, RefreshStatus, RevealedAccountIdentity, SourceRefreshState,
    SourceSummary, SourceSummaryRecord, WeeklyValueCalibration, api_equivalent_projection_window,
    model_has_native_account_route,
};
pub use desktop_capabilities::DesktopCapabilities;
pub use model::{
    GatewaySummary, ModelCatalogIdentity, ModelDisplayProjection, ModelSummary,
    PoolModelConfigurationInput, SetModelContextInput, SetModelEnabledInput, SetModelOrderInput,
    SetModelReasoningInput, SetModelServiceTierInput, apply_member_model_display_order,
    apply_model_display_order, apply_model_display_order_with_catalog, apply_model_metadata,
    apply_model_reasoning_summary, apply_model_speed_summary, apply_pool_model_configuration,
    apply_pool_model_configuration_with_context, member_model_catalog, model_has_api_source_route,
    pool_candidate_count, pooled_source_runtime_available, source_runtime_available,
};
pub use model_policy::{
    ModelPolicyError, canonical_pool_model_id, complete_model_display_order,
    configured_source_model_ids, update_model_reasoning_policy,
};
pub use model_protocols::{ModelProtocolRoute, apply_model_protocol_routes};
pub use routing::{
    AccountOperationalInput, AccountOperationalSource, AccountOperationalState,
    AccountRoutingBlockReason, OperationalStatus, ProxyMode, QuotaRefreshStatus,
    account_candidate_enabled, account_operational_state, operational_status, pool_routing_summary,
    quota_refresh_status,
};
pub use usage::{
    UsageBucket, UsageGroup, UsagePage, UsageQuery, UsageRange, UsageSummary, UsageTokenBreakdown,
    UsageTotals, UsageWorkload, merge_model_equivalents,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeStruct};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

mod pool;
mod presets;
mod runtime;

#[cfg(test)]
pub use pool::pool_model_summaries;
pub use pool::{
    PoolMembershipInput, PoolModelProjection, pool_model_projection_with_pricing,
    pool_model_summaries_with_pricing, pool_pricing_source_summary,
};
pub use presets::{
    AccountPresetRule, CONFIGURATION_PRESET_FORMAT, CONFIGURATION_PRESET_SCHEMA_VERSION,
    ConfigurationPreset, ConfigurationPresetApplyInput, ConfigurationPresetApplyResult,
    ConfigurationPresetChange, ConfigurationPresetDocument, ConfigurationPresetPreview,
    ConfigurationPresetPreviewInput, ConfigurationPresetSettings, DEFAULT_MAX_RETRY_CANDIDATES,
    DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS, MAX_MAX_RETRY_CANDIDATES,
    MAX_QUOTA_REQUEST_TIMEOUT_SECONDS, MIN_MAX_RETRY_CANDIDATES, MIN_QUOTA_REQUEST_TIMEOUT_SECONDS,
    PresetQuotaPolicy, PresetRoutingPolicy, SourcePresetRule, max_retry_candidates_in_range,
    merge_configuration_preset_settings, normalize_configuration_preset,
    quota_request_timeout_in_range, validate_resolved_configuration_preset_members,
};
pub use runtime::{ClientWireApi, local_gateway_client_wire_apis};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayDiagnostic {
    pub stream: bool,
    pub model: String,
    pub latency_ms: u64,
    pub bytes_received: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStateSnapshot {
    pub schema_version: u32,
    /// Display-only local history revision; no quota or routing semantics.
    #[serde(default)]
    pub weekly_capacity_revision: u64,
    pub gateway: GatewaySummary,
    pub platform: String,
    pub capabilities: DesktopCapabilities,
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
