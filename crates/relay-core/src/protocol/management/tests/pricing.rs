use super::*;

#[test]
fn source_editor_prices_use_its_own_resolver_not_another_pool_member() {
    let catalog = PricingCatalog::from_litellm_json(
        r#"{"shared-model":{"litellm_provider":"openai","input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}"#,
    )
    .unwrap();
    let mut first = source_summary("first", &["SHARED-model", "unknown-model"]);
    first.in_pool = false;
    first.excluded_models.push("disabled-model".into());
    let mut second = source_summary("second", &["shared-model"]);
    let reference = TokenPrice {
        input: 1_000_000,
        output: 2_000_000,
        ..Default::default()
    };
    let provider = TokenPrice {
        cache_write: Some(0),
        priority: crate::pricing::TokenRateSet {
            input: Some(160_000),
            ..Default::default()
        },
        ..test_token_price(80_000, 400_000)
    };
    let context = PricingContext {
        source_evidence: BTreeMap::from([(
            "second".into(),
            BTreeMap::from([(
                "shared-model".into(),
                PriceEvidence {
                    provider: Some(provider),
                    manual: None,
                },
            )]),
        )]),
        ..Default::default()
    };
    first.resolve_model_prices(&catalog, &context);
    second.resolve_model_prices(&catalog, &context);
    assert_eq!(
        first.resolved_model_prices["shared-model"].quote,
        Some(reference)
    );
    assert_eq!(
        first.resolved_model_prices["shared-model"].source,
        PriceSource::LiteLlmExact
    );
    assert_eq!(first.resolved_model_prices["unknown-model"].quote, None);
    assert_eq!(first.resolved_model_prices["disabled-model"].quote, None);
    assert_eq!(
        second.resolved_model_prices["shared-model"].quote,
        Some(provider)
    );
    assert_eq!(
        second.resolved_model_prices["shared-model"].source,
        PriceSource::Provider
    );
    assert!(first.detected_model_prices.is_empty());
    assert!(first.model_price_overrides.is_empty());

    let mut encoded = serde_json::to_value(&first).unwrap();
    assert_eq!(
        encoded["resolvedModelPrices"]["shared-model"]["source"],
        "liteLlmExact"
    );
    encoded
        .as_object_mut()
        .unwrap()
        .remove("resolvedModelPrices");
    let legacy: SourceSummary = serde_json::from_value(encoded).unwrap();
    assert!(legacy.resolved_model_prices.is_empty());
}

#[test]
fn source_price_projection_preserves_manual_provenance_and_unlisted_saved_models() {
    let mut source = source_summary("manual", &[]);
    source.model_price_overrides.insert(
        "private-model".into(),
        ApiModelPriceOverride {
            input_micro_usd_per_million: 3_000_000,
            output_micro_usd_per_million: 4_000_000,
            ..Default::default()
        },
    );
    let manual = test_token_price(3_000_000, 4_000_000);
    let context = PricingContext {
        source_evidence: BTreeMap::from([(
            "manual".into(),
            BTreeMap::from([(
                "private-model".into(),
                PriceEvidence {
                    provider: None,
                    manual: Some(manual),
                },
            )]),
        )]),
        ..Default::default()
    };
    source.resolve_model_prices(&PricingCatalog::empty(), &context);
    assert_eq!(
        source.resolved_model_prices["private-model"].source,
        PriceSource::Manual
    );
    assert_eq!(
        source.resolved_model_prices["private-model"].quote,
        Some(manual)
    );
}

#[test]
fn pool_pricing_summary_reports_provider_evidence() {
    let source = source_summary("provider", &["gpt-test"]);
    let price = test_token_price(1_000_000, 2_000_000);
    let context = PricingContext {
        source_evidence: BTreeMap::from([(
            "provider".into(),
            BTreeMap::from([(
                "gpt-test".into(),
                PriceEvidence {
                    provider: Some(price),
                    manual: None,
                },
            )]),
        )]),
        ..Default::default()
    };

    assert_eq!(
        pool_pricing_source_summary(&[source], &[], &PricingCatalog::empty(), &context),
        PricingSourceSummary::Provider
    );
}
#[test]
fn pool_model_price_keeps_messages_cache_creation_when_generic_route_wins_order() {
    let generic = source_summary("a-generic", &["claude-test"]);
    let mut messages = source_summary("z-messages", &["claude-test"]);
    messages.wire_api = WireApi::Messages;
    messages.protocol_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::Messages,
        adapter: SourceAdapter::Native,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: vec!["claude-test".into()],
    }];
    let generic_price = TokenPrice {
        input: 1_000_000,
        cache_read: Some(100_000),
        cache_write_5m: None,
        cache_write_1h: None,
        output: 2_000_000,
        flex: crate::pricing::TokenRateSet::EMPTY,
        priority: crate::pricing::TokenRateSet::EMPTY,
        above_200k: crate::pricing::LongContextRates::EMPTY,
        above_272k: crate::pricing::LongContextRates::EMPTY,
        ..Default::default()
    };
    let messages_price = TokenPrice {
        cache_write_5m: Some(1_250_000),
        cache_write_1h: Some(2_500_000),
        flex: crate::pricing::TokenRateSet {
            input: Some(500_000),
            ..Default::default()
        },
        priority: crate::pricing::TokenRateSet {
            output: Some(4_000_000),
            ..Default::default()
        },
        above_200k: crate::pricing::LongContextRates {
            standard: crate::pricing::TokenRateSet {
                input: Some(2_000_000),
                ..Default::default()
            },
            ..Default::default()
        },
        ..generic_price
    };
    let context = PricingContext {
        source_metadata: BTreeMap::from([
            (
                "a-generic".into(),
                SourcePricingMetadata {
                    cache_write_models: BTreeSet::new(),
                    ..Default::default()
                },
            ),
            (
                "z-messages".into(),
                SourcePricingMetadata {
                    cache_write_models: BTreeSet::from(["claude-test".into()]),
                    ..Default::default()
                },
            ),
        ]),
        source_evidence: BTreeMap::from([
            (
                "a-generic".into(),
                BTreeMap::from([(
                    "claude-test".into(),
                    PriceEvidence {
                        provider: Some(generic_price),
                        manual: None,
                    },
                )]),
            ),
            (
                "z-messages".into(),
                BTreeMap::from([(
                    "claude-test".into(),
                    PriceEvidence {
                        provider: Some(messages_price),
                        manual: None,
                    },
                )]),
            ),
        ]),
        ..Default::default()
    };

    let models = pool_model_summaries_with_pricing(
        &[generic, messages],
        &[],
        &[],
        &PricingCatalog::empty(),
        &context,
    );

    assert_eq!(models.len(), 1);
    assert_eq!(
        models[0].cache_write_5m_micro_usd_per_million,
        Some(1_250_000)
    );
    assert_eq!(
        models[0].cache_write_1h_micro_usd_per_million,
        Some(2_500_000)
    );
    assert_eq!(models[0].token_price, Some(messages_price));
}
#[test]
fn pool_pricing_summary_reports_manual_evidence_when_catalog_is_unpriced() {
    let source = source_summary("manual", &["private-model"]);
    let price = test_token_price(3_000_000, 4_000_000);
    let context = PricingContext {
        source_evidence: BTreeMap::from([(
            "manual".into(),
            BTreeMap::from([(
                "private-model".into(),
                PriceEvidence {
                    provider: None,
                    manual: Some(price),
                },
            )]),
        )]),
        ..Default::default()
    };

    assert_eq!(
        pool_pricing_source_summary(&[source], &[], &PricingCatalog::empty(), &context),
        PricingSourceSummary::Manual
    );
}
#[test]
fn pool_pricing_summary_reports_mixed_provenance_and_unpriced_pool() {
    let provider = source_summary("provider", &["gpt-test"]);
    let manual = source_summary("manual", &["private-model"]);
    let unknown = source_summary("unknown", &["future-model"]);
    let context = PricingContext {
        source_evidence: BTreeMap::from([
            (
                "provider".into(),
                BTreeMap::from([(
                    "gpt-test".into(),
                    PriceEvidence {
                        provider: Some(test_token_price(1_000_000, 2_000_000)),
                        manual: None,
                    },
                )]),
            ),
            (
                "manual".into(),
                BTreeMap::from([(
                    "private-model".into(),
                    PriceEvidence {
                        provider: None,
                        manual: Some(test_token_price(3_000_000, 4_000_000)),
                    },
                )]),
            ),
        ]),
        ..Default::default()
    };
    let catalog = PricingCatalog::empty();

    assert_eq!(
        pool_pricing_source_summary(&[provider.clone(), manual.clone()], &[], &catalog, &context,),
        PricingSourceSummary::Mixed
    );
    assert_eq!(
        pool_pricing_source_summary(&[unknown], &[], &catalog, &context),
        PricingSourceSummary::Unpriced
    );
}
#[test]
fn pool_pricing_summary_ignores_non_eligible_members() {
    let eligible = source_summary("eligible", &["gpt-test"]);
    let mut disabled = source_summary("disabled", &["disabled-model"]);
    disabled.enabled = false;
    let mut outside_pool = source_summary("outside", &["outside-model"]);
    outside_pool.in_pool = false;
    let mut draining = source_summary("draining", &["draining-model"]);
    draining.draining = true;
    let mut missing_secret = source_summary("missing-secret", &["missing-model"]);
    missing_secret.secret_available = false;
    let context = PricingContext {
        source_evidence: BTreeMap::from([(
            "eligible".into(),
            BTreeMap::from([(
                "gpt-test".into(),
                PriceEvidence {
                    provider: Some(test_token_price(1_000_000, 2_000_000)),
                    manual: None,
                },
            )]),
        )]),
        ..Default::default()
    };

    assert_eq!(
        pool_pricing_source_summary(
            &[eligible, disabled, outside_pool, draining, missing_secret],
            &[],
            &PricingCatalog::empty(),
            &context,
        ),
        PricingSourceSummary::Provider
    );
}

#[test]
fn combined_pool_projection_keeps_all_member_provenance_and_current_prices() {
    let catalog = PricingCatalog::empty();
    let mut provider = source_summary("a-provider", &["shared", "source-only"]);
    provider.enabled = false;
    provider.secret_available = false;
    provider.resolve_model_prices(&catalog, &PricingContext::default());
    let manual = source_summary("z-manual", &["SHARED"]);
    let mut outside = source_summary("outside", &["outside-model"]);
    outside.in_pool = false;
    let sources = [provider, manual, outside];
    let accounts = [account_summary(true, &["SHARED", "account-only"])];
    let price = test_token_price(1_000_000, 2_000_000);
    let context = PricingContext {
        source_evidence: BTreeMap::from([
            (
                "a-provider".into(),
                BTreeMap::from([(
                    "shared".into(),
                    PriceEvidence {
                        provider: Some(price),
                        manual: None,
                    },
                )]),
            ),
            (
                "z-manual".into(),
                BTreeMap::from([(
                    "shared".into(),
                    PriceEvidence {
                        provider: None,
                        manual: Some(test_token_price(3_000_000, 4_000_000)),
                    },
                )]),
            ),
        ]),
        ..Default::default()
    };
    let projection = pool_model_projection_with_pricing(
        &sources,
        &accounts,
        &["shared".into()],
        &catalog,
        &context,
    );
    assert_eq!(projection.pricing_source, PricingSourceSummary::Mixed);
    assert_eq!(
        projection.pricing_source,
        pool_pricing_source_summary(&sources, &accounts, &catalog, &context)
    );
    assert_eq!(
        projection
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["SHARED", "account-only", "source-only"]
    );
    let shared = &projection.models[0];
    assert_eq!(shared.member_count, 3);
    assert!(!shared.enabled);
    assert_eq!(shared.token_price, Some(price));
    assert!(shared.protocol_routes.is_empty());
    for context in [&PricingContext::default(), &context] {
        let empty = pool_model_projection_with_pricing(&[], &[], &[], &catalog, context);
        assert!(empty.models.is_empty());
        assert_eq!(empty.pricing_source, PricingSourceSummary::Unpriced);
    }
    let unpriced = pool_model_projection_with_pricing(
        &sources,
        &accounts,
        &[],
        &catalog,
        &PricingContext::default(),
    );
    assert_eq!(unpriced.pricing_source, PricingSourceSummary::Unpriced);
    assert!(unpriced
        .models
        .iter()
        .all(|model| model.token_price.is_none()));
}

#[test]
#[ignore = "paired synthetic pricing projection timing; no provider requests"]
fn measure_pool_pricing_projection() {
    use std::{hint::black_box, time::Instant};

    let sources = (0..3)
        .map(|index| {
            let mut source = source_summary(&format!("synthetic-{index}"), &[]);
            source.models = (0..200)
                .map(|model| format!("gpt-synthetic-{index}-{model}"))
                .collect();
            source
        })
        .collect::<Vec<_>>();
    let catalog = PricingCatalog::empty();
    let context = PricingContext::default();
    let combined = || pool_model_projection_with_pricing(&sources, &[], &[], &catalog, &context);
    let separate = || PoolModelProjection {
        models: pool_model_summaries_with_pricing(&sources, &[], &[], &catalog, &context),
        pricing_source: pool_pricing_source_summary(&sources, &[], &catalog, &context),
    };
    let expected = separate();
    let actual = combined();
    assert_eq!(actual.models, expected.models);
    assert_eq!(actual.pricing_source, expected.pricing_source);
    let mut samples = [Vec::new(), Vec::new()];
    // Alternate the order so cache warm-up does not systematically favor one path.
    for round in 0..20 {
        for index in [round % 2, 1 - round % 2] {
            let started = Instant::now();
            let projection = if index == 0 { separate() } else { combined() };
            black_box(&projection);
            samples[index].push(started.elapsed().as_secs_f64() * 1_000.0);
        }
    }
    for (label, mut samples) in ["separate", "combined"].into_iter().zip(samples) {
        samples.sort_by(f64::total_cmp);
        println!(
            "pool_pricing_projection mode={label} models=600 median_ms={:.2}",
            (samples[9] + samples[10]) / 2.0
        );
    }
}
