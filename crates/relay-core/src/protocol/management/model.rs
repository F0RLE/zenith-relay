use super::{AccountSummary, OperationalStatus, SourceSummary};
use crate::model_metadata::ReasoningMethod;
use crate::{
    ApiModelPriceOverride, CandidateRuntimeSnapshot, DefaultServiceTier, GatewayRuntime,
    ImageRequestPrice,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod availability;
mod order;

pub use availability::{
    model_has_api_source_route, pool_candidate_count, pooled_source_runtime_available,
    source_runtime_available,
};
pub use order::{
    apply_member_model_display_order, apply_model_display_order,
    apply_model_display_order_with_catalog, apply_model_metadata, member_model_catalog,
    ModelCatalogIdentity,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySummary {
    #[serde(default)]
    pub tool_policy: crate::ToolPolicy,
    #[serde(default)]
    pub basis_points_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_routing: Option<crate::PoolRoutingPolicy>,
    pub running: bool,
    pub base_url: String,
    pub candidate_count: usize,
    pub visible_model_ids: Vec<String>,
    pub max_retry_candidates: u8,
    pub default_service_tier: DefaultServiceTier,
    #[serde(default)]
    pub image_base_model: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelSummary>,
    /// Advisory identities for the complete member inventory, including models
    /// excluded by member rules. This map never grants runtime eligibility.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_catalog: BTreeMap<String, ModelCatalogIdentity>,
    #[serde(default)]
    pub common_proxy_configured: bool,
    #[serde(default)]
    pub common_proxy_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub common_proxy_id: Option<String>,
    #[serde(default)]
    pub account_proxy_required: bool,
    #[serde(default)]
    pub quota_request_timeout_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chatgpt_interface_quota_reserve_basis_points: Option<u64>,
    #[serde(default = "default_codex_background_tasks_enabled")]
    pub codex_background_tasks_enabled: bool,
    #[serde(default = "default_codex_websockets_enabled")]
    pub codex_websockets_enabled: bool,
    #[serde(default)]
    /// Legacy snapshot key retained for older desktop/server clients.
    pub chatgpt_retry_until_available: bool,
    #[serde(default = "default_block_degraded_routes_enabled")]
    pub block_degraded_routes_enabled: bool,
    #[serde(default)]
    pub routing_order: Vec<CandidateRuntimeSnapshot>,
}

fn default_codex_websockets_enabled() -> bool {
    true
}

fn default_codex_background_tasks_enabled() -> bool {
    true
}

fn default_block_degraded_routes_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSummary {
    pub id: String,
    #[serde(default)]
    pub protocol_routes: Vec<super::ModelProtocolRoute>,
    pub enabled: bool,
    pub member_count: usize,
    #[serde(default)]
    pub codex_visible: bool,
    #[serde(default)]
    pub codex_display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_provider: Option<String>,
    /// Reference identity only. It does not rewrite the source model ID or
    /// make a hosted alias executable on another provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_source_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_canonical_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_last_updated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_status: Option<String>,
    /// Shared reference metadata and Relay defaults. These fields describe the
    /// model family, but never grant runtime access to a source or account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_reasoning_method: Option<ReasoningMethod>,
    #[serde(default)]
    pub catalog_reasoning_effort_levels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_default_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_reasoning_budget_min_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_reasoning_budget_max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_reasoning_budget_default_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_tool_call: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_structured_output: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_attachment: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_open_weights: Option<bool>,
    #[serde(default)]
    pub catalog_input_modalities: Vec<String>,
    #[serde(default)]
    pub catalog_output_modalities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_context_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_input_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_output_limit: Option<u64>,
    pub input_micro_usd_per_million: Option<u64>,
    pub cached_input_micro_usd_per_million: Option<u64>,
    #[serde(default)]
    pub cache_write_5m_micro_usd_per_million: Option<u64>,
    #[serde(default)]
    pub cache_write_1h_micro_usd_per_million: Option<u64>,
    pub output_micro_usd_per_million: Option<u64>,
    #[serde(default)]
    pub image_request_prices: Vec<ImageRequestPrice>,
    #[serde(default)]
    pub custom_price: bool,
    #[serde(default)]
    pub reasoning_levels: Vec<String>,
    #[serde(default)]
    pub reasoning_supported_levels: Vec<String>,
    #[serde(default)]
    pub reasoning_allowed_levels: Vec<String>,
    #[serde(default)]
    pub reasoning_configurable: bool,
    /// Legacy wire field; always false. Unknown reasoning enums are not guessed.
    #[serde(default)]
    pub reasoning_manual_fallback: bool,
    /// The Relay model-family policy offers faster request modes.
    #[serde(default)]
    pub speed_supported: bool,
    /// Relay-owned request choices; source tier declarations and runtime health
    /// do not change the policy.
    #[serde(default)]
    pub speed_tiers: Vec<DefaultServiceTier>,
    #[serde(default)]
    pub speed_tier: DefaultServiceTier,
    #[serde(default)]
    pub speed_configurable: bool,
}

pub fn apply_model_speed_summary(
    model: &mut ModelSummary,
    effective_tier: DefaultServiceTier,
    runtime: Option<&GatewayRuntime>,
) {
    let speed_tiers = runtime
        .map(|runtime| runtime.model_supported_service_tiers(&model.id))
        .unwrap_or_else(|| {
            crate::catalog::model_service_tiers(&model.id, model.catalog_provider.as_deref())
        });
    let supported = speed_tiers
        .iter()
        .any(|tier| *tier != DefaultServiceTier::Standard);
    model.speed_tiers = speed_tiers.to_vec();
    model.speed_supported = supported;
    model.speed_configurable = supported;
    model.speed_tier = if supported {
        runtime
            .map(|runtime| runtime.model_effective_service_tier(&model.id))
            .unwrap_or(effective_tier)
    } else {
        DefaultServiceTier::Standard
    };
}

/// Applies the same configured pool policy to every runtime snapshot. Local
/// desktop and user-managed server snapshots share this projection, while
/// their storage and runtime lifecycle remain separate.
pub fn apply_pool_model_configuration(
    models: &mut [ModelSummary],
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    model_price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    model_reasoning_allowed_levels: &BTreeMap<String, Vec<String>>,
    model_service_tier_overrides: &BTreeMap<String, DefaultServiceTier>,
    runtime: Option<&GatewayRuntime>,
) {
    let routes = super::model_protocols::ModelProtocolIndex::new(sources, accounts);
    let block_degraded_routes = runtime.is_none_or(GatewayRuntime::block_degraded_routes_enabled);
    for model in models {
        let model_id = model.id.clone();
        model.protocol_routes = routes.routes_for(&model_id);
        model.codex_visible = model.enabled
            && crate::codex_model_is_picker_eligible_for(&model_id, block_degraded_routes)
            && model
                .protocol_routes
                .iter()
                .any(|route| route.client_wire_api == crate::WireApi::Responses);
        let cache_write_route = routes.has_cache_write_pricing(&model_id);
        if let Some(price) = model_price_overrides.get(&crate::model_id_key(&model_id)) {
            model.input_micro_usd_per_million = Some(price.input_micro_usd_per_million);
            model.cached_input_micro_usd_per_million = price.cached_input_micro_usd_per_million;
            model.cache_write_5m_micro_usd_per_million = cache_write_route
                .then_some(price.cache_write_5m_micro_usd_per_million)
                .flatten();
            model.cache_write_1h_micro_usd_per_million = cache_write_route
                .then_some(price.cache_write_1h_micro_usd_per_million)
                .flatten();
            model.output_micro_usd_per_million = Some(price.output_micro_usd_per_million);
            model.custom_price = true;
        }
        let reported_reasoning_levels = Some(runtime.map_or_else(
            || model.catalog_reasoning_effort_levels.clone(),
            |runtime| runtime.model_reasoning_levels(&model_id),
        ));
        let features = runtime.map_or_else(
            || {
                crate::model_metadata::ModelCapabilities {
                    reasoning: model.catalog_reasoning,
                    tool_call: model.catalog_tool_call,
                    structured_output: model.catalog_structured_output,
                    attachment: model.catalog_attachment,
                    ..Default::default()
                }
                .with_defaults()
                .protocol_features()
            },
            |runtime| runtime.model_capabilities(&model_id).protocol_features(),
        );
        for route in &mut model.protocol_routes {
            route.features = features.clone();
            route.reasoning_efforts.clear();
            route.project_reasoning(reported_reasoning_levels.as_deref().unwrap_or_default());
        }
        // Model Rules edits inventory, including offline or excluded members.
        // Keep declared modes intact here; each executable route above and the
        // request resolver apply their own protocol-specific restrictions.
        apply_model_reasoning_summary(
            model,
            reported_reasoning_levels,
            crate::reasoning_policy_levels(model_reasoning_allowed_levels, &model_id),
            true,
        );
        apply_model_speed_summary(
            model,
            model_service_tier_overrides
                .get(&crate::model_id_key(&model_id))
                .copied()
                .or_else(|| runtime.map(GatewayRuntime::default_service_tier))
                .unwrap_or(DefaultServiceTier::Standard),
            runtime,
        );
    }
}

/// Applies shared reference reasoning levels and a narrowing operator override to a
/// pooled management model. A present empty override disables all levels. The
/// route flag covers both native OAuth accounts and API sources.
pub fn apply_model_reasoning_summary(
    model: &mut ModelSummary,
    reported_levels: Option<Vec<String>>,
    saved_manual_levels: Option<&[String]>,
    has_pool_route: bool,
) {
    model.reasoning_levels.clear();
    model.reasoning_supported_levels.clear();
    model.reasoning_allowed_levels.clear();
    model.reasoning_configurable = false;
    model.reasoning_manual_fallback = false;

    let declared_levels = reported_levels.unwrap_or_default();
    model.reasoning_supported_levels = crate::canonicalize_reasoning_levels(declared_levels);
    // A reasoning boolean is not an effort enum. Keep the levels unknown until
    // an automatic metadata source declares them explicitly.
    if has_pool_route {
        let effective_levels = saved_manual_levels.unwrap_or(&model.reasoning_supported_levels);
        model.reasoning_allowed_levels = crate::canonicalize_reasoning_levels(effective_levels);
        model.reasoning_levels = model.reasoning_allowed_levels.clone();
        model.reasoning_allowed_levels.retain(|level| {
            model
                .reasoning_supported_levels
                .iter()
                .any(|supported| supported.eq_ignore_ascii_case(level))
        });
        model.reasoning_levels = model.reasoning_allowed_levels.clone();
    }
    model.reasoning_configurable = has_pool_route && !model.reasoning_supported_levels.is_empty();
}

/// Model-setting bodies shared by the desktop commands and the management API.
///
/// Names stay camelCase and unknown fields are rejected, so both hosts keep the
/// same request contract.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelEnabledInput {
    pub model_id: String,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelPriceInput {
    pub model_id: String,
    pub input_micro_usd_per_million: Option<u64>,
    pub cached_input_micro_usd_per_million: Option<u64>,
    pub cache_write_5m_micro_usd_per_million: Option<u64>,
    pub cache_write_1h_micro_usd_per_million: Option<u64>,
    pub output_micro_usd_per_million: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelReasoningInput {
    pub model_id: String,
    #[serde(default)]
    pub allowed_levels: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelServiceTierInput {
    pub model_id: String,
    pub service_tier: DefaultServiceTier,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelOrderInput {
    pub model_ids: Vec<String>,
}
