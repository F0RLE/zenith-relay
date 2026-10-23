use super::decimal::{usd_per_request_to_micro_usd, usd_per_token_to_micro_usd_per_million};
use super::{
    CatalogEntry, ImageModelPrice, LongContextRates, PricingError, TokenPrice, TokenRateSet,
    MAX_MODEL_PRICE_MICRO_USD_PER_MILLION,
};
use serde_json::Value;

pub(super) fn parse_entry(
    model_id: &str,
    pricing_value: &Value,
) -> Result<Option<CatalogEntry>, PricingError> {
    let pricing_record = pricing_value
        .as_object()
        .ok_or(PricingError::InvalidRecord)?;
    let provider = pricing_record
        .get("litellm_provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|provider_name| !provider_name.is_empty())
        .map(str::to_ascii_lowercase);
    let token = parse_token_price(pricing_record)?;
    let image = parse_image_price(pricing_record)?;
    let request_micro_usd = optional_micro_request(pricing_record)?;
    // A request/image tariff with placeholder zero token fields is not a free
    // text model. Explicit zero components on token-priced models remain valid.
    let token = token.filter(|quote| {
        quote.input != 0 || quote.output != 0 || (image.is_none() && request_micro_usd.is_none())
    });
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
    pricing_record: &serde_json::Map<String, Value>,
) -> Result<Option<TokenPrice>, PricingError> {
    let input_price = optional_token(pricing_record, "input_cost_per_token")?;
    let output_price = optional_token(pricing_record, "output_cost_per_token")?;
    let Some((input, output)) = input_price.zip(output_price) else {
        return Ok(None);
    };
    let cache_read = optional_token(pricing_record, "cache_read_input_token_cost")?;
    let standard = rate_set(pricing_record, "")?;
    Ok(Some(TokenPrice {
        input,
        cache_read,
        cache_write_5m: standard.cache_write_5m,
        cache_write_1h: standard.cache_write_1h,
        cache_write: standard.cache_write,
        output,
        flex: rate_set(pricing_record, "_flex")?,
        priority: rate_set(pricing_record, "_priority")?,
        ultrafast: rate_set(pricing_record, "_ultrafast")?,
        above_100k: long_context_rates(pricing_record, "100k")?,
        above_128k: long_context_rates(pricing_record, "128k")?,
        above_200k: long_context_rates(pricing_record, "200k")?,
        above_272k: long_context_rates(pricing_record, "272k")?,
        above_512k: long_context_rates(pricing_record, "512k")?,
    }))
}

fn long_context_rates(
    pricing_record: &serde_json::Map<String, Value>,
    threshold: &str,
) -> Result<LongContextRates, PricingError> {
    Ok(LongContextRates {
        standard: long_context_set(pricing_record, threshold, "")?,
        flex: long_context_set(pricing_record, threshold, "_flex")?,
        priority: long_context_set(pricing_record, threshold, "_priority")?,
        ultrafast: long_context_set(pricing_record, threshold, "_ultrafast")?,
    })
}

fn long_context_set(
    pricing_record: &serde_json::Map<String, Value>,
    threshold: &str,
    tier_suffix: &str,
) -> Result<TokenRateSet, PricingError> {
    let above = format!("_above_{threshold}_tokens{tier_suffix}");
    let mut rates = TokenRateSet {
        input: optional_token(pricing_record, &format!("input_cost_per_token{above}"))?,
        cache_read: optional_token(
            pricing_record,
            &format!("cache_read_input_token_cost{above}"),
        )?,
        cache_write_5m: optional_token(
            pricing_record,
            &format!("cache_creation_input_token_cost{above}"),
        )?,
        cache_write_1h: optional_token(
            pricing_record,
            &format!(
                "cache_creation_input_token_cost_above_1hr_above_{threshold}_tokens{tier_suffix}"
            ),
        )?,
        output: optional_token(pricing_record, &format!("output_cost_per_token{above}"))?,
        ..TokenRateSet::EMPTY
    };
    apply_provider_cache_contract(pricing_record, &mut rates);
    Ok(rates)
}

fn rate_set(
    pricing_record: &serde_json::Map<String, Value>,
    suffix: &str,
) -> Result<TokenRateSet, PricingError> {
    let mut rates = TokenRateSet {
        input: optional_token(pricing_record, &format!("input_cost_per_token{suffix}"))?,
        cache_read: optional_token(
            pricing_record,
            &format!("cache_read_input_token_cost{suffix}"),
        )?,
        cache_write_5m: optional_token(
            pricing_record,
            &format!("cache_creation_input_token_cost{suffix}"),
        )?,
        cache_write_1h: optional_token(
            pricing_record,
            &format!("cache_creation_input_token_cost_above_1hr{suffix}"),
        )?,
        output: optional_token(pricing_record, &format!("output_cost_per_token{suffix}"))?,
        ..TokenRateSet::EMPTY
    };
    apply_provider_cache_contract(pricing_record, &mut rates);
    Ok(rates)
}

fn apply_provider_cache_contract(
    record: &serde_json::Map<String, Value>,
    rates: &mut TokenRateSet,
) {
    let provider = record
        .get("litellm_provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase);
    if matches!(
        provider.as_deref(),
        Some("anthropic" | "vertex_ai-anthropic_models")
    ) {
        return;
    }
    // Only a known Anthropic source contract assigns the untagged field to
    // its short window. Other providers and compatible hosts publish their
    // own generic tariff; neither a model name nor another provider's TTL
    // establishes its retention policy.
    rates.cache_write = rates.cache_write_5m.take();
}

fn parse_image_price(
    pricing_record: &serde_json::Map<String, Value>,
) -> Result<Option<ImageModelPrice>, PricingError> {
    let output_per_image = optional_request(pricing_record, "output_cost_per_image")?;
    let input_per_image = optional_request(pricing_record, "input_cost_per_image")?;
    let input_per_image_token = optional_token(pricing_record, "input_cost_per_image_token")?;
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
    pricing_record: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, PricingError> {
    let Some(raw_price) = pricing_record
        .get(key)
        .filter(|price_value| !price_value.is_null())
    else {
        return Ok(None);
    };
    let micro_usd_per_million = usd_per_token_to_micro_usd_per_million(raw_price)?;
    (micro_usd_per_million <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION)
        .then_some(Some(micro_usd_per_million))
        .ok_or(PricingError::Overflow)
}

fn optional_request(
    pricing_record: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, PricingError> {
    let Some(raw_price) = pricing_record
        .get(key)
        .filter(|price_value| !price_value.is_null())
    else {
        return Ok(None);
    };
    let micro_usd_per_request = usd_per_request_to_micro_usd(raw_price)?;
    (micro_usd_per_request <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION)
        .then_some(Some(micro_usd_per_request))
        .ok_or(PricingError::Overflow)
}

fn optional_micro_request(
    pricing_record: &serde_json::Map<String, Value>,
) -> Result<Option<u64>, PricingError> {
    let raw_request_price = [
        "input_cost_per_request",
        "output_cost_per_request",
        "cost_per_request",
        "request_cost_per_request",
    ]
    .iter()
    .find_map(|key| pricing_record.get(*key));
    let Some(raw_request_price) = raw_request_price.filter(|price_value| !price_value.is_null())
    else {
        return Ok(None);
    };
    let micro_usd_per_request = usd_per_request_to_micro_usd(raw_request_price)?;
    (micro_usd_per_request <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION)
        .then_some(Some(micro_usd_per_request))
        .ok_or(PricingError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_creation_tariffs_keep_their_generic_component_and_schedules() {
        let entry = parse_entry("synthetic-openai", &json!({
            "litellm_provider":"openai", "input_cost_per_token":2e-6,
            "output_cost_per_token":10e-6, "cache_creation_input_token_cost":2.5e-6,
            "input_cost_per_token_flex":1e-6, "cache_creation_input_token_cost_flex":1.25e-6,
            "input_cost_per_token_priority":4e-6, "cache_creation_input_token_cost_priority":5e-6,
            "input_cost_per_token_ultrafast":12e-6, "cache_creation_input_token_cost_ultrafast":15e-6,
            "input_cost_per_token_above_272k_tokens":4e-6,
            "cache_creation_input_token_cost_above_272k_tokens":5e-6
        })).unwrap().unwrap().token.unwrap();
        assert_eq!(entry.cache_write_5m, None);
        assert_eq!(entry.cache_write_1h, None);
        assert_eq!(entry.cache_write, Some(2_500_000));
        assert_eq!(entry.flex.cache_write, Some(1_250_000));
        assert_eq!(entry.priority.cache_write, Some(5_000_000));
        assert_eq!(entry.ultrafast.cache_write, Some(15_000_000));
        assert_eq!(entry.above_272k.standard.cache_write, Some(5_000_000));
        assert_eq!(entry.clear_messages_cache_writes(), entry);
    }

    #[test]
    fn generic_creation_tariffs_do_not_become_anthropic_windows_for_other_providers() {
        for provider in [
            None,
            Some("azure"),
            Some("bedrock_converse"),
            Some("vertex_ai"),
            Some("deepseek"),
            Some("minimax"),
            Some("zai"),
            Some("custom"),
        ] {
            let token = parse_entry(
                "synthetic-claude-or-gpt-name",
                &json!({
                    "litellm_provider": provider,
                    "input_cost_per_token": 2e-6,
                    "output_cost_per_token": 10e-6,
                    "cache_creation_input_token_cost": 0,
                    "cache_creation_input_token_cost_priority": 3e-6,
                    "cache_creation_input_token_cost_above_200k_tokens": 4e-6
                }),
            )
            .unwrap()
            .unwrap()
            .token
            .unwrap();
            assert_eq!(token.cache_write, Some(0), "{provider:?}");
            assert_eq!(token.cache_write_5m, None, "{provider:?}");
            assert_eq!(token.priority.cache_write, Some(3_000_000), "{provider:?}");
            assert_eq!(token.above_200k.standard.cache_write, Some(4_000_000));
            assert_eq!(token.clear_messages_cache_writes(), token);
        }
    }

    #[test]
    fn anthropic_source_contract_keeps_distinct_window_prices() {
        for provider in ["anthropic", " ANTHROPIC ", "vertex_ai-anthropic_models"] {
            let token = parse_entry(
                "synthetic-model",
                &json!({
                    "litellm_provider": provider,
                    "input_cost_per_token": 2e-6,
                    "output_cost_per_token": 10e-6,
                    "cache_creation_input_token_cost": 2.5e-6,
                    "cache_creation_input_token_cost_above_1hr": 4e-6
                }),
            )
            .unwrap()
            .unwrap()
            .token
            .unwrap();
            assert_eq!(token.cache_write, None);
            assert_eq!(token.cache_write_5m, Some(2_500_000));
            assert_eq!(token.cache_write_1h, Some(4_000_000));
        }
    }

    #[test]
    fn missing_creation_prices_and_other_provider_quotes_are_not_guessed() {
        for provider in ["openai", "anthropic", "custom"] {
            let entry = parse_entry(
                "synthetic",
                &json!({
                    "litellm_provider":provider, "input_cost_per_token":2e-6,
                    "output_cost_per_token":10e-6
                }),
            )
            .unwrap()
            .unwrap()
            .token
            .unwrap();
            assert_eq!(entry.cache_write, None);
        }
        let entry = parse_entry(
            "synthetic",
            &json!({
                "litellm_provider":"openai", "input_cost_per_token":2e-6,
                "output_cost_per_token":10e-6, "cache_creation_input_token_cost":3e-6
            }),
        )
        .unwrap()
        .unwrap()
        .token
        .unwrap();
        assert_eq!(entry.cache_write, Some(3_000_000));
    }

    #[test]
    fn parses_cache_windows_without_substituting_input_price() {
        let pricing_entry = parse_entry(
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
        let token = pricing_entry.token.unwrap();
        assert_eq!(token.input, 3_000_000);
        assert_eq!(token.cache_read, Some(300_000));
        assert_eq!(token.cache_write_5m, Some(3_750_000));
        assert_eq!(token.cache_write_1h, Some(6_000_000));
    }

    #[test]
    fn parses_flex_priority_and_long_context_without_filling_gaps() {
        let pricing_entry = parse_entry(
            "gpt-6.1-sol",
            &json!({
                "litellm_provider": "openai",
                "input_cost_per_token": 2e-6,
                "output_cost_per_token": 10e-6,
                "cache_read_input_token_cost": 0.1e-6,
                "cache_creation_input_token_cost": 2.5e-6,
                "input_cost_per_token_flex": 1e-6,
                "output_cost_per_token_flex": 5e-6,
                "cache_read_input_token_cost_priority": 0.2e-6,
                "input_cost_per_token_priority": 4e-6,
                "output_cost_per_token_priority": 20e-6,
                "input_cost_per_token_above_272k_tokens": 4e-6,
                "output_cost_per_token_above_272k_tokens": 15e-6,
                "input_cost_per_token_above_272k_tokens_priority": 8e-6,
                "output_cost_per_token_above_272k_tokens_priority": 30e-6,
                "input_cost_per_token_above_200k_tokens_flex": 3e-6
            }),
        )
        .unwrap()
        .unwrap();
        let token = pricing_entry.token.unwrap();
        assert_eq!(token.flex.input, Some(1_000_000));
        assert_eq!(token.flex.output, Some(5_000_000));
        assert_eq!(token.flex.cache_read, None);
        assert_eq!(token.priority.input, Some(4_000_000));
        assert_eq!(token.priority.output, Some(20_000_000));
        assert_eq!(token.priority.cache_read, Some(200_000));
        assert_eq!(token.priority.cache_write_5m, None);
        assert!(token.above_200k.standard.is_empty());
        assert_eq!(token.above_200k.flex.input, Some(3_000_000));
        assert_eq!(token.above_200k.flex.output, None);
        assert_eq!(token.above_272k.standard.input, Some(4_000_000));
        assert_eq!(token.above_272k.standard.output, Some(15_000_000));
        assert_eq!(token.above_272k.priority.input, Some(8_000_000));
        assert_eq!(token.above_272k.priority.output, Some(30_000_000));
        assert!(token.above_272k.flex.is_empty());
    }

    #[test]
    fn parses_each_published_context_threshold_and_cache_schedule() {
        for threshold in ["100k", "128k", "200k", "272k", "512k"] {
            let mut record = json!({
                "input_cost_per_token": 1e-6,
                "output_cost_per_token": 2e-6,
                "max_input_tokens": 1_000_000
            });
            for suffix in ["", "_flex", "_priority"] {
                record[format!("input_cost_per_token_above_{threshold}_tokens{suffix}")] =
                    json!(3e-6);
                record[format!("cache_read_input_token_cost_above_{threshold}_tokens{suffix}")] =
                    json!(0);
                record[format!(
                    "cache_creation_input_token_cost_above_1hr_above_{threshold}_tokens{suffix}"
                )] = json!(5e-6);
            }
            let token = parse_entry("synthetic-model", &record)
                .unwrap()
                .unwrap()
                .token
                .unwrap();
            let published: Vec<_> = token
                .context_rates()
                .into_iter()
                .filter(|(_, rates)| !rates.is_empty())
                .collect();
            assert_eq!(published.len(), 1);
            assert_eq!(format!("{}k", published[0].0 / 1000), threshold);
            for rates in [
                published[0].1.standard,
                published[0].1.flex,
                published[0].1.priority,
            ] {
                assert_eq!(rates.input, Some(3_000_000));
                assert_eq!(rates.output, None);
                assert_eq!(rates.cache_read, Some(0));
                assert_eq!(rates.cache_write_1h, Some(5_000_000));
            }
            let cleared = token.clear_messages_cache_writes();
            for (_, rates) in cleared.context_rates() {
                for rate in [rates.standard, rates.flex, rates.priority] {
                    assert_eq!(rate.cache_write_5m, None);
                    assert_eq!(rate.cache_write_1h, None);
                }
            }
        }
    }

    #[test]
    fn keeps_image_and_request_prices_out_of_token_price() {
        let image_pricing_entry = parse_entry(
            "gemini-image",
            &json!({
                "litellm_provider": "gemini",
                "mode": "image_generation",
                "output_cost_per_image": 0.039
            }),
        )
        .unwrap()
        .unwrap();
        assert!(image_pricing_entry.token.is_none());
        assert_eq!(
            image_pricing_entry
                .image
                .unwrap()
                .output_micro_usd_per_image,
            Some(39_000)
        );

        let request_pricing_entry =
            parse_entry("custom-request", &json!({"input_cost_per_request": 0.005}))
                .unwrap()
                .unwrap();
        assert!(request_pricing_entry.token.is_none());
        assert_eq!(request_pricing_entry.request_micro_usd, Some(5_000));
        for price in [
            json!({ "input_cost_per_token": 0, "output_cost_per_token": 0, "cost_per_request": 0.005 }),
            json!({ "input_cost_per_token": 0, "output_cost_per_token": 0, "output_cost_per_image": 0.04 }),
        ] {
            assert!(parse_entry("request-or-image", &price)
                .unwrap()
                .unwrap()
                .token
                .is_none());
        }
        let free = parse_entry(
            "free-token-model",
            &json!({
                "input_cost_per_token": 0, "output_cost_per_token": 0
            }),
        )
        .unwrap()
        .unwrap();
        assert_eq!(free.token.unwrap().input, 0);
    }
}
