use super::{AccountSummary, OperationalStatus, SourceSummary};
use crate::model_metadata::{ModelMetadataCatalog, ReasoningMethod};
use crate::{
    ApiModelPriceOverride, CandidateKind, CandidateRuntimeSnapshot, DefaultServiceTier,
    GatewayRuntime, ImageRequestPrice, RoutingStrategy,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySummary {
    pub running: bool,
    pub base_url: String,
    pub candidate_count: usize,
    pub visible_model_ids: Vec<String>,
    pub max_retry_candidates: u8,
    #[serde(default = "default_cooldown_after_failures")]
    pub cooldown_after_failures: u8,
    #[serde(default = "default_keep_last_candidate_available")]
    pub keep_last_candidate_available: bool,
    pub routing_strategy: RoutingStrategy,
    #[serde(default)]
    pub subscription_plan_order: Vec<String>,
    pub default_service_tier: DefaultServiceTier,
    #[serde(default)]
    pub image_base_model: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelSummary>,
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
    pub chatgpt_retry_until_available: bool,
    #[serde(default)]
    pub routing_order: Vec<CandidateRuntimeSnapshot>,
}

fn default_codex_websockets_enabled() -> bool {
    true
}

fn default_codex_background_tasks_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSummary {
    pub id: String,
    pub enabled: bool,
    pub member_count: usize,
    #[serde(default)]
    pub codex_visible: bool,
    #[serde(default)]
    pub codex_display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_provider: Option<String>,
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
    /// Advisory model metadata from models.dev. These fields describe the
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
    /// The upstream omitted reasoning metadata for an otherwise unknown pooled
    /// model. The management client may offer manual levels for discovery, but
    /// must not treat this as an advertised upstream capability.
    #[serde(default)]
    pub reasoning_manual_fallback: bool,
    /// Set only when a current pool route has confirmed an upstream Fast tier.
    #[serde(default)]
    pub speed_supported: bool,
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
    let supported =
        runtime.is_some_and(|runtime| runtime.model_supports_fast_service_tier(&model.id));
    model.speed_supported = supported;
    model.speed_configurable = supported;
    model.speed_tier = if supported {
        effective_tier
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
    for model in models {
        let model_id = model.id.clone();
        let cache_write_route = sources.iter().any(|source| {
            source
                .models_with_cache_write_pricing()
                .contains(&model_id.to_ascii_lowercase())
        });
        if let Some(price) = model_price_overrides.get(&model_id.trim().to_ascii_lowercase()) {
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
        let has_api_source_route = model_has_api_source_route(sources, &model_id);
        let has_pool_route =
            has_api_source_route || super::model_has_native_account_route(accounts, &model_id);
        // A provider can return an explicit empty reasoning list when its
        // generic `/models` endpoint has no capability metadata. Treat that
        // as an absent declaration for the management projection so the
        // official catalog can still describe a known model. A non-empty
        // provider declaration remains authoritative and is not widened by
        // the catalog fallback.
        let reported_reasoning_levels = runtime
            .and_then(|runtime| runtime.source_declared_reasoning_levels(&model_id))
            .filter(|levels| !levels.is_empty())
            .or_else(|| {
                (!model.catalog_reasoning_effort_levels.is_empty())
                    .then(|| model.catalog_reasoning_effort_levels.clone())
            });
        apply_model_reasoning_summary(
            model,
            reported_reasoning_levels,
            crate::reasoning_policy_levels(model_reasoning_allowed_levels, &model_id),
            has_pool_route,
        );
        apply_model_speed_summary(
            model,
            model_service_tier_overrides
                .get(&model_id.to_ascii_lowercase())
                .copied()
                .or_else(|| runtime.map(GatewayRuntime::default_service_tier))
                .unwrap_or(DefaultServiceTier::Standard),
            runtime,
        );
    }
}

/// Counts pooled source and account candidates that are currently eligible
/// for rotation. This is a snapshot statistic, not scheduler admission.
pub fn pool_candidate_count(sources: &[SourceSummary], accounts: &[AccountSummary]) -> usize {
    sources
        .iter()
        .filter(|source| {
            source.in_pool
                && source.supports_any_wire_api()
                && source.operational_status == OperationalStatus::Rotation
        })
        .count()
        + accounts
            .iter()
            .filter(|account| {
                account.in_pool && account.operational_status == OperationalStatus::Rotation
            })
            .count()
}

/// Applies the operator's explicit presentation order without dropping a
/// newly discovered upstream model. Unknown or stale saved IDs are ignored;
/// models absent from the saved list keep their upstream-relative order.
pub fn apply_model_display_order(models: &mut [ModelSummary], saved_order: &[String]) {
    apply_model_display_order_with_catalog(models, saved_order, &ModelMetadataCatalog::empty());
}

/// Applies saved presentation order and uses the external metadata catalog
/// only for newly discovered models. Runtime routing is never consulted here.
pub fn apply_model_display_order_with_catalog(
    models: &mut [ModelSummary],
    saved_order: &[String],
    catalog: &ModelMetadataCatalog,
) {
    let order = catalog.merge_display_order(models.iter().map(|model| &model.id), saved_order);
    let positions = order
        .iter()
        .enumerate()
        .map(|(position, model)| (model.to_ascii_lowercase(), position))
        .collect::<BTreeMap<_, _>>();
    models.sort_by_key(|model| {
        positions
            .get(&model.id.to_ascii_lowercase())
            .copied()
            .unwrap_or(usize::MAX)
    });
}

pub fn apply_model_metadata(models: &mut [ModelSummary], catalog: &ModelMetadataCatalog) {
    for model in models {
        // Snapshots can be refreshed in place by callers. Clear the complete
        // presentation projection before applying the new catalog so a model
        // removed from metadata cannot retain fields from an older snapshot.
        let metadata = catalog.resolve(&model.id);
        model.catalog_provider = metadata.map(|metadata| metadata.provider.clone());
        model.catalog_family = metadata.and_then(|metadata| metadata.family.clone());
        model.catalog_name = metadata.and_then(|metadata| metadata.name.clone());
        model.catalog_release_date = metadata.and_then(|metadata| metadata.release_date.clone());
        model.catalog_last_updated = metadata.and_then(|metadata| metadata.last_updated.clone());
        model.catalog_status = metadata.and_then(|metadata| metadata.status.clone());
        let capabilities = metadata
            .map(|metadata| metadata.capabilities.clone())
            .unwrap_or_else(crate::model_metadata::ModelCapabilities::unknown_model);
        model.catalog_reasoning = capabilities.reasoning;
        model.catalog_reasoning_method = capabilities.reasoning_method;
        model.catalog_reasoning_effort_levels = capabilities.reasoning_effort_levels;
        model.catalog_default_reasoning_effort = capabilities.default_reasoning_effort;
        model.catalog_tool_call = capabilities.tool_call;
        model.catalog_structured_output = capabilities.structured_output;
        model.catalog_attachment = capabilities.attachment;
        model.catalog_open_weights = capabilities.open_weights;
        model.catalog_input_modalities = capabilities.input_modalities;
        model.catalog_output_modalities = capabilities.output_modalities;
        model.catalog_context_limit = capabilities.context_limit;
        model.catalog_input_limit = capabilities.input_limit;
        model.catalog_output_limit = capabilities.output_limit;
    }
}

/// Applies models.dev reasoning levels and a narrowing operator override to a
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

/// Returns whether an eligible pooled API source can serve this model through
/// any confirmed client contract. Native account capabilities stay owned by
/// their upstream catalog and are deliberately excluded from manual settings.
pub fn model_has_api_source_route(sources: &[SourceSummary], model: &str) -> bool {
    sources.iter().any(|source| {
        source.enabled
            && source.in_pool
            && !source.draining
            && source.secret_available
            && source
                .models_for_any_wire_api()
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(model))
    })
}

pub fn source_runtime_available(
    routing_order: &[CandidateRuntimeSnapshot],
    source_id: &str,
) -> bool {
    routing_order.iter().any(|candidate| {
        candidate.kind == CandidateKind::ApiSource
            && candidate.available
            && (candidate.candidate_id == source_id
                || candidate
                    .candidate_id
                    .strip_prefix(source_id)
                    .is_some_and(|suffix| suffix.starts_with("::")))
    })
}

/// Returns whether an API source has any healthy runtime route exposed through
/// the pool's multi-protocol system key. Candidate ids may be the legacy source
/// id or a protocol-specific child such as `source::messages`.
pub fn pooled_source_runtime_available(
    routing_order: &[CandidateRuntimeSnapshot],
    source_id: &str,
) -> bool {
    source_runtime_available(routing_order, source_id)
}

fn default_cooldown_after_failures() -> u8 {
    2
}

fn default_keep_last_candidate_available() -> bool {
    true
}
