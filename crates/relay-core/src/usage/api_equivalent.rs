use super::ApiEquivalentSummary;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::OnceLock};

pub const MAX_MODEL_PRICE_MICRO_USD_PER_MILLION: u64 = 1_000_000_000_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiModelPrice {
    pub catalog_rank: u32,
    pub input_micro_usd_per_million: u64,
    pub cached_input_micro_usd_per_million: u64,
    pub cache_write_5m_micro_usd_per_million: Option<u64>,
    pub cache_write_1h_micro_usd_per_million: Option<u64>,
    pub output_micro_usd_per_million: u64,
}

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
                    cached_input_micro_usd_per_million: Some(cached_input.unwrap_or(input)),
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

pub fn normalize_model_price_overrides(
    prices: BTreeMap<String, ApiModelPriceOverride>,
) -> Result<BTreeMap<String, ApiModelPriceOverride>, &'static str> {
    let mut normalized = BTreeMap::new();
    for (model, price) in prices {
        let model = model.trim();
        if model.is_empty()
            || model.len() > 256
            || model.chars().any(char::is_control)
            || !price.is_valid()
        {
            return Err("model price override is invalid");
        }
        normalized.insert(model.to_ascii_lowercase(), price);
    }
    Ok(normalized)
}

#[derive(Deserialize)]
struct PriceCatalog {
    schema_version: u32,
    catalog_version: String,
    source_url: String,
    verified_at: String,
    currency: String,
    unit_tokens: u64,
    models: Vec<ModelPrice>,
}

#[derive(Deserialize)]
struct ModelPrice {
    id: String,
    input_micro_usd_per_million: u64,
    cached_input_micro_usd_per_million: Option<u64>,
    cache_write_input_micro_usd_per_million: Option<u64>,
    output_micro_usd_per_million: u64,
}

pub fn estimate_api_equivalent(
    model: Option<&str>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
) -> ApiEquivalentSummary {
    estimate_api_equivalent_with_price_override(
        model,
        input_tokens,
        cached_input_tokens,
        cache_write_input_tokens,
        output_tokens,
        total_tokens,
        None,
    )
}

pub fn estimate_api_equivalent_with_price_override(
    model: Option<&str>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    price_override: Option<ApiModelPriceOverride>,
) -> ApiEquivalentSummary {
    let measured_tokens = input_tokens
        .unwrap_or_default()
        .saturating_add(output_tokens.unwrap_or_default());
    let total_tokens = total_tokens.unwrap_or(measured_tokens).max(measured_tokens);
    let catalog_price = model.and_then(model_price);
    if price_override.is_none() && catalog_price.is_none() {
        return ApiEquivalentSummary {
            unpriced_tokens: total_tokens,
            ..Default::default()
        };
    }
    let input_price = price_override
        .map(|price| price.input_micro_usd_per_million)
        .or_else(|| catalog_price.map(|price| price.input_micro_usd_per_million))
        .unwrap_or_default();
    let output_price = price_override
        .map(|price| price.output_micro_usd_per_million)
        .or_else(|| catalog_price.map(|price| price.output_micro_usd_per_million))
        .unwrap_or_default();
    let input_tokens = input_tokens.unwrap_or_default();
    let cached_input_tokens = cached_input_tokens.map(|tokens| tokens.min(input_tokens));
    let cache_write_input_tokens = cache_write_input_tokens.map(|tokens| {
        tokens.min(input_tokens.saturating_sub(cached_input_tokens.unwrap_or_default()))
    });
    let uncached_input_tokens = cached_input_tokens.map(|cached| {
        input_tokens
            .saturating_sub(cached)
            .saturating_sub(cache_write_input_tokens.unwrap_or_default())
    });
    let output_tokens = output_tokens.unwrap_or_default();
    let priced_input_tokens = cached_input_tokens
        .unwrap_or_default()
        .saturating_add(cache_write_input_tokens.unwrap_or_default())
        .saturating_add(uncached_input_tokens.unwrap_or_default());
    ApiEquivalentSummary {
        micro_usd: token_cost(uncached_input_tokens.unwrap_or_default(), input_price)
            .saturating_add(token_cost(
                cached_input_tokens.unwrap_or_default(),
                price_override.map_or_else(
                    || {
                        catalog_price
                            .and_then(|price| price.cached_input_micro_usd_per_million)
                            .unwrap_or(input_price)
                    },
                    |price| {
                        price
                            .cached_input_micro_usd_per_million
                            .unwrap_or(input_price)
                    },
                ),
            ))
            .saturating_add(token_cost(
                cache_write_input_tokens.unwrap_or_default(),
                price_override.map_or_else(
                    || {
                        catalog_price
                            .and_then(|price| price.cache_write_input_micro_usd_per_million)
                            .unwrap_or(input_price)
                    },
                    |price| {
                        price
                            .cache_write_5m_micro_usd_per_million
                            .unwrap_or(input_price)
                    },
                ),
            ))
            .saturating_add(token_cost(output_tokens, output_price)),
        priced_tokens: priced_input_tokens.saturating_add(output_tokens),
        unpriced_tokens: total_tokens
            .saturating_sub(priced_input_tokens.saturating_add(output_tokens)),
    }
}

pub fn api_model_price(model: &str) -> Option<ApiModelPrice> {
    price_catalog()?
        .models
        .iter()
        .enumerate()
        .find(|(_, price)| price.id.eq_ignore_ascii_case(model))
        .and_then(|(rank, price)| {
            Some(ApiModelPrice {
                catalog_rank: u32::try_from(rank).ok()?,
                input_micro_usd_per_million: price.input_micro_usd_per_million,
                cached_input_micro_usd_per_million: price
                    .cached_input_micro_usd_per_million
                    .unwrap_or(price.input_micro_usd_per_million),
                cache_write_5m_micro_usd_per_million: price.cache_write_input_micro_usd_per_million,
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: price.output_micro_usd_per_million,
            })
        })
}

pub fn api_pricing_revision() -> &'static str {
    price_catalog()
        .map(|catalog| catalog.catalog_version.as_str())
        .unwrap_or("unavailable")
}

fn model_price(model: &str) -> Option<&'static ModelPrice> {
    price_catalog()?
        .models
        .iter()
        .find(|price| price.id.eq_ignore_ascii_case(model))
}

fn price_catalog() -> Option<&'static PriceCatalog> {
    static CATALOG: OnceLock<Option<PriceCatalog>> = OnceLock::new();
    CATALOG
        .get_or_init(|| {
            serde_json::from_str(include_str!("../../data/openai-api-prices.json")).ok()
        })
        .as_ref()
        .filter(|catalog| {
            catalog.schema_version == 3
                && catalog.unit_tokens == 1_000_000
                && catalog.currency == "USD"
                && !catalog.catalog_version.is_empty()
                && !catalog.source_url.is_empty()
                && !catalog.verified_at.is_empty()
                && catalog.models.iter().all(|price| {
                    price
                        .cached_input_micro_usd_per_million
                        .is_none_or(|cached| cached <= price.input_micro_usd_per_million)
                        && price
                            .cache_write_input_micro_usd_per_million
                            .is_none_or(|write| write > 0)
                })
        })
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

    #[test]
    fn official_catalog_prices_known_models_without_floating_point() {
        let estimate = estimate_api_equivalent(
            Some("gpt-5.4"),
            Some(1_000_000),
            Some(400_000),
            None,
            Some(100_000),
            Some(1_100_000),
        );
        assert_eq!(estimate.micro_usd, 3_100_000);
        assert_eq!(estimate.priced_tokens, 1_100_000);
        assert_eq!(estimate.unpriced_tokens, 0);
        assert_eq!(api_model_price("GPT-5.4").unwrap().catalog_rank, 5);
        assert_eq!(
            api_model_price("GPT-5.4")
                .unwrap()
                .cached_input_micro_usd_per_million,
            250_000
        );
        let catalog = price_catalog().unwrap();
        assert_eq!(catalog.catalog_version, "openai-standard-2026-07-26");
        assert_eq!(
            catalog.source_url,
            "https://developers.openai.com/api/docs/pricing/"
        );
        assert_eq!(
            catalog
                .models
                .iter()
                .find(|model| model.id == "gpt-5.4-mini")
                .unwrap()
                .cached_input_micro_usd_per_million,
            Some(75_000)
        );
    }

    #[test]
    fn unknown_or_unsplit_usage_is_never_silently_priced() {
        assert_eq!(
            estimate_api_equivalent(
                Some("private-model"),
                Some(2),
                Some(1),
                None,
                Some(3),
                Some(5)
            ),
            ApiEquivalentSummary {
                micro_usd: 0,
                priced_tokens: 0,
                unpriced_tokens: 5
            }
        );
        assert_eq!(
            estimate_api_equivalent(Some("gpt-5.4"), None, None, None, None, Some(9)),
            ApiEquivalentSummary {
                micro_usd: 0,
                priced_tokens: 0,
                unpriced_tokens: 9
            }
        );
        assert_eq!(
            estimate_api_equivalent(
                Some("gpt-5.4"),
                Some(10),
                Some(100),
                None,
                Some(0),
                Some(10)
            )
            .micro_usd,
            3
        );
        assert_eq!(
            estimate_api_equivalent(
                Some("gpt-5.6-sol"),
                Some(1_000_000),
                Some(100_000),
                Some(200_000),
                Some(0),
                Some(1_000_000)
            )
            .micro_usd,
            4_550_000
        );
        assert_eq!(
            estimate_api_equivalent(
                Some("gpt-5.6-sol"),
                Some(100),
                None,
                Some(20),
                Some(0),
                Some(100)
            ),
            ApiEquivalentSummary {
                micro_usd: 100,
                priced_tokens: 20,
                unpriced_tokens: 80,
            }
        );
        assert!(api_model_price("gpt-future-codex").is_none());
    }

    #[test]
    fn custom_price_overrides_catalog_and_prices_unknown_models() {
        let custom = ApiModelPriceOverride {
            input_micro_usd_per_million: 1_500_000,
            cached_input_micro_usd_per_million: Some(150_000),
            cache_write_5m_micro_usd_per_million: Some(1_875_000),
            cache_write_1h_micro_usd_per_million: Some(3_000_000),
            output_micro_usd_per_million: 2_500_000,
        };
        for model in ["gpt-5.4", "private-model"] {
            assert_eq!(
                estimate_api_equivalent_with_price_override(
                    Some(model),
                    Some(1_000_000),
                    Some(400_000),
                    Some(100_000),
                    Some(100_000),
                    Some(1_100_000),
                    Some(custom),
                ),
                ApiEquivalentSummary {
                    micro_usd: 1_247_500,
                    priced_tokens: 1_100_000,
                    unpriced_tokens: 0,
                }
            );
        }
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
        assert_eq!(price.cached_input_micro_usd_per_million, Some(1_400_000));
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
