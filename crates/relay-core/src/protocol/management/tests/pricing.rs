use super::*;

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
    };
    let messages_price = TokenPrice {
        cache_write_5m: Some(1_250_000),
        cache_write_1h: Some(2_500_000),
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
