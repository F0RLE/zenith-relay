use super::*;
use serde::Deserialize;

/// Pool membership body shared by the desktop command and the management API.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PoolMembershipInput {
    #[serde(default)]
    pub account_ids: Vec<String>,
    #[serde(default)]
    pub source_ids: Vec<String>,
    pub in_pool: bool,
}

#[cfg(test)]
pub fn pool_model_summaries(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    hidden_models: &[String],
) -> Vec<ModelSummary> {
    let catalog = PricingCatalog::from_litellm_json(include_str!(
        "../../../tests/fixtures/litellm-prices.json"
    ))
    .expect("pricing fixture must be valid");
    pool_model_summaries_with_pricing(
        sources,
        accounts,
        hidden_models,
        &catalog,
        &PricingContext::default(),
    )
}

/// Builds the complete configured pool inventory, enriched with pricing.
/// Runtime eligibility belongs to protocol routes and request admission, not
/// to this editable catalog: missing credentials or capabilities cannot erase
/// a model's name, order, prices, or saved policy.
pub fn pool_model_summaries_with_pricing(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    hidden_models: &[String],
    catalog: &PricingCatalog,
    context: &PricingContext,
) -> Vec<ModelSummary> {
    let models = collect_pool_models(sources, accounts);
    let mut summaries = models
        .into_values()
        .map(|model| {
            let model_id = model.id.clone();
            let resolved = resolve_pool_model_price(&model, &model_id, catalog, context);
            let quote = resolved.as_ref().and_then(|price| price.quote);
            let enabled = !hidden_models
                .iter()
                .any(|hidden| hidden.eq_ignore_ascii_case(&model_id));
            (
                model.upstream_order,
                model_summary(
                    model_id.clone(),
                    model.members.len(),
                    enabled,
                    quote,
                    catalog.image_request_prices(&model_id),
                ),
            )
        })
        .collect::<Vec<_>>();
    summaries.sort_by_key(|(upstream_order, _)| *upstream_order);
    summaries.into_iter().map(|(_, summary)| summary).collect()
}

/// Returns the provenance represented by the currently eligible pool models.
/// A snapshot can contain several source/account policies, so selecting the
/// first model's source would be misleading; every resolved member contributes
/// to the aggregate and mixed provenance is reported explicitly.
pub fn pool_pricing_source_summary(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    catalog: &PricingCatalog,
    context: &PricingContext,
) -> PricingSourceSummary {
    let mut resolved_sources = Vec::new();
    for model in collect_pool_models(sources, accounts).values() {
        for member in &model.members {
            let Some((kind, candidate_id)) = member.split_once(':') else {
                continue;
            };
            let resolved = context.candidate_price(catalog, kind, candidate_id, Some(&model.id));
            if resolved.quote.is_some() {
                resolved_sources.push(resolved.source);
            } else {
                resolved_sources.push(PriceSource::Unpriced);
            }
        }
    }
    PricingSourceSummary::from_sources(resolved_sources)
}

fn collect_pool_models(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
) -> BTreeMap<String, PoolModel> {
    let mut models = BTreeMap::<String, PoolModel>::new();
    let mut upstream_order = 0usize;
    for source in sources.iter().filter(|source| source.in_pool) {
        let pool_models = crate::normalize_model_ids(
            source.models.iter().chain(
                source
                    .protocol_bindings
                    .iter()
                    .flat_map(|binding| &binding.model_ids),
            ),
        );
        add_member_models(
            &mut models,
            &crate::scheduler::source_member_key(&source.id),
            &pool_models,
            &mut upstream_order,
        );
    }
    for account in accounts.iter().filter(|account| account.in_pool) {
        add_member_models(
            &mut models,
            &crate::scheduler::account_member_key(&account.id),
            &account.models,
            &mut upstream_order,
        );
    }

    models
}

fn model_summary(
    id: String,
    member_count: usize,
    enabled: bool,
    quote: Option<TokenPrice>,
    image_request_prices: Vec<ImageRequestPrice>,
) -> ModelSummary {
    let (
        input_price,
        cache_read_price,
        short_cache_write_price,
        long_cache_write_price,
        output_price,
    ) = quote.map_or((None, None, None, None, None), |price| {
        (
            Some(price.input),
            price.cache_read,
            price.cache_write_5m,
            price.cache_write_1h,
            Some(price.output),
        )
    });
    ModelSummary {
        enabled,
        protocol_routes: Vec::new(),
        codex_visible: enabled && codex_model_is_picker_eligible(&id),
        codex_display_name: codex_model_display_name(&id),
        id,
        member_count,
        catalog_provider: None,
        catalog_source_model_id: None,
        catalog_canonical_model_id: None,
        catalog_family: None,
        catalog_name: None,
        catalog_release_date: None,
        catalog_last_updated: None,
        catalog_status: None,
        catalog_reasoning: None,
        catalog_reasoning_method: None,
        catalog_reasoning_effort_levels: Vec::new(),
        catalog_default_reasoning_effort: None,
        catalog_reasoning_budget_min_tokens: None,
        catalog_reasoning_budget_max_tokens: None,
        catalog_reasoning_budget_default_tokens: None,
        catalog_tool_call: None,
        catalog_structured_output: None,
        catalog_attachment: None,
        catalog_open_weights: None,
        catalog_input_modalities: Vec::new(),
        catalog_output_modalities: Vec::new(),
        catalog_context_limit: None,
        catalog_input_limit: None,
        catalog_output_limit: None,
        input_micro_usd_per_million: input_price,
        cached_input_micro_usd_per_million: cache_read_price,
        cache_write_5m_micro_usd_per_million: short_cache_write_price,
        cache_write_1h_micro_usd_per_million: long_cache_write_price,
        output_micro_usd_per_million: output_price,
        image_request_prices,
        custom_price: false,
        reasoning_levels: Vec::new(),
        reasoning_supported_levels: Vec::new(),
        reasoning_allowed_levels: Vec::new(),
        reasoning_configurable: false,
        reasoning_manual_fallback: false,
        speed_supported: false,
        speed_tiers: Vec::new(),
        speed_tier: DefaultServiceTier::Standard,
        speed_configurable: false,
    }
}

fn resolve_pool_model_price(
    model: &PoolModel,
    model_id: &str,
    catalog: &PricingCatalog,
    context: &PricingContext,
) -> Option<ResolvedPrice> {
    let mut fallback = None;
    let mut resolved: Option<ResolvedPrice> = None;
    for member in &model.members {
        let Some((kind, candidate_id)) = member.split_once(':') else {
            continue;
        };
        let candidate = context.candidate_price(catalog, kind, candidate_id, Some(model_id));
        if let Some(candidate_quote) = candidate.quote {
            if let Some(mut current) = resolved {
                let current_quote = current
                    .quote
                    .expect("a resolved pool price always has a quote");
                let mut quote = current_quote;
                // The same public model can be exposed by a generic route
                // and an Anthropic Messages route. Preserve the primary
                // route's price while filling cache-write fields only
                // from the route-aware Messages evidence.
                quote.cache_write_5m = current_quote
                    .cache_write_5m
                    .or(candidate_quote.cache_write_5m);
                quote.cache_write_1h = current_quote
                    .cache_write_1h
                    .or(candidate_quote.cache_write_1h);
                if quote.flex.is_empty() {
                    quote.flex = candidate_quote.flex;
                }
                if quote.priority.is_empty() {
                    quote.priority = candidate_quote.priority;
                }
                if quote.above_200k.is_empty() {
                    quote.above_200k = candidate_quote.above_200k;
                }
                if quote.above_272k.is_empty() {
                    quote.above_272k = candidate_quote.above_272k;
                }
                current.quote = Some(quote);
                resolved = Some(current);
            } else {
                resolved = Some(candidate);
            }
        } else {
            fallback = Some(candidate);
        }
    }
    resolved.or(fallback)
}

struct PoolModel {
    id: String,
    members: BTreeSet<String>,
    upstream_order: usize,
}

fn add_member_models(
    models: &mut BTreeMap<String, PoolModel>,
    member_id: &str,
    member_models: &[String],
    upstream_order: &mut usize,
) {
    for model in member_models {
        let model_order = *upstream_order;
        *upstream_order = upstream_order.saturating_add(1);
        let key = crate::model_id_key(model);
        let pool_model = models.entry(key).or_insert_with(|| PoolModel {
            id: model.clone(),
            members: BTreeSet::new(),
            upstream_order: model_order,
        });
        pool_model.members.insert(member_id.to_string());
    }
}
