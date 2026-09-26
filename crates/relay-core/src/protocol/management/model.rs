use super::{AccountSummary, OperationalStatus, SourceSummary};
use crate::model_metadata::{ModelMetadataCatalog, ReasoningMethod};
use crate::{
    ApiModelPriceOverride, CandidateKind, CandidateRuntimeSnapshot, DefaultServiceTier,
    GatewayRuntime, ImageRequestPrice,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    for model in models {
        let model_id = model.id.clone();
        model.protocol_routes = routes.routes_for(&model_id);
        model.codex_visible = model.enabled
            && crate::codex_model_is_picker_eligible(&model_id)
            && model
                .protocol_routes
                .iter()
                .any(|route| route.client_wire_api == crate::WireApi::Responses);
        let cache_write_route = routes.has_cache_write_pricing(&model_id);
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

/// Applies saved presentation order while placing new models through the
/// catalog's stable release/update ordering. This changes presentation only;
/// routing and eligibility continue to use the live pool evidence.
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogIdentity {
    pub catalog_provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_family: Option<String>,
}

/// Resolve only IDs present in member inventory or saved rules/prices. Keep
/// this independent from the pool's filtered operational model summaries.
pub fn member_model_catalog(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    catalog: &ModelMetadataCatalog,
) -> BTreeMap<String, ModelCatalogIdentity> {
    let source_models = sources.iter().flat_map(|source| {
        source
            .models
            .iter()
            .chain(&source.allowed_models)
            .chain(&source.excluded_models)
            .chain(source.model_price_overrides.keys())
            .chain(source.detected_model_prices.keys())
    });
    let account_models = accounts.iter().flat_map(|account| {
        account
            .models
            .iter()
            .chain(&account.allowed_models)
            .chain(&account.excluded_models)
    });
    source_models
        .chain(account_models)
        .filter_map(|id| {
            let metadata = catalog.resolve(id)?;
            Some((
                id.to_ascii_lowercase(),
                ModelCatalogIdentity {
                    catalog_provider: metadata.provider.clone(),
                    catalog_family: metadata.family.clone(),
                },
            ))
        })
        .collect()
}

/// Order complete member inventories for the editors, including excluded models
/// and members outside the pool. Do not use the filtered public catalog here.
/// This changes only snapshot presentation, never discovery or routing rules.
pub fn apply_member_model_display_order(
    sources: &mut [SourceSummary],
    accounts: &mut [AccountSummary],
    saved_order: &[String],
    catalog: &ModelMetadataCatalog,
) {
    for models in sources
        .iter_mut()
        .map(|source| &mut source.models)
        .chain(accounts.iter_mut().map(|account| &mut account.models))
    {
        *models = catalog.merge_display_order(models.iter(), saved_order);
    }
}

pub fn apply_model_metadata(models: &mut [ModelSummary], catalog: &ModelMetadataCatalog) {
    for model in models {
        // Snapshots can be refreshed in place by callers. Clear the complete
        // presentation projection before applying the new catalog so a model
        // removed from metadata cannot retain fields from an older snapshot.
        let metadata = catalog.resolve(&model.id);
        model.codex_display_name = catalog.codex_display_name(&model.id);
        model.catalog_provider = metadata.map(|metadata| metadata.provider.clone());
        model.catalog_family = metadata.and_then(|metadata| metadata.family.clone());
        model.catalog_name = metadata.and_then(|metadata| metadata.name.clone());
        model.catalog_release_date = metadata.and_then(|metadata| metadata.release_date.clone());
        model.catalog_last_updated = metadata.and_then(|metadata| metadata.last_updated.clone());
        model.catalog_status = metadata.and_then(|metadata| metadata.status.clone());
        let capabilities = catalog.capabilities_for(&model.id);
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

/// Returns whether an enabled pooled API source has a route for this model.
/// Account membership is counted separately by callers.
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
