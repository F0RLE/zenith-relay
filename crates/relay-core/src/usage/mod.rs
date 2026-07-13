use crate::WireApi;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

pub type UsageCallback = Arc<dyn Fn(UsageEvent) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    pub request_id: String,
    pub attempt: u16,
    pub local_key_id: String,
    pub source_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub wire_api: WireApi,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consecutive_failures: Option<u32>,
    pub latency_ms: u64,
    pub ttft_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiEquivalentSummary {
    pub micro_usd: u64,
    pub priced_tokens: u64,
    pub unpriced_tokens: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiModelPrice {
    pub catalog_rank: u32,
    pub input_micro_usd_per_million: u64,
    pub output_micro_usd_per_million: u64,
}

impl ApiEquivalentSummary {
    pub fn merge(&mut self, other: Self) {
        self.micro_usd = self.micro_usd.saturating_add(other.micro_usd);
        self.priced_tokens = self.priced_tokens.saturating_add(other.priced_tokens);
        self.unpriced_tokens = self.unpriced_tokens.saturating_add(other.unpriced_tokens);
    }
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
    output_micro_usd_per_million: u64,
}

pub fn estimate_api_equivalent(
    model: Option<&str>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
) -> ApiEquivalentSummary {
    let measured_tokens = input_tokens
        .unwrap_or_default()
        .saturating_add(output_tokens.unwrap_or_default());
    let total_tokens = total_tokens.unwrap_or(measured_tokens).max(measured_tokens);
    let Some(price) = model.and_then(model_price) else {
        return ApiEquivalentSummary {
            unpriced_tokens: total_tokens,
            ..Default::default()
        };
    };
    let input_tokens = input_tokens.unwrap_or_default();
    let cached_input_tokens = cached_input_tokens.unwrap_or_default().min(input_tokens);
    let uncached_input_tokens = input_tokens.saturating_sub(cached_input_tokens);
    let output_tokens = output_tokens.unwrap_or_default();
    ApiEquivalentSummary {
        micro_usd: token_cost(uncached_input_tokens, price.input_micro_usd_per_million)
            .saturating_add(token_cost(
                cached_input_tokens,
                price
                    .cached_input_micro_usd_per_million
                    .unwrap_or(price.input_micro_usd_per_million),
            ))
            .saturating_add(token_cost(
                output_tokens,
                price.output_micro_usd_per_million,
            )),
        priced_tokens: measured_tokens,
        unpriced_tokens: total_tokens.saturating_sub(measured_tokens),
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
                output_micro_usd_per_million: price.output_micro_usd_per_million,
            })
        })
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
            catalog.schema_version == 1
                && catalog.unit_tokens == 1_000_000
                && catalog.currency == "USD"
                && !catalog.catalog_version.is_empty()
                && !catalog.source_url.is_empty()
                && !catalog.verified_at.is_empty()
                && catalog.models.iter().all(|price| {
                    price
                        .cached_input_micro_usd_per_million
                        .is_none_or(|cached| cached <= price.input_micro_usd_per_million)
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
            Some(100_000),
            Some(1_100_000),
        );
        assert_eq!(estimate.micro_usd, 3_100_000);
        assert_eq!(estimate.priced_tokens, 1_100_000);
        assert_eq!(estimate.unpriced_tokens, 0);
        assert_eq!(api_model_price("GPT-5.4").unwrap().catalog_rank, 5);
        let catalog = price_catalog().unwrap();
        assert_eq!(catalog.catalog_version, "openai-standard-2026-07-13");
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
            estimate_api_equivalent(Some("private-model"), Some(2), Some(1), Some(3), Some(5)),
            ApiEquivalentSummary {
                micro_usd: 0,
                priced_tokens: 0,
                unpriced_tokens: 5
            }
        );
        assert_eq!(
            estimate_api_equivalent(Some("gpt-5.4"), None, None, None, Some(9)),
            ApiEquivalentSummary {
                micro_usd: 0,
                priced_tokens: 0,
                unpriced_tokens: 9
            }
        );
        assert_eq!(
            estimate_api_equivalent(Some("gpt-5.4"), Some(10), Some(100), Some(0), Some(10))
                .micro_usd,
            3
        );
        assert!(api_model_price("gpt-future-codex").is_none());
    }
}
