use super::decimal::{usd_per_request_to_micro_usd, usd_per_token_to_micro_usd_per_million};
use super::{
    CatalogEntry, ImageModelPrice, PricingError, TokenPrice, MAX_MODEL_PRICE_MICRO_USD_PER_MILLION,
};
use serde_json::Value;

pub(super) fn parse_entry(
    model_id: &str,
    value: &Value,
) -> Result<Option<CatalogEntry>, PricingError> {
    let object = value.as_object().ok_or(PricingError::InvalidRecord)?;
    let provider = object
        .get("litellm_provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let token = parse_token_price(object)?;
    let image = parse_image_price(object)?;
    let request_micro_usd = optional_micro_request(object)?;
    if token.is_none() && image.is_none() && request_micro_usd.is_none() {
        return Ok(None);
    }
    if let Some(request_micro_usd) = request_micro_usd {
        if token.is_none() && image.is_none() {
            return Ok(Some(CatalogEntry::request_priced(
                model_id.to_string(),
                provider,
                request_micro_usd,
            )));
        }
    }
    Ok(Some(CatalogEntry {
        model_id: model_id.to_string(),
        provider,
        token,
        image,
        request_micro_usd,
    }))
}

fn parse_token_price(
    object: &serde_json::Map<String, Value>,
) -> Result<Option<TokenPrice>, PricingError> {
    let input = optional_token(object, "input_cost_per_token")?;
    let output = optional_token(object, "output_cost_per_token")?;
    let Some((input, output)) = input.zip(output) else {
        return Ok(None);
    };
    let cache_read = optional_token(object, "cache_read_input_token_cost")?;
    let cache_write_5m = optional_token(object, "cache_creation_input_token_cost")?;
    let cache_write_1h = optional_token(object, "cache_creation_input_token_cost_above_1hr")?;
    Ok(Some(TokenPrice {
        input,
        cache_read,
        cache_write_5m,
        cache_write_1h,
        output,
    }))
}

fn parse_image_price(
    object: &serde_json::Map<String, Value>,
) -> Result<Option<ImageModelPrice>, PricingError> {
    let output_per_image = optional_request(object, "output_cost_per_image")?;
    let input_per_image = optional_request(object, "input_cost_per_image")?;
    let input_per_image_token = optional_token(object, "input_cost_per_image_token")?;
    if output_per_image.is_none() && input_per_image.is_none() && input_per_image_token.is_none() {
        return Ok(None);
    }
    Ok(Some(ImageModelPrice {
        input_micro_usd_per_image: input_per_image,
        output_micro_usd_per_image: output_per_image,
        input_micro_usd_per_image_token: input_per_image_token,
    }))
}

fn optional_token(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, PricingError> {
    let Some(value) = object.get(key).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let value = usd_per_token_to_micro_usd_per_million(value)?;
    (value <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION)
        .then_some(Some(value))
        .ok_or(PricingError::Overflow)
}

fn optional_request(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, PricingError> {
    let Some(value) = object.get(key).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let value = usd_per_request_to_micro_usd(value)?;
    (value <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION)
        .then_some(Some(value))
        .ok_or(PricingError::Overflow)
}

fn optional_micro_request(
    object: &serde_json::Map<String, Value>,
) -> Result<Option<u64>, PricingError> {
    let value = [
        "input_cost_per_request",
        "output_cost_per_request",
        "cost_per_request",
        "request_cost_per_request",
    ]
    .iter()
    .find_map(|key| object.get(*key));
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let value = usd_per_request_to_micro_usd(value)?;
    (value <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION)
        .then_some(Some(value))
        .ok_or(PricingError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_cache_windows_without_substituting_input_price() {
        let entry = parse_entry(
            "claude-sonnet-4",
            &json!({
                "litellm_provider": "anthropic",
                "input_cost_per_token": 3e-6,
                "output_cost_per_token": 15e-6,
                "cache_read_input_token_cost": 0.3e-6,
                "cache_creation_input_token_cost": 3.75e-6,
                "cache_creation_input_token_cost_above_1hr": 6e-6
            }),
        )
        .unwrap()
        .unwrap();
        let token = entry.token.unwrap();
        assert_eq!(token.input, 3_000_000);
        assert_eq!(token.cache_read, Some(300_000));
        assert_eq!(token.cache_write_5m, Some(3_750_000));
        assert_eq!(token.cache_write_1h, Some(6_000_000));
    }

    #[test]
    fn keeps_image_and_request_prices_out_of_token_price() {
        let image = parse_entry(
            "gemini-image",
            &json!({
                "litellm_provider": "gemini",
                "mode": "image_generation",
                "output_cost_per_image": 0.039
            }),
        )
        .unwrap()
        .unwrap();
        assert!(image.token.is_none());
        assert_eq!(
            image.image.unwrap().output_micro_usd_per_image,
            Some(39_000)
        );

        let request = parse_entry("custom-request", &json!({"input_cost_per_request": 0.005}))
            .unwrap()
            .unwrap();
        assert!(request.token.is_none());
        assert_eq!(request.request_micro_usd, Some(5_000));
    }
}
