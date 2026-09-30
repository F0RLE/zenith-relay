use super::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn resolves_provider_exact_then_canonical_then_manual() {
    let catalog = PricingCatalog::from_litellm_payload(
        &json!({
            "openrouter/gpt-test": {
                "litellm_provider": "openrouter",
                "input_cost_per_token": 1e-6,
                "output_cost_per_token": 2e-6
            },
            "gpt-test": {
                "litellm_provider": "openai",
                "input_cost_per_token": 3e-6,
                "output_cost_per_token": 4e-6
            }
        }),
        Some("sha256:test".into()),
        Some(10),
        true,
    )
    .unwrap();
    let manual = TokenPrice {
        input: 9,
        cache_read: None,
        cache_write_5m: None,
        cache_write_1h: None,
        output: 9,
    };
    let exact = catalog.resolve_source(
        "gpt-test",
        Some("openrouter"),
        Some("openai"),
        None,
        Some(manual),
    );
    assert_eq!(exact.source, PriceSource::LiteLlmExact);
    assert_eq!(exact.quote.unwrap().input, 1_000_000);
    let account = catalog.resolve_account("gpt-test", Some("openai"));
    assert_eq!(account.source, PriceSource::LiteLlmCanonical);
    assert_eq!(account.quote.unwrap().input, 3_000_000);
}

#[test]
fn account_family_isolation_does_not_use_other_provider() {
    let catalog = PricingCatalog::from_litellm_payload(
        &json!({"claude-sonnet": {
            "litellm_provider": "anthropic",
            "input_cost_per_token": 3e-6,
            "output_cost_per_token": 15e-6
        }}),
        None,
        None,
        false,
    )
    .unwrap();
    assert_eq!(
        catalog
            .resolve_account("claude-sonnet", Some("openai"))
            .source,
        PriceSource::Unpriced
    );
}

#[test]
fn conflicting_case_variants_are_not_silently_overwritten() {
    let catalog = PricingCatalog::from_litellm_payload(
            &json!({
                "GPT-Test": {"litellm_provider": "openai", "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6},
                "gpt-test": {"litellm_provider": "openai", "input_cost_per_token": 3e-6, "output_cost_per_token": 4e-6}
            }),
            None,
            None,
            false,
        )
        .unwrap();
    assert!(catalog.conflicts.contains("gpt-test"));
    assert_eq!(
        catalog.resolve_account("gpt-test", Some("openai")).source,
        PriceSource::Unpriced
    );
}

#[test]
fn equivalent_qualified_and_unqualified_aliases_resolve_for_accounts() {
    let catalog = PricingCatalog::from_litellm_payload(
        &json!({
            "gpt-test": {
                "litellm_provider": "openai",
                "input_cost_per_token": 1e-6,
                "output_cost_per_token": 2e-6
            },
            "openai/gpt-test": {
                "litellm_provider": "openai",
                "input_cost_per_token": 1e-6,
                "output_cost_per_token": 2e-6
            }
        }),
        None,
        None,
        false,
    )
    .unwrap();

    assert!(catalog.conflicts.is_empty());
    let resolved = catalog.resolve_account("gpt-test", Some("openai"));
    assert_eq!(resolved.source, PriceSource::LiteLlmCanonical);
    assert_eq!(resolved.quote.unwrap().input, 1_000_000);
}

#[test]
fn canonical_matching_accepts_a_qualified_model_id() {
    let catalog = PricingCatalog::from_litellm_payload(
        &json!({
            "gpt-test": {
                "litellm_provider": "openai",
                "input_cost_per_token": 1e-6,
                "output_cost_per_token": 2e-6
            }
        }),
        None,
        None,
        false,
    )
    .unwrap();

    let resolved = catalog.resolve_account("openai/gpt-test", Some("openai"));
    assert_eq!(resolved.source, PriceSource::LiteLlmCanonical);
    assert_eq!(resolved.quote.unwrap().input, 1_000_000);
}

#[test]
fn conflicting_canonical_aliases_are_left_unpriced() {
    let catalog = PricingCatalog::from_litellm_payload(
        &json!({
            "gpt-test": {
                "litellm_provider": "openai",
                "input_cost_per_token": 1e-6,
                "output_cost_per_token": 2e-6
            },
            "openai/gpt-test": {
                "litellm_provider": "openai",
                "input_cost_per_token": 3e-6,
                "output_cost_per_token": 4e-6
            }
        }),
        None,
        None,
        false,
    )
    .unwrap();

    assert_eq!(
        catalog.resolve_account("gpt-test", Some("openai")).source,
        PriceSource::Unpriced
    );
}

#[test]
fn explicit_provider_is_required_for_provider_specific_exact_prices() {
    let catalog = PricingCatalog::from_litellm_payload(
        &json!({
            "gpt-test": {
                "litellm_provider": "openrouter",
                "input_cost_per_token": 1e-6,
                "output_cost_per_token": 2e-6
            }
        }),
        None,
        None,
        false,
    )
    .unwrap();
    let manual = TokenPrice {
        input: 9,
        cache_read: None,
        cache_write_5m: None,
        cache_write_1h: None,
        output: 9,
    };

    let exact = catalog.resolve_source("openrouter/gpt-test", Some("openrouter"), None, None, None);
    assert_eq!(exact.source, PriceSource::LiteLlmExact);
    assert_eq!(exact.quote.unwrap().input, 1_000_000);

    let isolated = catalog.resolve_source("gpt-test", Some("openai"), None, None, Some(manual));
    assert_eq!(isolated.source, PriceSource::Manual);
    assert_eq!(isolated.quote.unwrap(), manual);
}

#[test]
fn qualified_exact_match_cannot_cross_provider_namespaces() {
    let catalog = PricingCatalog::from_litellm_payload(
        &json!({
            "openrouter/gpt-test": {
                "litellm_provider": "openai",
                "input_cost_per_token": 1e-6,
                "output_cost_per_token": 2e-6
            }
        }),
        None,
        None,
        false,
    )
    .unwrap();

    let resolved = catalog.resolve_source("gpt-test", Some("openrouter"), None, None, None);
    assert_eq!(resolved.source, PriceSource::Unpriced);
}

#[test]
fn pricing_context_normalizes_source_ids_but_keeps_account_policy_isolated() {
    let catalog = PricingCatalog::from_litellm_payload(
        &json!({
            "gpt-test": {
                "litellm_provider": "openai",
                "input_cost_per_token": 1e-6,
                "output_cost_per_token": 2e-6
            }
        }),
        None,
        None,
        false,
    )
    .unwrap();
    let provider = TokenPrice {
        input: 7,
        cache_read: None,
        cache_write_5m: None,
        cache_write_1h: None,
        output: 7,
    };
    let context = PricingContext {
        account_provider_families: BTreeMap::from([("acct".into(), "openrouter".into())]),
        source_metadata: BTreeMap::from([(
            "source".into(),
            SourcePricingMetadata {
                pricing_provider: Some("openrouter".into()),
                official_provider_family: None,
                cache_write_models: BTreeSet::new(),
            },
        )]),
        source_evidence: BTreeMap::from([(
            "source".into(),
            BTreeMap::from([(
                "GPT-TEST".to_ascii_lowercase(),
                PriceEvidence {
                    provider: Some(provider),
                    manual: None,
                },
            )]),
        )]),
        global_manual_prices: BTreeMap::new(),
    };

    let source = context.candidate_price(&catalog, "source", "SOURCE", Some("gpt-test"));
    assert_eq!(source.source, PriceSource::Provider);
    assert_eq!(source.quote.unwrap(), provider);
    for client in crate::WireApi::ALL {
        for upstream in crate::WireApi::ALL {
            let adapter = crate::SourceAdapter::between(client, upstream).unwrap();
            let candidate_id = format!("source::{}", adapter.route_suffix(client));
            let price =
                context.candidate_price(&catalog, "source", &candidate_id, Some("gpt-test"));
            assert_eq!(price.source, PriceSource::Provider, "{candidate_id}");
            assert_eq!(price.quote, Some(provider), "{candidate_id}");
        }
    }
    let account = context.candidate_price(&catalog, "account", "acct", Some("gpt-test"));
    assert_eq!(account.source, PriceSource::Unpriced);
}

#[test]
fn cache_creation_price_requires_a_messages_model_route() {
    let price = TokenPrice {
        input: 1_000_000,
        cache_read: Some(100_000),
        cache_write_5m: Some(1_250_000),
        cache_write_1h: Some(2_500_000),
        output: 2_000_000,
    };
    let context = PricingContext {
        source_metadata: BTreeMap::from([
            (
                "generic".into(),
                SourcePricingMetadata {
                    cache_write_models: BTreeSet::new(),
                    ..Default::default()
                },
            ),
            (
                "messages".into(),
                SourcePricingMetadata {
                    cache_write_models: BTreeSet::from(["claude-test".into()]),
                    ..Default::default()
                },
            ),
        ]),
        source_evidence: BTreeMap::from([
            (
                "generic".into(),
                BTreeMap::from([(
                    "claude-test".into(),
                    PriceEvidence {
                        provider: Some(price),
                        manual: None,
                    },
                )]),
            ),
            (
                "messages".into(),
                BTreeMap::from([(
                    "claude-test".into(),
                    PriceEvidence {
                        provider: Some(price),
                        manual: None,
                    },
                )]),
            ),
        ]),
        ..Default::default()
    };
    let catalog = PricingCatalog::empty();

    let generic = context.candidate_price(&catalog, "source", "generic", Some("claude-test"));
    assert_eq!(generic.quote.unwrap().cache_write_5m, None);
    assert_eq!(generic.quote.unwrap().cache_write_1h, None);

    let messages = context.candidate_price(&catalog, "source", "messages", Some("claude-test"));
    assert_eq!(messages.quote.unwrap().cache_write_5m, Some(1_250_000));
    assert_eq!(messages.quote.unwrap().cache_write_1h, Some(2_500_000));
}

#[test]
fn pricing_identity_is_canonical_and_bounded() {
    assert_eq!(normalize_pricing_identity(None).unwrap(), None);
    assert_eq!(
        normalize_pricing_identity(Some(" OpenRouter ".into()))
            .unwrap()
            .as_deref(),
        Some("openrouter")
    );
    assert_eq!(
        normalize_pricing_identity(Some("official.provider_family-1".into()))
            .unwrap()
            .as_deref(),
        Some("official.provider_family-1")
    );
    assert!(normalize_pricing_identity(Some("   ".into())).is_err());
    assert!(normalize_pricing_identity(Some("has space".into())).is_err());
    assert!(normalize_pricing_identity(Some("a".repeat(129))).is_err());
}
