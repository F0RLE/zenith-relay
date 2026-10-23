use crate::ApiModelPriceOverride;
use serde_json::Value;

/// Explicit window prices carry their own TTL evidence. A compatible endpoint
/// does not assign another provider's retention policy to an untagged price.
pub(super) fn detected_model_price(model_record: &Value) -> Option<ApiModelPriceOverride> {
    let pricing_record = model_record
        .get("pricing")
        .filter(|pricing_value| pricing_value.is_object());
    let input_price = price_component(
        model_record,
        pricing_record,
        &[
            "inputCostMicrousdPerMillion",
            "inputMicroUsdPerMillion",
            "input_cost_microusd_per_million",
            "input_micro_usd_per_million",
        ],
        &["inputCostPerToken", "input_cost_per_token", "prompt"],
    )?;
    let output_price = price_component(
        model_record,
        pricing_record,
        &[
            "outputCostMicrousdPerMillion",
            "outputMicroUsdPerMillion",
            "output_cost_microusd_per_million",
            "output_micro_usd_per_million",
        ],
        &["outputCostPerToken", "output_cost_per_token", "completion"],
    )?;
    // The API-equivalent meter is token based. Do not turn a request-priced
    // model into a misleading zero-cost token model.
    if input_price == 0
        && output_price == 0
        && request_price(model_record, pricing_record).is_some_and(|price| price > 0)
    {
        return None;
    }
    let cache_read_price = price_component(
        model_record,
        pricing_record,
        &[
            "cachedInputCostMicrousdPerMillion",
            "cachedInputMicroUsdPerMillion",
            "cached_input_cost_microusd_per_million",
            "cached_input_micro_usd_per_million",
        ],
        &[
            "cachedInputCostPerToken",
            "cached_input_cost_per_token",
            "input_cache_read",
        ],
    )
    .or_else(|| uniform_ttl_price(model_record, pricing_record, "promptCacheReadCostsByTtl"));
    let short_cache_write_price = price_component(
        model_record,
        pricing_record,
        &[
            "cacheWrite5mMicrousdPerMillion",
            "cacheWrite5mMicroUsdPerMillion",
            "cache_write_5m_microusd_per_million",
            "cache_write_5m_micro_usd_per_million",
        ],
        &[],
    )
    .or_else(|| {
        ttl_price(
            model_record,
            pricing_record,
            "promptCacheWriteCostsByTtl",
            "5m",
        )
    });
    let long_cache_write_price = price_component(
        model_record,
        pricing_record,
        &[
            "cacheWrite1hMicrousdPerMillion",
            "cacheWrite1hMicroUsdPerMillion",
            "cache_write_1h_microusd_per_million",
            "cache_write_1h_micro_usd_per_million",
        ],
        &[],
    )
    .or_else(|| {
        ttl_price(
            model_record,
            pricing_record,
            "promptCacheWriteCostsByTtl",
            "1h",
        )
    });
    let mut price = ApiModelPriceOverride::from_optional_fields(
        Some(input_price),
        cache_read_price,
        short_cache_write_price,
        long_cache_write_price,
        Some(output_price),
    )
    .ok()
    .flatten()?;
    // Preserve a quoted generic rate independently, including an explicit zero.
    price.cache_write_micro_usd_per_million = price_component(
        model_record,
        pricing_record,
        &[
            "cacheWriteMicroUsdPerMillion",
            "cache_write_micro_usd_per_million",
            "cacheCreationInputCostMicrousdPerMillion",
            "cache_creation_input_cost_microusd_per_million",
        ],
        &[
            "cacheWriteCostPerToken",
            "cache_write_cost_per_token",
            "input_cache_write",
        ],
    );
    // Relay's explicit microUSD schedules can be supplied by a compatible
    // catalog too. Never infer tier multipliers from a provider/model name.
    let schedules = pricing_record.unwrap_or(model_record);
    for (field, target) in [
        ("flex", &mut price.flex),
        ("priority", &mut price.priority),
        ("ultrafast", &mut price.ultrafast),
    ] {
        if let Some(schedule) = schedules.get(field) {
            *target = serde_json::from_value(schedule.clone()).ok()?;
        }
    }
    for (field, target) in [
        ("above100k", &mut price.above_100k),
        ("above128k", &mut price.above_128k),
        ("above200k", &mut price.above_200k),
        ("above272k", &mut price.above_272k),
        ("above512k", &mut price.above_512k),
    ] {
        if let Some(schedule) = schedules.get(field) {
            *target = serde_json::from_value(schedule.clone()).ok()?;
        }
    }
    price.is_valid().then_some(price)
}

/// Read counters do not identify a retention window. Use a TTL map only when
/// every supplied rate agrees, including future windows and explicit zeroes.
fn uniform_ttl_price(
    model_record: &Value,
    pricing_record: Option<&Value>,
    field: &str,
) -> Option<u64> {
    let prices = model_record
        .get(field)
        .or_else(|| pricing_record.and_then(|pricing| pricing.get(field)))?
        .as_object()?;
    let mut rates = prices.values().map(unsigned_integer);
    let first = rates.next()??;
    rates.all(|rate| rate == Some(first)).then_some(first)
}

fn price_component(
    model_record: &Value,
    pricing_record: Option<&Value>,
    micro_usd_fields: &[&str],
    usd_per_token_fields: &[&str],
) -> Option<u64> {
    micro_usd_field(model_record, micro_usd_fields)
        .or_else(|| {
            pricing_record
                .and_then(|pricing_value| micro_usd_field(pricing_value, micro_usd_fields))
        })
        .or_else(|| usd_per_token_field(model_record, usd_per_token_fields))
        .or_else(|| {
            pricing_record
                .and_then(|pricing_value| usd_per_token_field(pricing_value, usd_per_token_fields))
        })
}

fn micro_usd_field(price_record: &Value, fields: &[&str]) -> Option<u64> {
    fields
        .iter()
        .find_map(|field| unsigned_integer(price_record.get(*field)?))
}

fn usd_per_token_field(price_record: &Value, fields: &[&str]) -> Option<u64> {
    fields
        .iter()
        .find_map(|field| usd_per_token_to_micro_usd_per_million(price_record.get(*field)?))
}

fn usd_per_request_field(price_record: &Value, fields: &[&str]) -> Option<u64> {
    fields
        .iter()
        .find_map(|field| usd_per_request_to_micro_usd(price_record.get(*field)?))
}

fn ttl_price(model: &Value, pricing: Option<&Value>, field: &str, ttl: &str) -> Option<u64> {
    model
        .get(field)
        .and_then(|ttl_prices| ttl_prices.get(ttl))
        .and_then(unsigned_integer)
        .or_else(|| {
            pricing
                .and_then(|pricing_value| pricing_value.get(field))
                .and_then(|ttl_prices| ttl_prices.get(ttl))
                .and_then(unsigned_integer)
        })
}

fn request_price(model: &Value, pricing: Option<&Value>) -> Option<u64> {
    let micro_usd_fields = [
        "requestCostMicrousd",
        "requestCostMicroUsd",
        "request_cost_microusd",
        "request_micro_usd",
    ];
    let usd_per_request_fields = [
        "requestCostPerRequest",
        "request_cost_per_request",
        "request",
    ];
    micro_usd_field(model, &micro_usd_fields)
        .or_else(|| {
            pricing.and_then(|pricing_value| micro_usd_field(pricing_value, &micro_usd_fields))
        })
        .or_else(|| usd_per_request_field(model, &usd_per_request_fields))
        .or_else(|| {
            pricing.and_then(|pricing_value| {
                usd_per_request_field(pricing_value, &usd_per_request_fields)
            })
        })
}

fn unsigned_integer(numeric_value: &Value) -> Option<u64> {
    numeric_value.as_u64().or_else(|| {
        numeric_value
            .as_str()
            .and_then(|number_text| number_text.trim().parse::<u64>().ok())
    })
}

fn usd_per_token_to_micro_usd_per_million(price_value: &Value) -> Option<u64> {
    crate::usd_per_token_to_micro_usd_per_million(price_value).ok()
}

fn usd_per_request_to_micro_usd(price_value: &Value) -> Option<u64> {
    crate::usd_per_request_to_micro_usd(price_value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_direct_microusd_catalog_prices() {
        let price = detected_model_price(&json!({
            "id": "zenith-model",
            "inputCostMicrousdPerMillion": 2_500_000,
            "cachedInputCostMicrousdPerMillion": 250_000,
            "cacheCreationInputCostMicrousdPerMillion": 3_125_000,
            "outputCostMicrousdPerMillion": 15_000_000,
        }));
        assert_eq!(
            price,
            Some(ApiModelPriceOverride {
                input_micro_usd_per_million: 2_500_000,
                cached_input_micro_usd_per_million: Some(250_000),
                cache_write_micro_usd_per_million: Some(3_125_000),
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 15_000_000,
                ..Default::default()
            })
        );
    }

    #[test]
    fn detects_ttl_cache_prices_for_both_windows() {
        let price = detected_model_price(&json!({
            "id": "ttl-model",
            "inputCostMicrousdPerMillion": 1_000_000,
            "outputCostMicrousdPerMillion": 2_000_000,
            "promptCacheReadCostsByTtl": { "5m": 100_000, "1h": 200_000 },
            "promptCacheWriteCostsByTtl": { "5m": 1_250_000, "1h": 2_500_000 },
        }))
        .unwrap();

        assert_eq!(price.cached_input_micro_usd_per_million, None);
        assert_eq!(price.cache_write_5m_micro_usd_per_million, Some(1_250_000));
        assert_eq!(price.cache_write_1h_micro_usd_per_million, Some(2_500_000));
    }

    #[test]
    fn detects_openrouter_style_token_prices_without_floating_point() {
        let price = detected_model_price(&json!({
            "id": "external-model",
            "pricing": {
                "prompt": "0.0000025",
                "completion": "0.000015",
                "input_cache_read": "0.00000025",
                "input_cache_write": "0.000003125",
            }
        }));
        assert_eq!(
            price,
            Some(ApiModelPriceOverride {
                input_micro_usd_per_million: 2_500_000,
                cached_input_micro_usd_per_million: Some(250_000),
                cache_write_micro_usd_per_million: Some(3_125_000),
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 15_000_000,
                ..Default::default()
            })
        );
    }

    #[test]
    fn keeps_explicit_ttl_prices_outside_a_messages_route() {
        let price = detected_model_price(&json!({
            "id": "openai-compatible-model",
            "inputCostMicrousdPerMillion": 1_000_000,
            "outputCostMicrousdPerMillion": 2_000_000,
            "cacheCreationInputCostMicrousdPerMillion": 1_250_000,
            "promptCacheWriteCostsByTtl": { "5m": 1_250_000, "1h": 2_500_000 }
        }))
        .unwrap();

        assert_eq!(price.cache_write_5m_micro_usd_per_million, Some(1_250_000));
        assert_eq!(price.cache_write_1h_micro_usd_per_million, Some(2_500_000));
    }

    #[test]
    fn detects_ttl_cache_prices_inside_pricing() {
        let price = detected_model_price(&json!({
            "id": "ttl-model",
            "pricing": {
                "prompt": "0.000001",
                "completion": "0.000002",
                "promptCacheWriteCostsByTtl": { "5m": 1_250_000, "1h": 2_500_000 }
            }
        }))
        .unwrap();
        assert_eq!(price.cache_write_5m_micro_usd_per_million, Some(1_250_000));
        assert_eq!(price.cache_write_1h_micro_usd_per_million, Some(2_500_000));
    }

    #[test]
    fn does_not_guess_a_ttl_for_generic_cache_creation() {
        let price = detected_model_price(&json!({
            "id": "generic-model",
            "inputCostMicrousdPerMillion": 1_000_000,
            "outputCostMicrousdPerMillion": 2_000_000,
            "cacheCreationInputCostMicrousdPerMillion": 1_250_000,
            "pricing": { "input_cache_write": "0.00000125" }
        }))
        .unwrap();
        assert_eq!(price.cache_write_5m_micro_usd_per_million, None);
        assert_eq!(price.cache_write_1h_micro_usd_per_million, None);
        assert_eq!(price.cache_write_micro_usd_per_million, Some(1_250_000));
    }

    #[test]
    fn ttl_read_prices_require_a_uniform_rate_not_a_preferred_window() {
        for (rates, expected) in [
            (
                json!({"5m": 100_000, "1h": 100_000, "17m": 100_000}),
                Some(100_000),
            ),
            (json!({"5m": 100_000, "1h": 200_000}), None),
            (json!({"17m": 0}), Some(0)),
            (json!({"5m": 0, "17m": null}), None),
            (json!({}), None),
        ] {
            for nested in [false, true] {
                let mut model = json!({
                    "inputMicroUsdPerMillion": 1_000_000,
                    "outputMicroUsdPerMillion": 2_000_000
                });
                if nested {
                    model["pricing"] = json!({"promptCacheReadCostsByTtl": rates});
                } else {
                    model["promptCacheReadCostsByTtl"] = rates.clone();
                }
                let price = detected_model_price(&model).unwrap();
                assert_eq!(
                    price.cached_input_micro_usd_per_million, expected,
                    "{model}"
                );
            }
        }
    }

    #[test]
    fn detects_scientific_notation_without_floating_point() {
        let price = detected_model_price(&json!({
            "id": "scientific-model",
            "pricing": {
                "prompt": 1e-6,
                "completion": 2.5e-6,
            }
        }));
        assert_eq!(
            price,
            Some(ApiModelPriceOverride {
                input_micro_usd_per_million: 1_000_000,
                cached_input_micro_usd_per_million: None,
                cache_write_5m_micro_usd_per_million: None,
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 2_500_000,
                ..Default::default()
            })
        );
    }

    #[test]
    fn ignores_incomplete_or_invalid_catalog_prices() {
        assert!(detected_model_price(&json!({
            "id": "incomplete",
            "inputCostMicrousdPerMillion": 1_000_000,
        }))
        .is_none());
        assert!(detected_model_price(&json!({
            "id": "invalid",
            "pricing": { "prompt": "-1", "completion": "0.000001" },
        }))
        .is_none());
    }

    #[test]
    fn does_not_turn_request_priced_models_into_zero_cost_token_models() {
        assert!(detected_model_price(&json!({
            "id": "image-model",
            "inputCostMicrousdPerMillion": 0,
            "outputCostMicrousdPerMillion": 0,
            "requestCostMicrousd": 500_000,
        }))
        .is_none());
        assert!(detected_model_price(&json!({
            "id": "image-model-openrouter",
            "pricing": { "prompt": "0", "completion": "0", "request": "0.5" },
        }))
        .is_none());
    }
}
