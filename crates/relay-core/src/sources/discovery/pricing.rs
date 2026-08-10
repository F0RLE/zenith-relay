use crate::ApiModelPriceOverride;
use serde_json::Value;

const MICRO_USD_PER_MILLION_TOKENS: u128 = 1_000_000_000_000;
const MICRO_USD_PER_USD: u128 = 1_000_000;

pub(super) fn detected_model_price(model: &Value) -> Option<ApiModelPriceOverride> {
    let pricing = model.get("pricing").filter(|value| value.is_object());
    let input = price_component(
        model,
        pricing,
        &[
            "inputCostMicrousdPerMillion",
            "inputMicroUsdPerMillion",
            "input_cost_microusd_per_million",
            "input_micro_usd_per_million",
        ],
        &["inputCostPerToken", "input_cost_per_token", "prompt"],
    )?;
    let output = price_component(
        model,
        pricing,
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
    if input == 0 && output == 0 && request_price(model, pricing).is_some_and(|price| price > 0) {
        return None;
    }
    let cached_input = price_component(
        model,
        pricing,
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
    .or_else(|| ttl_price(model, "promptCacheReadCostsByTtl", "5m"))
    .or_else(|| ttl_price(model, "promptCacheReadCostsByTtl", "1h"));
    let cache_write_5m = price_component(
        model,
        pricing,
        &[
            "cacheWrite5mMicrousdPerMillion",
            "cacheWrite5mMicroUsdPerMillion",
            "cache_write_5m_microusd_per_million",
            "cache_write_5m_micro_usd_per_million",
            "cacheCreationInputCostMicrousdPerMillion",
            "cache_creation_input_cost_microusd_per_million",
        ],
        &[
            "cacheWriteCostPerToken",
            "cache_write_cost_per_token",
            "input_cache_write",
        ],
    )
    .or_else(|| ttl_price(model, "promptCacheWriteCostsByTtl", "5m"));
    let cache_write_1h = price_component(
        model,
        pricing,
        &[
            "cacheWrite1hMicrousdPerMillion",
            "cacheWrite1hMicroUsdPerMillion",
            "cache_write_1h_microusd_per_million",
            "cache_write_1h_micro_usd_per_million",
        ],
        &[],
    )
    .or_else(|| ttl_price(model, "promptCacheWriteCostsByTtl", "1h"));
    ApiModelPriceOverride::from_optional_fields(
        Some(input),
        cached_input,
        cache_write_5m,
        cache_write_1h,
        Some(output),
    )
    .ok()
    .flatten()
}

fn price_component(
    model: &Value,
    pricing: Option<&Value>,
    micro_usd_fields: &[&str],
    usd_per_token_fields: &[&str],
) -> Option<u64> {
    micro_usd_field(model, micro_usd_fields)
        .or_else(|| pricing.and_then(|value| micro_usd_field(value, micro_usd_fields)))
        .or_else(|| usd_per_token_field(model, usd_per_token_fields))
        .or_else(|| pricing.and_then(|value| usd_per_token_field(value, usd_per_token_fields)))
}

fn micro_usd_field(value: &Value, fields: &[&str]) -> Option<u64> {
    fields
        .iter()
        .find_map(|field| unsigned_integer(value.get(*field)?))
}

fn usd_per_token_field(value: &Value, fields: &[&str]) -> Option<u64> {
    fields
        .iter()
        .find_map(|field| usd_per_token_to_micro_usd_per_million(value.get(*field)?))
}

fn usd_per_request_field(value: &Value, fields: &[&str]) -> Option<u64> {
    fields
        .iter()
        .find_map(|field| usd_per_request_to_micro_usd(value.get(*field)?))
}

fn ttl_price(model: &Value, field: &str, ttl: &str) -> Option<u64> {
    unsigned_integer(model.get(field)?.get(ttl)?)
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
        .or_else(|| pricing.and_then(|value| micro_usd_field(value, &micro_usd_fields)))
        .or_else(|| usd_per_request_field(model, &usd_per_request_fields))
        .or_else(|| pricing.and_then(|value| usd_per_request_field(value, &usd_per_request_fields)))
}

fn unsigned_integer(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_str()
            .and_then(|value| value.trim().parse::<u64>().ok())
    })
}

fn usd_per_token_to_micro_usd_per_million(value: &Value) -> Option<u64> {
    usd_decimal_to_microusd(value, MICRO_USD_PER_MILLION_TOKENS)
}

fn usd_per_request_to_micro_usd(value: &Value) -> Option<u64> {
    usd_decimal_to_microusd(value, MICRO_USD_PER_USD)
}

fn usd_decimal_to_microusd(value: &Value, scale: u128) -> Option<u64> {
    let value = value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_number().map(ToString::to_string))?;
    let value = value.trim();
    let (mantissa, exponent) = match value.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (value, 0),
    };
    let (whole, fractional) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (whole.is_empty() && fractional.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let mut digits = String::with_capacity(whole.len() + fractional.len());
    digits.push_str(whole);
    digits.push_str(fractional);
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some(0);
    }
    // u128 has enough room for supported prices while keeping malformed
    // provider metadata bounded and entirely free of floating-point math.
    if digits.len() > 38 {
        return None;
    }
    let digits = digits.parse::<u128>().ok()?;
    let scale_digits = i64::from(scale.ilog10());
    let fractional_digits = i64::try_from(fractional.len()).ok()?;
    let shift = i64::from(exponent)
        .checked_add(scale_digits)?
        .checked_sub(fractional_digits)?;
    let result = if shift >= 0 {
        if shift > 38 {
            return None;
        }
        digits.checked_mul(10_u128.checked_pow(u32::try_from(shift).ok()?)?)?
    } else {
        let divisor_power = shift.unsigned_abs();
        // The significant value is below 10^38, so a denominator this large
        // cannot round it to one micro-unit.
        if divisor_power > 38 {
            return Some(0);
        }
        let divisor = 10_u128.checked_pow(u32::try_from(divisor_power).ok()?)?;
        let quotient = digits / divisor;
        let remainder = digits % divisor;
        quotient + u128::from(remainder >= divisor.div_ceil(2))
    };

    u64::try_from(result).ok()
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
                cache_write_5m_micro_usd_per_million: Some(3_125_000),
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 15_000_000,
            })
        );
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
                cache_write_5m_micro_usd_per_million: Some(3_125_000),
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 15_000_000,
            })
        );
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
                cached_input_micro_usd_per_million: Some(1_000_000),
                cache_write_5m_micro_usd_per_million: None,
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 2_500_000,
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
