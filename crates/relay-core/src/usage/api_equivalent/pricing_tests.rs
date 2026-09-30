use super::*;
use crate::pricing::{
    PriceSource, PricingCatalog, PricingContext, MAX_MODEL_PRICE_MICRO_USD_PER_MILLION,
};

fn fixture_catalog() -> PricingCatalog {
    PricingCatalog::from_litellm_json(include_str!("../../../tests/fixtures/litellm-prices.json"))
        .expect("pricing fixture must be valid")
}

fn fixture_account_estimate(model: &str, usage: ApiEquivalentUsage) -> ApiEquivalentSummary {
    estimate_api_equivalent_with_catalog(
        CandidatePriceQuery {
            catalog: &fixture_catalog(),
            candidate_kind: "account",
            model: Some(model),
            provider_family: Some("openai"),
            pricing_provider: None,
            provider_price: None,
            manual_price: None,
        },
        usage,
    )
    .0
}

#[test]
fn observed_sums_keep_rollup_buckets_and_gate_complete_aggregates() {
    let complete = ApiEquivalentUsage::from_observed_sums(ObservedUsageSums {
        input_tokens: Some(10),
        cached_input_tokens: Some(4),
        cache_write_5m_tokens: Some(2),
        cache_write_1h_tokens: None,
        unknown_cache_write_tokens: Some(1),
        output_tokens: Some(3),
        total_tokens: Some(13),
        input_samples: 2,
        cached_samples: 2,
        cache_write_samples: 1,
        output_samples: 1,
        total_samples: 1,
        gate_measured_buckets: true,
    });
    assert_eq!(complete.cached_input_tokens, Some(4));
    assert_eq!(complete.cache_write_1h_tokens, Some(0));
    assert_eq!(complete.output_tokens, Some(3));
    assert_eq!(complete.total_tokens, Some(13));

    let partial = ApiEquivalentUsage::from_observed_sums(ObservedUsageSums {
        input_tokens: Some(10),
        cached_input_tokens: Some(4),
        cache_write_5m_tokens: Some(2),
        unknown_cache_write_tokens: Some(1),
        output_tokens: Some(3),
        total_tokens: Some(13),
        input_samples: 2,
        cached_samples: 1,
        gate_measured_buckets: true,
        ..ObservedUsageSums::default()
    });
    assert_eq!(partial.input_tokens, Some(10));
    assert_eq!(partial.cached_input_tokens, None);
    assert_eq!(partial.cache_write_5m_tokens, None);
    assert_eq!(partial.output_tokens, None);
    assert_eq!(partial.total_tokens, None);

    let rollup = ApiEquivalentUsage::from_observed_sums(ObservedUsageSums {
        input_tokens: Some(10),
        cached_input_tokens: Some(4),
        cache_write_5m_tokens: Some(2),
        unknown_cache_write_tokens: Some(1),
        output_tokens: Some(3),
        total_tokens: Some(13),
        ..ObservedUsageSums::default()
    });
    assert_eq!(rollup.input_tokens, Some(10));
    assert_eq!(rollup.cached_input_tokens, None);
    assert_eq!(rollup.output_tokens, Some(3));
    assert_eq!(rollup.total_tokens, Some(13));
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
        CandidatePriceQuery {
            catalog: &catalog,
            candidate_kind: "source",
            model: Some("gpt-5.4"),
            provider_family: None,
            pricing_provider: None,
            provider_price: Some(provider),
            manual_price: Some(manual),
        },
        usage,
    );
    assert_eq!(provider_resolved.source, PriceSource::Provider);
    assert_eq!(provider_estimate.micro_usd, 3_000_000);

    let (exact_estimate, exact_resolved) = estimate_api_equivalent_with_catalog(
        CandidatePriceQuery {
            catalog: &catalog,
            candidate_kind: "source",
            model: Some("gpt-5.4"),
            provider_family: None,
            pricing_provider: None,
            provider_price: None,
            manual_price: Some(manual),
        },
        usage,
    );
    assert_eq!(exact_resolved.source, PriceSource::LiteLlmExact);
    assert_eq!(exact_estimate.micro_usd, 17_500_000);

    let (manual_estimate, manual_resolved) = estimate_api_equivalent_with_catalog(
        CandidatePriceQuery {
            catalog: &catalog,
            candidate_kind: "source",
            model: Some("private-model"),
            provider_family: None,
            pricing_provider: None,
            provider_price: None,
            manual_price: Some(manual),
        },
        usage,
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
    let protocol_source_price = context.candidate_price(
        &catalog,
        "source",
        "source-1::messages",
        Some("private-model"),
    );
    assert_eq!(protocol_source_price.source, PriceSource::Provider);
    assert_eq!(protocol_source_price.quote, Some(provider.into()));
    let bridged_source_price = context.candidate_price(
        &catalog,
        "source",
        "source-1::responses_to_messages",
        Some("private-model"),
    );
    assert_eq!(bridged_source_price.source, PriceSource::Provider);
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

    let normalized =
        normalize_model_price_overrides(BTreeMap::from([(" Claude-Opus-4-8 ".to_string(), price)]))
            .unwrap();
    assert_eq!(normalized.get("claude-opus-4-8"), Some(&price));
}
