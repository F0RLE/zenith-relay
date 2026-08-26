use super::ApiEquivalentSummary;
use crate::is_valid_model_id;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::OnceLock};

pub const MAX_MODEL_PRICE_MICRO_USD_PER_MILLION: u64 = 1_000_000_000_000;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRequestPrice {
    pub operation: String,
    pub quality: String,
    pub size: String,
    pub micro_usd: u64,
}

/// Official OpenAI image output prices from the current API pricing guide.
/// Prompt and input-image tokens for edits remain separate usage facts.
pub fn official_image_request_prices(model: &str) -> Vec<ImageRequestPrice> {
    let rows: &[(&str, u64, u64, u64)] = match model.trim().to_ascii_lowercase().as_str() {
        "gpt-image-2" => &[
            ("low", 6_000, 5_000, 5_000),
            ("medium", 53_000, 41_000, 41_000),
            ("high", 211_000, 165_000, 165_000),
        ],
        "gpt-image-1.5" => &[
            ("low", 9_000, 13_000, 13_000),
            ("medium", 34_000, 50_000, 50_000),
            ("high", 133_000, 200_000, 200_000),
        ],
        "gpt-image-1" => &[
            ("low", 11_000, 16_000, 16_000),
            ("medium", 42_000, 63_000, 63_000),
            ("high", 167_000, 250_000, 250_000),
        ],
        "gpt-image-1-mini" => &[
            ("low", 5_000, 6_000, 6_000),
            ("medium", 11_000, 15_000, 15_000),
            ("high", 36_000, 52_000, 52_000),
        ],
        _ => return Vec::new(),
    };
    ["generation", "edit"]
        .into_iter()
        .flat_map(|operation| {
            rows.iter()
                .flat_map(move |(quality, square, portrait, landscape)| {
                    [
                        ("1024x1024", *square),
                        ("1024x1536", *portrait),
                        ("1536x1024", *landscape),
                    ]
                    .into_iter()
                    .map(move |(size, micro_usd)| ImageRequestPrice {
                        operation: operation.to_string(),
                        quality: (*quality).to_string(),
                        size: size.to_string(),
                        micro_usd,
                    })
                })
        })
        .collect()
}

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

/// Price provenance for a compatible API source.
///
/// Account usage never uses this type: account API-equivalent values come
/// from the bundled official catalog only. For API sources, provider-discovered
/// prices win, the official catalog is the next fallback, and an operator's
/// manual value is used only when neither exists.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiModelPriceSources {
    pub provider: Option<ApiModelPriceOverride>,
    pub manual: Option<ApiModelPriceOverride>,
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
        if !is_valid_model_id(model) || !price.is_valid() {
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
    #[serde(alias = "cache_write_input_micro_usd_per_million")]
    cache_write_5m_micro_usd_per_million: Option<u64>,
    #[serde(default)]
    cache_write_1h_micro_usd_per_million: Option<u64>,
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
    estimate_api_equivalent_with_cache_ttl(
        model,
        input_tokens,
        cached_input_tokens,
        None,
        None,
        cache_write_input_tokens,
        output_tokens,
        total_tokens,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn estimate_api_equivalent_with_cache_ttl(
    model: Option<&str>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_write_5m_tokens: Option<u64>,
    cache_write_1h_tokens: Option<u64>,
    unknown_cache_write_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
) -> ApiEquivalentSummary {
    estimate_api_equivalent_with_cache_ttl_and_price_override(
        model,
        input_tokens,
        cached_input_tokens,
        cache_write_5m_tokens,
        cache_write_1h_tokens,
        unknown_cache_write_tokens,
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
    estimate_api_equivalent_with_cache_ttl_and_price_override(
        model,
        input_tokens,
        cached_input_tokens,
        None,
        None,
        cache_write_input_tokens,
        output_tokens,
        total_tokens,
        price_override,
    )
}

#[allow(clippy::obfuscated_if_else, clippy::too_many_arguments)]
pub fn estimate_api_equivalent_with_cache_ttl_and_price_override(
    model: Option<&str>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_write_5m_tokens: Option<u64>,
    cache_write_1h_tokens: Option<u64>,
    unknown_cache_write_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    price_override: Option<ApiModelPriceOverride>,
) -> ApiEquivalentSummary {
    let measured_tokens = input_tokens
        .unwrap_or_default()
        .saturating_add(output_tokens.unwrap_or_default());
    let total_tokens = total_tokens.unwrap_or(measured_tokens).max(measured_tokens);
    let catalog_price = model.and_then(api_model_price);
    if price_override.is_none() && catalog_price.is_none() {
        return ApiEquivalentSummary {
            unpriced_tokens: total_tokens,
            ..Default::default()
        };
    }
    let input_price = price_override
        .map(|price| price.input_micro_usd_per_million)
        .or_else(|| catalog_price.map(|price| price.input_micro_usd_per_million));
    let cached_price = price_override
        .and_then(|price| price.cached_input_micro_usd_per_million)
        .or_else(|| catalog_price.map(|price| price.cached_input_micro_usd_per_million));
    let cache_write_5m_price = price_override
        .and_then(|price| price.cache_write_5m_micro_usd_per_million)
        .or_else(|| catalog_price.and_then(|price| price.cache_write_5m_micro_usd_per_million));
    let cache_write_1h_price = price_override
        .and_then(|price| price.cache_write_1h_micro_usd_per_million)
        .or_else(|| catalog_price.and_then(|price| price.cache_write_1h_micro_usd_per_million));
    let output_price = price_override
        .map(|price| price.output_micro_usd_per_million)
        .or_else(|| catalog_price.map(|price| price.output_micro_usd_per_million));
    let input_tokens = input_tokens.unwrap_or_default();
    let cached_input_tokens = cached_input_tokens.map(|tokens| tokens.min(input_tokens));
    let cache_write_5m_tokens = cache_write_5m_tokens.map(|tokens| {
        tokens.min(input_tokens.saturating_sub(cached_input_tokens.unwrap_or_default()))
    });
    let cache_write_1h_tokens = cache_write_1h_tokens.map(|tokens| {
        tokens.min(
            input_tokens
                .saturating_sub(cached_input_tokens.unwrap_or_default())
                .saturating_sub(cache_write_5m_tokens.unwrap_or_default()),
        )
    });
    let unknown_cache_write_tokens = unknown_cache_write_tokens.map(|tokens| {
        tokens.min(
            input_tokens
                .saturating_sub(cached_input_tokens.unwrap_or_default())
                .saturating_sub(cache_write_5m_tokens.unwrap_or_default())
                .saturating_sub(cache_write_1h_tokens.unwrap_or_default()),
        )
    });
    let uncached_input_tokens = cached_input_tokens.map(|cached| {
        input_tokens
            .saturating_sub(cached)
            .saturating_sub(cache_write_5m_tokens.unwrap_or_default())
            .saturating_sub(cache_write_1h_tokens.unwrap_or_default())
            .saturating_sub(unknown_cache_write_tokens.unwrap_or_default())
    });
    let output_tokens = output_tokens.unwrap_or_default();
    let priced_uncached = input_price.is_some() && uncached_input_tokens.is_some();
    let priced_cached = cached_price.is_some() && cached_input_tokens.is_some();
    let priced_5m = cache_write_5m_price.is_some() && cache_write_5m_tokens.is_some();
    let priced_1h = cache_write_1h_price.is_some() && cache_write_1h_tokens.is_some();
    let priced_output = output_price.is_some() && output_tokens > 0;
    let priced_tokens = priced_uncached
        .then_some(uncached_input_tokens.unwrap_or_default())
        .unwrap_or_default()
        .saturating_add(
            priced_cached
                .then_some(cached_input_tokens.unwrap_or_default())
                .unwrap_or_default(),
        )
        .saturating_add(
            priced_5m
                .then_some(cache_write_5m_tokens.unwrap_or_default())
                .unwrap_or_default(),
        )
        .saturating_add(
            priced_1h
                .then_some(cache_write_1h_tokens.unwrap_or_default())
                .unwrap_or_default(),
        )
        .saturating_add(priced_output.then_some(output_tokens).unwrap_or_default());
    ApiEquivalentSummary {
        micro_usd: token_cost(
            priced_uncached
                .then_some(uncached_input_tokens.unwrap_or_default())
                .unwrap_or_default(),
            input_price.unwrap_or_default(),
        )
        .saturating_add(token_cost(
            priced_cached
                .then_some(cached_input_tokens.unwrap_or_default())
                .unwrap_or_default(),
            cached_price.unwrap_or_default(),
        ))
        .saturating_add(token_cost(
            priced_5m
                .then_some(cache_write_5m_tokens.unwrap_or_default())
                .unwrap_or_default(),
            cache_write_5m_price.unwrap_or_default(),
        ))
        .saturating_add(token_cost(
            priced_1h
                .then_some(cache_write_1h_tokens.unwrap_or_default())
                .unwrap_or_default(),
            cache_write_1h_price.unwrap_or_default(),
        ))
        .saturating_add(token_cost(
            priced_output.then_some(output_tokens).unwrap_or_default(),
            output_price.unwrap_or_default(),
        )),
        priced_tokens,
        unpriced_tokens: total_tokens.saturating_sub(priced_tokens),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn estimate_api_equivalent_with_cache_ttl_and_price_sources(
    model: Option<&str>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_write_5m_tokens: Option<u64>,
    cache_write_1h_tokens: Option<u64>,
    unknown_cache_write_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
    price_sources: Option<ApiModelPriceSources>,
) -> ApiEquivalentSummary {
    let official_price_exists = model.and_then(api_model_price).is_some();
    let price_override = price_sources.and_then(|sources| {
        sources
            .provider
            .or_else(|| (!official_price_exists).then_some(sources.manual).flatten())
    });
    estimate_api_equivalent_with_cache_ttl_and_price_override(
        model,
        input_tokens,
        cached_input_tokens,
        cache_write_5m_tokens,
        cache_write_1h_tokens,
        unknown_cache_write_tokens,
        output_tokens,
        total_tokens,
        price_override,
    )
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
                cache_write_5m_micro_usd_per_million: price.cache_write_5m_micro_usd_per_million,
                cache_write_1h_micro_usd_per_million: price.cache_write_1h_micro_usd_per_million,
                output_micro_usd_per_million: price.output_micro_usd_per_million,
            })
        })
}

pub fn api_pricing_revision() -> &'static str {
    price_catalog()
        .map(|catalog| catalog.catalog_version.as_str())
        .unwrap_or("unavailable")
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
                            .cache_write_5m_micro_usd_per_million
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
    fn official_image_prices_are_request_and_size_based() {
        let prices = official_image_request_prices("GPT-IMAGE-2");
        assert_eq!(prices.len(), 18);
        assert_eq!(prices[0].operation, "generation");
        assert_eq!(prices[0].quality, "low");
        assert_eq!(prices[0].size, "1024x1024");
        assert_eq!(prices[0].micro_usd, 6_000);
        assert_eq!(prices[9].operation, "edit");
        assert!(official_image_request_prices("gpt-5.6").is_empty());
    }

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
        assert_eq!(catalog.catalog_version, "openai-standard-2026-08-25");
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
    fn gpt_56_official_catalog_prices_match_current_rates() {
        let expected = [
            ("gpt-5.6-sol", 4_000_000, 400_000, 5_000_000, 20_000_000),
            ("gpt-5.6-terra", 2_000_000, 200_000, 2_500_000, 12_000_000),
            ("gpt-5.6-luna", 200_000, 20_000, 250_000, 1_200_000),
        ];

        for (model, input, cached_input, cache_write, output) in expected {
            let price = api_model_price(model).expect("GPT-5.6 model is in the official catalog");
            assert_eq!(price.input_micro_usd_per_million, input, "{model}");
            assert_eq!(
                price.cached_input_micro_usd_per_million, cached_input,
                "{model} cached input"
            );
            assert_eq!(
                price.cache_write_5m_micro_usd_per_million,
                Some(cache_write),
                "{model} cache write"
            );
            assert_eq!(price.output_micro_usd_per_million, output, "{model}");
        }

        let legacy = api_model_price("gpt-5.5").expect("GPT-5.5 is in the official catalog");
        assert_eq!(legacy.input_micro_usd_per_million, 5_000_000);
        assert_eq!(legacy.cached_input_micro_usd_per_million, 500_000);
        assert_eq!(legacy.cache_write_5m_micro_usd_per_million, None);
        assert_eq!(legacy.output_micro_usd_per_million, 30_000_000);
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
            2_840_000
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
                micro_usd: 0,
                priced_tokens: 0,
                unpriced_tokens: 100,
            }
        );
        assert!(api_model_price("gpt-future-codex").is_none());
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
                    micro_usd: 1_060_000,
                    priced_tokens: 1_000_000,
                    unpriced_tokens: 100_000,
                }
            );
        }
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
            estimate_api_equivalent_with_cache_ttl_and_price_override(
                Some("private-model"),
                Some(160),
                Some(40),
                Some(20),
                None,
                None,
                Some(10),
                Some(170),
                Some(price),
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
        let usage = (Some(1_000_000), Some(0), Some(1_000_000), Some(2_000_000));
        assert_eq!(
            estimate_api_equivalent_with_cache_ttl_and_price_sources(
                Some("gpt-5.4"),
                usage.0,
                usage.1,
                None,
                None,
                None,
                usage.2,
                Some(usage.0.unwrap() + usage.2.unwrap()),
                Some(ApiModelPriceSources {
                    provider: Some(provider),
                    manual: Some(manual),
                }),
            )
            .micro_usd,
            3_000_000
        );
        assert_eq!(
            estimate_api_equivalent_with_cache_ttl_and_price_sources(
                Some("gpt-5.4"),
                usage.0,
                usage.1,
                None,
                None,
                None,
                usage.2,
                Some(usage.0.unwrap() + usage.2.unwrap()),
                Some(ApiModelPriceSources {
                    provider: None,
                    manual: Some(manual),
                }),
            ),
            estimate_api_equivalent(
                Some("gpt-5.4"),
                usage.0,
                usage.1,
                None,
                usage.2,
                Some(2_000_000)
            )
        );
        assert_eq!(
            estimate_api_equivalent_with_cache_ttl_and_price_sources(
                Some("private-model"),
                usage.0,
                usage.1,
                None,
                None,
                None,
                usage.2,
                Some(usage.0.unwrap() + usage.2.unwrap()),
                Some(ApiModelPriceSources {
                    provider: None,
                    manual: Some(manual),
                }),
            )
            .micro_usd,
            18_000_000
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
