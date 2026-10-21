use super::{ApiEquivalentSummary, ApiEquivalentUsage, ApiModelPriceOverride};
use crate::{
    is_valid_model_id, model_id_key,
    pricing::{
        PriceEvidence, PriceSource, PricingCatalog, PricingContext, PricingMetadata,
        PricingSourceSummary, ResolvedPrice, TokenPrice, TokenRateSet,
    },
};
use std::collections::BTreeMap;

impl ApiModelPriceOverride {
    pub fn from_optional_fields(
        input_price: Option<u64>,
        cache_read_price: Option<u64>,
        short_cache_write_price: Option<u64>,
        long_cache_write_price: Option<u64>,
        output_price: Option<u64>,
    ) -> Result<Option<Self>, &'static str> {
        match (
            input_price,
            cache_read_price,
            short_cache_write_price,
            long_cache_write_price,
            output_price,
        ) {
            (
                Some(input_price),
                cache_read_price,
                short_cache_write_price,
                long_cache_write_price,
                Some(output_price),
            ) => {
                let price = Self {
                    input_micro_usd_per_million: input_price,
                    cached_input_micro_usd_per_million: cache_read_price,
                    cache_write_5m_micro_usd_per_million: short_cache_write_price,
                    cache_write_1h_micro_usd_per_million: long_cache_write_price,
                    output_micro_usd_per_million: output_price,
                    ..Default::default()
                };
                price
                    .is_valid()
                    .then_some(Some(price))
                    .ok_or("model prices must be valid")
            }
            (None, None, None, None, None) => Ok(None),
            _ => Err("model prices must be valid"),
        }
    }

    pub fn is_valid(&self) -> bool {
        TokenPrice::from(*self).is_valid()
    }
}

impl From<ApiModelPriceOverride> for TokenPrice {
    fn from(price: ApiModelPriceOverride) -> Self {
        Self {
            input: price.input_micro_usd_per_million,
            // An absent cache tariff is intentionally preserved. In
            // particular, it must never inherit the ordinary input tariff.
            cache_read: price.cached_input_micro_usd_per_million,
            cache_write_5m: price.cache_write_5m_micro_usd_per_million,
            cache_write_1h: price.cache_write_1h_micro_usd_per_million,
            output: price.output_micro_usd_per_million,
            flex: price.flex,
            priority: price.priority,
            ultrafast: price.ultrafast,
            above_100k: price.above_100k,
            above_128k: price.above_128k,
            above_200k: price.above_200k,
            above_272k: price.above_272k,
            above_512k: price.above_512k,
        }
    }
}

impl From<TokenPrice> for ApiModelPriceOverride {
    fn from(price: TokenPrice) -> Self {
        Self {
            input_micro_usd_per_million: price.input,
            cached_input_micro_usd_per_million: price.cache_read,
            cache_write_5m_micro_usd_per_million: price.cache_write_5m,
            cache_write_1h_micro_usd_per_million: price.cache_write_1h,
            output_micro_usd_per_million: price.output,
            flex: price.flex,
            priority: price.priority,
            ultrafast: price.ultrafast,
            above_100k: price.above_100k,
            above_128k: price.above_128k,
            above_200k: price.above_200k,
            above_272k: price.above_272k,
            above_512k: price.above_512k,
        }
    }
}

impl PricingContext {
    /// Turns stored manual and per-source prices into the neutral context
    /// both hosts use for usage estimates. Account families and source
    /// metadata stay with the host because those records are not shared.
    pub fn from_price_overrides(
        price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
        source_price_overrides: &super::SourceModelPriceOverrides,
    ) -> Self {
        let source_evidence = source_price_overrides
            .iter()
            .map(|(source_id, models)| {
                let evidence = models
                    .iter()
                    .map(|(model, prices)| {
                        (
                            model_id_key(model),
                            PriceEvidence {
                                provider: prices.provider.map(Into::into),
                                manual: prices.manual.map(Into::into),
                            },
                        )
                    })
                    .collect();
                (source_id.clone(), evidence)
            })
            .collect();
        let global_manual_prices = price_overrides
            .iter()
            .map(|(model, price)| (model_id_key(model), (*price).into()))
            .collect();
        Self {
            source_evidence,
            global_manual_prices,
            ..Self::default()
        }
    }
}

pub fn normalize_model_price_overrides(
    prices: BTreeMap<String, ApiModelPriceOverride>,
) -> Result<BTreeMap<String, ApiModelPriceOverride>, &'static str> {
    let mut normalized = BTreeMap::new();
    for (model_id, price_override) in prices {
        let model_id = model_id.trim();
        if !is_valid_model_id(model_id) || !price_override.is_valid() {
            return Err("model price override is invalid");
        }
        normalized.insert(model_id_key(model_id), price_override);
    }
    Ok(normalized)
}

/// Estimates token usage from one complete quote. This is the single pure
/// accounting path used by dynamic LiteLLM prices and compatibility overrides.
/// Cache components remain independent: a missing cache tariff makes only the
/// corresponding cache tokens unpriced instead of silently using input cost.
pub fn estimate_api_equivalent_with_token_price(
    usage: ApiEquivalentUsage,
    quote: Option<TokenPrice>,
) -> ApiEquivalentSummary {
    let input_tokens = usage.input_tokens;
    let output_tokens = usage.output_tokens;
    let cache_read_tokens = usage
        .cached_input_tokens
        .map(|cached_tokens| cached_tokens.min(input_tokens.unwrap_or(cached_tokens)));
    let short_cache_write_tokens = usage.cache_write_5m_tokens.map(|cache_write_tokens| {
        cache_write_tokens.min(input_tokens.unwrap_or(cache_write_tokens))
    });
    let long_cache_write_tokens = usage.cache_write_1h_tokens.map(|cache_write_tokens| {
        cache_write_tokens.min(input_tokens.unwrap_or(cache_write_tokens))
    });
    let unknown_cache_write_tokens = usage
        .unknown_cache_write_tokens
        .map(|unknown_cache_tokens| {
            unknown_cache_tokens.min(input_tokens.unwrap_or(unknown_cache_tokens))
        });

    let (
        uncached_input_tokens,
        cache_read_tokens,
        short_cache_write_tokens,
        long_cache_write_tokens,
        unknown_cache_write_tokens,
    ) = if let Some(input_tokens) = input_tokens {
        let mut remaining_input_tokens = input_tokens;
        let cache_read_tokens = cache_read_tokens
            .map(|cached_tokens| cached_tokens.min(remaining_input_tokens))
            .unwrap_or_default();
        remaining_input_tokens = remaining_input_tokens.saturating_sub(cache_read_tokens);
        let short_cache_write_tokens = short_cache_write_tokens
            .map(|cache_write_tokens| cache_write_tokens.min(remaining_input_tokens))
            .unwrap_or_default();
        remaining_input_tokens = remaining_input_tokens.saturating_sub(short_cache_write_tokens);
        let long_cache_write_tokens = long_cache_write_tokens
            .map(|cache_write_tokens| cache_write_tokens.min(remaining_input_tokens))
            .unwrap_or_default();
        remaining_input_tokens = remaining_input_tokens.saturating_sub(long_cache_write_tokens);
        let unknown_cache_write_tokens = unknown_cache_write_tokens
            .map(|unknown_cache_tokens| unknown_cache_tokens.min(remaining_input_tokens))
            .unwrap_or_default();
        remaining_input_tokens = remaining_input_tokens.saturating_sub(unknown_cache_write_tokens);
        (
            Some(remaining_input_tokens),
            cache_read_tokens,
            short_cache_write_tokens,
            long_cache_write_tokens,
            unknown_cache_write_tokens,
        )
    } else {
        (
            None,
            cache_read_tokens.unwrap_or_default(),
            short_cache_write_tokens.unwrap_or_default(),
            long_cache_write_tokens.unwrap_or_default(),
            unknown_cache_write_tokens.unwrap_or_default(),
        )
    };

    let measured_input_tokens = if input_tokens.is_some() {
        input_tokens.unwrap_or_default()
    } else {
        cache_read_tokens
            .saturating_add(short_cache_write_tokens)
            .saturating_add(long_cache_write_tokens)
            .saturating_add(unknown_cache_write_tokens)
    };
    let measured_tokens = measured_input_tokens.saturating_add(output_tokens.unwrap_or_default());
    let total_tokens = usage
        .total_tokens
        .unwrap_or(measured_tokens)
        .max(measured_tokens);

    let Some(quote) = quote else {
        return ApiEquivalentSummary {
            unpriced_tokens: total_tokens,
            ..Default::default()
        };
    };
    let Some(rates) = rates_for(quote, usage.price_class, usage.context_band) else {
        return ApiEquivalentSummary {
            unpriced_tokens: total_tokens,
            ..Default::default()
        };
    };

    let components = [
        (uncached_input_tokens, rates.input),
        (Some(cache_read_tokens), rates.cache_read),
        (Some(short_cache_write_tokens), rates.cache_write_5m),
        (Some(long_cache_write_tokens), rates.cache_write_1h),
        (Some(unknown_cache_write_tokens), None),
        (output_tokens, rates.output),
    ];
    let mut priced_tokens = 0_u64;
    let mut micro_usd = 0_u64;
    for (tokens, price) in components {
        if let (Some(tokens), Some(price)) = (tokens, price) {
            priced_tokens = priced_tokens.saturating_add(tokens);
            micro_usd = micro_usd.saturating_add(token_cost(tokens, price));
        }
    }
    ApiEquivalentSummary {
        micro_usd,
        priced_tokens,
        unpriced_tokens: total_tokens.saturating_sub(priced_tokens),
    }
}

/// Resolves a candidate's price against one immutable catalog snapshot.
/// Account candidates are restricted to their declared official family;
/// source candidates may use provider evidence, an exact LiteLLM record, an
/// explicitly confirmed canonical family, and finally a manual source value.
pub struct CandidatePriceQuery<'a> {
    pub catalog: &'a PricingCatalog,
    pub candidate_kind: &'a str,
    pub model: Option<&'a str>,
    pub provider_family: Option<&'a str>,
    pub pricing_provider: Option<&'a str>,
    pub provider_price: Option<ApiModelPriceOverride>,
    pub manual_price: Option<ApiModelPriceOverride>,
}

pub fn resolve_candidate_price(query: CandidatePriceQuery<'_>) -> ResolvedPrice {
    let CandidatePriceQuery {
        catalog,
        candidate_kind,
        model,
        provider_family,
        pricing_provider,
        provider_price,
        manual_price,
    } = query;
    let Some(model) = model else {
        return ResolvedPrice {
            quote: None,
            source: PriceSource::Unpriced,
            catalog_revision: catalog.revision.clone(),
            catalog_fetched_at_ms: catalog.fetched_at_ms,
            stale: catalog.stale,
        };
    };
    if candidate_kind.eq_ignore_ascii_case("account") {
        return catalog.resolve_account(model, provider_family.or(Some("openai")));
    }
    catalog.resolve_source(
        model,
        pricing_provider,
        provider_family,
        provider_price.map(Into::into),
        manual_price.map(Into::into),
    )
}

pub fn estimate_api_equivalent_with_catalog(
    query: CandidatePriceQuery<'_>,
    usage: ApiEquivalentUsage,
) -> (ApiEquivalentSummary, ResolvedPrice) {
    let resolved = resolve_candidate_price(query);
    let estimate = estimate_api_equivalent_with_token_price(usage, resolved.quote);
    (estimate, resolved)
}

/// Resolves and prices a persisted candidate using the host-provided
/// redacted identity context.  This is the preferred entry point for desktop
/// and server usage queries; all rows in one query should share the same
/// `PricingCatalog` snapshot.
pub fn estimate_candidate_api_equivalent_with_catalog(
    catalog: &PricingCatalog,
    context: &PricingContext,
    candidate_kind: &str,
    candidate_id: &str,
    model: Option<&str>,
    usage: ApiEquivalentUsage,
) -> (ApiEquivalentSummary, ResolvedPrice) {
    let resolved = context.candidate_price(catalog, candidate_kind, candidate_id, model);
    let estimate = estimate_api_equivalent_with_token_price(usage, resolved.quote);
    (estimate, resolved)
}

/// Prices one usage query against one catalog snapshot.
///
/// Desktop and server both keep their own SQL. This type owns only the shared
/// decision: which catalog price applies, what that usage is worth, and which
/// revision key invalidates a derived cache.
pub struct CatalogPriceResolver<'a> {
    catalog: &'a PricingCatalog,
    context: &'a PricingContext,
    revision: String,
}

impl<'a> CatalogPriceResolver<'a> {
    pub fn new(catalog: &'a PricingCatalog, context: &'a PricingContext) -> Self {
        Self {
            catalog,
            context,
            revision: context.revision_key(catalog),
        }
    }

    pub fn estimate(
        &self,
        candidate_kind: &str,
        candidate_id: &str,
        model: Option<&str>,
        usage: ApiEquivalentUsage,
    ) -> ApiEquivalentSummary {
        estimate_candidate_api_equivalent_with_catalog(
            self.catalog,
            self.context,
            candidate_kind,
            candidate_id,
            model,
            usage,
        )
        .0
    }

    pub fn source(
        &self,
        candidate_kind: &str,
        candidate_id: &str,
        model: Option<&str>,
    ) -> PriceSource {
        self.context
            .candidate_price(self.catalog, candidate_kind, candidate_id, model)
            .source
    }

    pub fn revision_key(&self) -> &str {
        &self.revision
    }

    pub fn pricing_metadata(
        &self,
        equivalent_summary: ApiEquivalentSummary,
        sources: &[PriceSource],
    ) -> PricingMetadata {
        PricingMetadata::for_catalog(
            self.catalog,
            PricingSourceSummary::from_sources(sources.iter().copied()),
            equivalent_summary.unpriced_tokens,
        )
    }
}

fn token_cost(tokens: u64, micro_usd_per_million: u64) -> u64 {
    let numerator = u128::from(tokens)
        .saturating_mul(u128::from(micro_usd_per_million))
        .saturating_add(500_000);
    u64::try_from(numerator / 1_000_000).unwrap_or(u64::MAX)
}

/// Picks the published schedule for one tier and prompt band.
///
/// Missing tier prices stay unpriced. Long-context fields overlay that tier's
/// base fields; the highest published threshold exceeded by the request wins.
fn rates_for(
    quote: TokenPrice,
    class: super::UsagePriceClass,
    band: super::UsageContextBand,
) -> Option<TokenRateSet> {
    use super::UsagePriceClass;
    let mut rates = match class {
        UsagePriceClass::Standard => TokenRateSet {
            input: Some(quote.input),
            cache_read: quote.cache_read,
            cache_write_5m: quote.cache_write_5m,
            cache_write_1h: quote.cache_write_1h,
            output: Some(quote.output),
        },
        UsagePriceClass::Flex => quote.flex,
        UsagePriceClass::Priority => quote.priority,
        UsagePriceClass::Ultrafast => quote.ultrafast,
    };
    let above = context_rates(quote, band, class);
    if let Some(above) = above {
        rates = rates.overlay(above);
    }
    (!rates.is_empty()).then_some(rates)
}

fn context_rates(
    quote: TokenPrice,
    band: super::UsageContextBand,
    class: super::UsagePriceClass,
) -> Option<TokenRateSet> {
    quote
        .context_rates()
        .into_iter()
        .rev()
        .filter(|(threshold, _)| *threshold <= band.threshold())
        .find_map(|(_, rates)| band_rates(rates, class))
}

fn band_rates(
    rates: crate::pricing::LongContextRates,
    class: super::UsagePriceClass,
) -> Option<TokenRateSet> {
    use super::UsagePriceClass;
    let rates = match class {
        UsagePriceClass::Standard => rates.standard,
        UsagePriceClass::Flex => rates.flex,
        UsagePriceClass::Priority => rates.priority,
        UsagePriceClass::Ultrafast => rates.ultrafast,
    };
    (!rates.is_empty()).then_some(rates)
}
