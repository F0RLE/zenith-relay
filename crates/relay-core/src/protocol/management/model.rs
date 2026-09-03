use super::{AccountSummary, OperationalStatus, SourceSummary};
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
    pub catalog_rank: Option<u32>,
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
    /// Compatibility flag for older management clients. The current pool
    /// exposes the same Normal/Fast policy for every listed model.
    #[serde(default)]
    pub speed_supported: bool,
    #[serde(default)]
    pub speed_tier: DefaultServiceTier,
    #[serde(default)]
    pub speed_configurable: bool,
}

pub fn apply_model_speed_summary(
    model: &mut ModelSummary,
    configured_tier: Option<DefaultServiceTier>,
) {
    model.speed_supported = true;
    model.speed_configurable = true;
    model.speed_tier = configured_tier.unwrap_or(DefaultServiceTier::Standard);
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
        if let Some(price) = model_price_overrides.get(&model_id.trim().to_ascii_lowercase()) {
            model.input_micro_usd_per_million = Some(price.input_micro_usd_per_million);
            model.cached_input_micro_usd_per_million = price.cached_input_micro_usd_per_million;
            model.cache_write_5m_micro_usd_per_million = price.cache_write_5m_micro_usd_per_million;
            model.cache_write_1h_micro_usd_per_million = price.cache_write_1h_micro_usd_per_million;
            model.output_micro_usd_per_million = Some(price.output_micro_usd_per_million);
            model.custom_price = true;
        }
        let has_api_source_route = model_has_api_source_route(sources, &model_id);
        let has_pool_route =
            has_api_source_route || super::model_has_native_account_route(accounts, &model_id);
        apply_model_reasoning_summary(
            model,
            runtime.and_then(|runtime| runtime.source_declared_reasoning_levels(&model_id)),
            crate::reasoning_policy_levels(model_reasoning_allowed_levels, &model_id),
            has_pool_route,
        );
        apply_model_speed_summary(
            model,
            model_service_tier_overrides
                .get(&model_id.to_ascii_lowercase())
                .copied(),
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
    let positions = saved_order
        .iter()
        .enumerate()
        .map(|(position, model)| (model.trim().to_ascii_lowercase(), position))
        .filter(|(model, _)| !model.is_empty())
        .collect::<BTreeMap<_, _>>();
    models.sort_by_key(|model| {
        positions
            .get(&model.id.to_ascii_lowercase())
            .copied()
            .unwrap_or(usize::MAX)
    });
}

/// Adds provider-reported defaults and the operator's manual override to a
/// pooled management model. Provider-reported modes are enabled until the
/// operator edits the list; a present empty override disables them all. The
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

    // Provider metadata is the current route contract. Known model defaults
    // are only a fallback for providers that omit the field entirely; using
    // them first hides newly introduced/provider-specific efforts.
    let mut declared_levels = match reported_levels {
        Some(levels) => levels,
        None => crate::known_model_reasoning_levels(&model.id)
            .map(|levels| levels.iter().copied().map(str::to_string).collect())
            .unwrap_or_default(),
    };
    if crate::anthropic_max_implies_ultra(&model.id)
        && declared_levels
            .iter()
            .any(|level| level.eq_ignore_ascii_case("max"))
        && !declared_levels
            .iter()
            .any(|level| level.eq_ignore_ascii_case("ultra"))
    {
        declared_levels.push("ultra".to_string());
    }
    model.reasoning_supported_levels = crate::canonicalize_reasoning_levels(declared_levels);
    if has_pool_route {
        let effective_levels = saved_manual_levels.unwrap_or(&model.reasoning_supported_levels);
        model.reasoning_allowed_levels = crate::canonicalize_reasoning_levels(effective_levels);
        model.reasoning_levels = model.reasoning_allowed_levels.clone();
        model.reasoning_allowed_levels.retain(|level| {
            model
                .reasoning_supported_levels
                .iter()
                .any(|supported| supported.eq_ignore_ascii_case(level))
                || saved_manual_levels.is_some()
        });
        model.reasoning_levels = model.reasoning_allowed_levels.clone();
    }
    model.reasoning_configurable = has_pool_route;
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
