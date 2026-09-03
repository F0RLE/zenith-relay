use super::ApiEquivalentSummary;
use crate::{
    is_valid_model_id,
    pricing::{
        PriceSource, PricingCatalog, PricingContext, ResolvedPrice, TokenPrice,
        MAX_MODEL_PRICE_MICRO_USD_PER_MILLION,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiModelPriceOverride {
    pub input_micro_usd_per_million: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_micro_usd_per_million: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_5m_micro_usd_per_million: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_micro_usd_per_million: Option<u64>,
    pub output_micro_usd_per_million: u64,
}

/// Price provenance for a compatible API source.
///
/// Account usage never uses this type as a catalog: account API-equivalent
/// values come from the declared-family LiteLLM snapshot. For API sources,
/// provider-discovered prices win, LiteLLM exact/canonical records are the
/// next fallbacks, and an operator's manual value is used only when neither
/// exists.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiModelPriceSources {
    pub provider: Option<ApiModelPriceOverride>,
    pub manual: Option<ApiModelPriceOverride>,
}

/// Per-source price provenance indexed by source id and normalized model id.
///
/// This remains a storage-neutral representation so desktop telemetry and the
/// user-managed server apply the same pricing policy without sharing a schema.
pub type SourceModelPriceOverrides = BTreeMap<String, BTreeMap<String, ApiModelPriceSources>>;

/// Token measurements required to estimate an API-equivalent value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApiEquivalentUsage {
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_5m_tokens: Option<u64>,
    pub cache_write_1h_tokens: Option<u64>,
    pub unknown_cache_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

impl ApiModelPriceOverride {
    pub fn from_optional_fields(
        input: Option<u64>,
        cached_input: Option<u64>,
        cache_write_5m: Option<u64>,
        cache_write_1h: Option<u64>,
        output: Option<u64>,
    ) -> Result<Option<Self>, &'static str> {
        match (input, cached_input, cache_write_5m, cache_write_1h, output) {
            (Some(input), cached_input, cache_write_5m, cache_write_1h, Some(output)) => {
                let price = Self {
                    input_micro_usd_per_million: input,
                    cached_input_micro_usd_per_million: cached_input,
                    cache_write_5m_micro_usd_per_million: cache_write_5m,
                    cache_write_1h_micro_usd_per_million: cache_write_1h,
                    output_micro_usd_per_million: output,
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
        self.input_micro_usd_per_million <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION
            && self
                .cached_input_micro_usd_per_million
                .is_none_or(|value| value <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION)
            && self
                .cache_write_5m_micro_usd_per_million
                .is_none_or(|value| value <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION)
            && self
                .cache_write_1h_micro_usd_per_million
                .is_none_or(|value| value <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION)
            && self.output_micro_usd_per_million <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION
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
        }
    }
}

pub fn normalize_model_price_overrides(
    prices: BTreeMap<String, ApiModelPriceOverride>,
) -> Result<BTreeMap<String, ApiModelPriceOverride>, &'static str> {
    let mut normalized = BTreeMap::new();
    for (model, price) in prices {
        let model = model.trim();
        if !is_valid_model_id(model) || !price.is_valid() {
            return Err("model price override is invalid");
        }
        normalized.insert(model.to_ascii_lowercase(), price);
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
    let input = usage.input_tokens;
    let output = usage.output_tokens;
    let cached = usage
        .cached_input_tokens
        .map(|value| value.min(input.unwrap_or(value)));
    let write_5m = usage
        .cache_write_5m_tokens
        .map(|value| value.min(input.unwrap_or(value)));
    let write_1h = usage
        .cache_write_1h_tokens
        .map(|value| value.min(input.unwrap_or(value)));
    let unknown = usage
        .unknown_cache_write_tokens
        .map(|value| value.min(input.unwrap_or(value)));

    let (uncached, cached, write_5m, write_1h, unknown) = if let Some(input) = input {
        let mut remaining = input;
        let cached = cached.map(|value| value.min(remaining)).unwrap_or_default();
        remaining = remaining.saturating_sub(cached);
        let write_5m = write_5m
            .map(|value| value.min(remaining))
            .unwrap_or_default();
        remaining = remaining.saturating_sub(write_5m);
        let write_1h = write_1h
            .map(|value| value.min(remaining))
            .unwrap_or_default();
        remaining = remaining.saturating_sub(write_1h);
        let unknown = unknown
            .map(|value| value.min(remaining))
            .unwrap_or_default();
        remaining = remaining.saturating_sub(unknown);
        (Some(remaining), cached, write_5m, write_1h, unknown)
    } else {
        (
            None,
            cached.unwrap_or_default(),
            write_5m.unwrap_or_default(),
            write_1h.unwrap_or_default(),
            unknown.unwrap_or_default(),
        )
    };

    let measured_input = if input.is_some() {
        input.unwrap_or_default()
    } else {
        cached
            .saturating_add(write_5m)
            .saturating_add(write_1h)
            .saturating_add(unknown)
    };
    let measured_tokens = measured_input.saturating_add(output.unwrap_or_default());
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

    let components = [
        (uncached, Some(quote.input)),
        (Some(cached), quote.cache_read),
        (Some(write_5m), quote.cache_write_5m),
        (Some(write_1h), quote.cache_write_1h),
        (Some(unknown), None),
        (output, Some(quote.output)),
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
pub fn resolve_candidate_price(
    catalog: &PricingCatalog,
    candidate_kind: &str,
    model: Option<&str>,
    provider_family: Option<&str>,
    pricing_provider: Option<&str>,
    provider_price: Option<ApiModelPriceOverride>,
    manual_price: Option<ApiModelPriceOverride>,
) -> ResolvedPrice {
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

#[allow(clippy::too_many_arguments)]
pub fn estimate_api_equivalent_with_catalog(
    catalog: &PricingCatalog,
    candidate_kind: &str,
    model: Option<&str>,
    usage: ApiEquivalentUsage,
    provider_family: Option<&str>,
    pricing_provider: Option<&str>,
    provider_price: Option<ApiModelPriceOverride>,
    manual_price: Option<ApiModelPriceOverride>,
) -> (ApiEquivalentSummary, ResolvedPrice) {
    let resolved = resolve_candidate_price(
        catalog,
        candidate_kind,
        model,
        provider_family,
        pricing_provider,
        provider_price,
        manual_price,
    );
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

fn token_cost(tokens: u64, micro_usd_per_million: u64) -> u64 {
    let numerator = u128::from(tokens)
        .saturating_mul(u128::from(micro_usd_per_million))
        .saturating_add(500_000);
    u64::try_from(numerator / 1_000_000).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod pricing_tests {
    use super::*;

    fn fixture_catalog() -> PricingCatalog {
        PricingCatalog::from_litellm_json(include_str!("../../tests/fixtures/litellm-prices.json"))
            .expect("pricing fixture must be valid")
    }

    fn fixture_account_estimate(model: &str, usage: ApiEquivalentUsage) -> ApiEquivalentSummary {
        estimate_api_equivalent_with_catalog(
            &fixture_catalog(),
            "account",
            Some(model),
            usage,
            Some("openai"),
            None,
            None,
            None,
        )
        .0
    }

    #[test]
    fn image_prices_come_from_the_litellm_fixture_without_invented_dimensions() {
        let catalog = fixture_catalog();
        let prices = catalog.image_request_prices("GPT-IMAGE-2");
        assert_eq!(prices.len(), 2);
        assert_eq!(prices[0].operation, "generation");
        assert_eq!(prices[0].quality, "default");
        assert_eq!(prices[0].size, "default");
        assert_eq!(prices[0].micro_usd, 6_000);
        assert_eq!(prices[1].operation, "edit");
        assert!(catalog.image_request_prices("gpt-5.6").is_empty());
    }

    #[test]
    fn litellm_catalog_prices_known_models_without_floating_point() {
        let catalog = fixture_catalog();
        let usage = ApiEquivalentUsage {
            input_tokens: Some(1_000_000),
            cached_input_tokens: Some(400_000),
            output_tokens: Some(100_000),
            total_tokens: Some(1_100_000),
            ..Default::default()
        };
        let estimate = fixture_account_estimate("gpt-5.4", usage);
        assert_eq!(estimate.micro_usd, 3_100_000);
        assert_eq!(estimate.priced_tokens, 1_100_000);
        assert_eq!(estimate.unpriced_tokens, 0);
        assert!(catalog.rank_for("GPT-5.4").is_some());
        assert_eq!(
            catalog
                .resolve_account("GPT-5.4", Some("openai"))
                .quote
                .unwrap()
                .cache_read,
            Some(250_000)
        );
        assert_eq!(
            catalog
                .resolve_account("gpt-5.4-mini", Some("openai"))
                .quote
                .unwrap()
                .cache_read,
            Some(75_000)
        );
    }

    #[test]
    fn gpt_56_official_catalog_prices_match_current_rates() {
        let expected = [
            ("gpt-5.6-sol", 4_000_000, 400_000, 5_000_000, 20_000_000),
            ("gpt-5.6-terra", 2_000_000, 200_000, 2_500_000, 12_000_000),
            ("gpt-5.6-luna", 200_000, 20_000, 250_000, 1_200_000),
        ];

        let catalog = fixture_catalog();
        for (model, input, cached_input, cache_write, output) in expected {
            let price = catalog
                .resolve_account(model, Some("openai"))
                .quote
                .expect("GPT-5.6 model is in the fixture catalog");
            assert_eq!(price.input, input, "{model}");
            assert_eq!(price.cache_read, Some(cached_input), "{model} cached input");
            assert_eq!(
                price.cache_write_5m,
                Some(cache_write),
                "{model} cache write"
            );
            assert_eq!(price.output, output, "{model}");
        }

        let legacy = catalog
            .resolve_account("gpt-5.5", Some("openai"))
            .quote
            .expect("GPT-5.5 is in the fixture catalog");
        assert_eq!(legacy.input, 5_000_000);
        assert_eq!(legacy.cache_read, Some(500_000));
        assert_eq!(legacy.cache_write_5m, None);
        assert_eq!(legacy.output, 30_000_000);
    }

    #[test]
    fn unknown_or_unsplit_usage_is_never_silently_priced() {
        assert_eq!(
            fixture_account_estimate(
                "private-model",
                ApiEquivalentUsage {
                    input_tokens: Some(2),
                    cached_input_tokens: Some(1),
                    output_tokens: Some(3),
                    total_tokens: Some(5),
                    ..Default::default()
                },
            ),
            ApiEquivalentSummary {
                micro_usd: 0,
                priced_tokens: 0,
                unpriced_tokens: 5
            }
        );
        assert_eq!(
            fixture_account_estimate(
                "gpt-5.4",
                ApiEquivalentUsage {
                    total_tokens: Some(9),
                    ..Default::default()
                },
            ),
            ApiEquivalentSummary {
                micro_usd: 0,
                priced_tokens: 0,
                unpriced_tokens: 9
            }
        );
        assert_eq!(
            fixture_account_estimate(
                "gpt-5.4",
                ApiEquivalentUsage {
                    input_tokens: Some(10),
                    cached_input_tokens: Some(100),
                    output_tokens: Some(0),
                    total_tokens: Some(10),
                    ..Default::default()
                },
            )
            .micro_usd,
            3
        );
        assert_eq!(
            fixture_account_estimate(
                "gpt-5.6-sol",
                ApiEquivalentUsage {
                    input_tokens: Some(1_000_000),
                    cached_input_tokens: Some(100_000),
                    cache_write_5m_tokens: Some(200_000),
                    output_tokens: Some(0),
                    total_tokens: Some(1_000_000),
                    ..Default::default()
                },
            )
            .micro_usd,
            3_840_000
        );
        assert_eq!(
            fixture_account_estimate(
                "gpt-5.6-sol",
                ApiEquivalentUsage {
                    input_tokens: Some(100),
                    unknown_cache_write_tokens: Some(20),
                    output_tokens: Some(0),
                    total_tokens: Some(100),
                    ..Default::default()
                },
            ),
            ApiEquivalentSummary {
                micro_usd: 320,
                priced_tokens: 80,
                unpriced_tokens: 20,
            }
        );
        assert_eq!(
            fixture_account_estimate(
                "gpt-5.6-sol",
                ApiEquivalentUsage {
                    input_tokens: Some(100),
                    cache_write_5m_tokens: Some(20),
                    output_tokens: Some(0),
                    total_tokens: Some(100),
                    ..Default::default()
                },
            ),
            ApiEquivalentSummary {
                micro_usd: 420,
                priced_tokens: 100,
                unpriced_tokens: 0,
            }
        );
        assert_eq!(
            fixture_catalog()
                .resolve_account("gpt-future-codex", Some("openai"))
                .source,
            PriceSource::Unpriced
        );
    }

    #[test]
    fn provider_price_override_is_separate_from_account_equivalent() {
        let custom = ApiModelPriceOverride {
            input_micro_usd_per_million: 1_500_000,
            cached_input_micro_usd_per_million: Some(150_000),
            cache_write_5m_micro_usd_per_million: Some(1_875_000),
            cache_write_1h_micro_usd_per_million: Some(3_000_000),
            output_micro_usd_per_million: 2_500_000,
        };
        assert_eq!(
            estimate_api_equivalent_with_token_price(
                ApiEquivalentUsage {
                    input_tokens: Some(1_000_000),
                    cached_input_tokens: Some(400_000),
                    cache_write_5m_tokens: Some(100_000),
                    cache_write_1h_tokens: Some(100_000),
                    output_tokens: Some(100_000),
                    total_tokens: Some(1_100_000),
                    ..Default::default()
                },
                Some(custom.into()),
            ),
            ApiEquivalentSummary {
                micro_usd: 1_397_500,
                priced_tokens: 1_100_000,
                unpriced_tokens: 0,
            }
        );
    }

    #[test]
    fn anthropic_cache_write_is_priced_once_after_input_components_are_split() {
        let price = ApiModelPriceOverride {
            input_micro_usd_per_million: 1_000_000,
            cached_input_micro_usd_per_million: Some(100_000),
            cache_write_5m_micro_usd_per_million: Some(2_000_000),
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: 3_000_000,
        };

        // Anthropic reports 100 uncached + 40 cache-read + 20 cache-write
        // tokens as input_tokens=160. Each component must be charged exactly
        // once rather than charging the aggregate input at the base rate too.
        assert_eq!(
            estimate_api_equivalent_with_token_price(
                ApiEquivalentUsage {
                    input_tokens: Some(160),
                    cached_input_tokens: Some(40),
                    cache_write_5m_tokens: Some(20),
                    output_tokens: Some(10),
                    total_tokens: Some(170),
                    ..Default::default()
                },
                Some(price.into()),
            ),
            ApiEquivalentSummary {
                micro_usd: 174,
                priced_tokens: 170,
                unpriced_tokens: 0,
            }
        );
    }

    #[test]
    fn source_price_provenance_is_provider_then_official_then_manual() {
        let provider = ApiModelPriceOverride {
            input_micro_usd_per_million: 1_000_000,
            cached_input_micro_usd_per_million: Some(100_000),
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: 2_000_000,
        };
        let manual = ApiModelPriceOverride {
            input_micro_usd_per_million: 9_000_000,
            cached_input_micro_usd_per_million: Some(900_000),
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: 9_000_000,
        };
        let catalog = fixture_catalog();
        let usage = ApiEquivalentUsage {
            input_tokens: Some(1_000_000),
            cached_input_tokens: Some(0),
            output_tokens: Some(1_000_000),
            total_tokens: Some(2_000_000),
            ..Default::default()
        };
        let (provider_estimate, provider_resolved) = estimate_api_equivalent_with_catalog(
            &catalog,
            "source",
            Some("gpt-5.4"),
            usage,
            None,
            None,
            Some(provider),
            Some(manual),
        );
        assert_eq!(provider_resolved.source, PriceSource::Provider);
        assert_eq!(provider_estimate.micro_usd, 3_000_000);

        let (exact_estimate, exact_resolved) = estimate_api_equivalent_with_catalog(
            &catalog,
            "source",
            Some("gpt-5.4"),
            usage,
            None,
            None,
            None,
            Some(manual),
        );
        assert_eq!(exact_resolved.source, PriceSource::LiteLlmExact);
        assert_eq!(exact_estimate.micro_usd, 17_500_000);

        let (manual_estimate, manual_resolved) = estimate_api_equivalent_with_catalog(
            &catalog,
            "source",
            Some("private-model"),
            usage,
            None,
            None,
            None,
            Some(manual),
        );
        assert_eq!(manual_resolved.source, PriceSource::Manual);
        assert_eq!(manual_estimate.micro_usd, 18_000_000);
    }

    #[test]
    fn candidate_pricing_keeps_account_and_source_rules_separate() {
        let provider = ApiModelPriceOverride {
            input_micro_usd_per_million: 1_000_000,
            cached_input_micro_usd_per_million: Some(100_000),
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: 2_000_000,
        };
        let manual = ApiModelPriceOverride {
            input_micro_usd_per_million: 9_000_000,
            cached_input_micro_usd_per_million: Some(900_000),
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: 9_000_000,
        };
        let catalog = fixture_catalog();
        let context = PricingContext {
            source_evidence: BTreeMap::from([(
                "source-1".to_string(),
                BTreeMap::from([(
                    "private-model".to_string(),
                    crate::pricing::PriceEvidence {
                        provider: Some(provider.into()),
                        manual: None,
                    },
                )]),
            )]),
            global_manual_prices: BTreeMap::from([("private-model".to_string(), manual.into())]),
            ..Default::default()
        };
        let usage = ApiEquivalentUsage {
            input_tokens: Some(1_000_000),
            cached_input_tokens: Some(0),
            output_tokens: Some(1_000_000),
            total_tokens: Some(2_000_000),
            ..Default::default()
        };

        let source_price =
            context.candidate_price(&catalog, "source", "source-1", Some("PRIVATE-MODEL"));
        assert_eq!(source_price.source, PriceSource::Provider);
        assert_eq!(source_price.quote, Some(provider.into()));
        let fallback_price =
            context.candidate_price(&catalog, "source", "missing-source", Some("private-model"));
        assert_eq!(fallback_price.source, PriceSource::Manual);
        assert_eq!(fallback_price.quote, Some(manual.into()));
        let account_price =
            context.candidate_price(&catalog, "account", "account-1", Some("private-model"));
        assert_eq!(account_price.source, PriceSource::Unpriced);

        let (source_estimate, _) = estimate_candidate_api_equivalent_with_catalog(
            &catalog,
            &context,
            "source",
            "source-1",
            Some("private-model"),
            usage,
        );
        assert_eq!(
            source_estimate,
            ApiEquivalentSummary {
                micro_usd: 3_000_000,
                priced_tokens: 2_000_000,
                unpriced_tokens: 0,
            }
        );
        let (account_estimate, _) = estimate_candidate_api_equivalent_with_catalog(
            &catalog,
            &context,
            "account",
            "account-1",
            Some("private-model"),
            usage,
        );
        assert_eq!(
            account_estimate,
            ApiEquivalentSummary {
                micro_usd: 0,
                priced_tokens: 0,
                unpriced_tokens: 2_000_000,
            }
        );
    }

    #[test]
    fn price_override_input_is_normalized_and_validated_once() {
        let price = ApiModelPriceOverride::from_optional_fields(
            Some(1_400_000),
            None,
            Some(2_100_000),
            Some(4_200_000),
            Some(7_000_000),
        )
        .unwrap()
        .unwrap();
        assert_eq!(price.cached_input_micro_usd_per_million, None);
        assert!(ApiModelPriceOverride::from_optional_fields(
            Some(MAX_MODEL_PRICE_MICRO_USD_PER_MILLION + 1),
            None,
            None,
            None,
            Some(1),
        )
        .is_err());

        let normalized = normalize_model_price_overrides(BTreeMap::from([(
            " Claude-Opus-4-8 ".to_string(),
            price,
        )]))
        .unwrap();
        assert_eq!(normalized.get("claude-opus-4-8"), Some(&price));
    }
}
