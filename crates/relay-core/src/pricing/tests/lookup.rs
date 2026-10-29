use super::*;

#[test]
fn image_index_preserves_exact_and_first_eligible_leaf_matches() {
    let catalog = PricingCatalog::from_litellm_payload(
        &json!({
            "a/image-test": {"mode": "image_generation", "output_cost_per_image": 0.01},
            "b/image-test": {"mode": "image_generation", "output_cost_per_image": 0.02,
                "input_cost_per_image": 0.03},
            "image-test": {"input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6}
        }),
        None,
        None,
        false,
    )
    .unwrap();
    let leaf = catalog.image_request_prices(" IMAGE-TEST ");
    assert_eq!(leaf.len(), 1);
    assert_eq!(leaf[0].micro_usd, 10_000);
    assert_eq!(catalog.image_request_prices("a/image-test"), leaf);
    let qualified = catalog.image_request_prices("B/IMAGE-TEST");
    assert_eq!(qualified.len(), 2);
    assert_eq!(qualified[0].operation, "generation");
    assert_eq!(qualified[0].micro_usd, 20_000);
    assert_eq!(qualified[1].operation, "edit");
    assert_eq!(qualified[1].micro_usd, 30_000);
    // A foreign qualified ID never gains a leaf fallback for image pricing.
    assert!(catalog.image_request_prices("other/image-test").is_empty());
    assert!(catalog.image_request_prices("missing").is_empty());
}

#[test]
fn image_index_excludes_conflicts_before_selecting_aliases() {
    let catalog = PricingCatalog::from_litellm_payload(
        &json!({
            "A/Image-Test": {"mode": "image_generation", "output_cost_per_image": 0.01},
            "a/image-test": {"mode": "image_generation", "output_cost_per_image": 0.02},
            "b/image-test": {"mode": "image_generation", "output_cost_per_image": 0.03}
        }),
        None,
        None,
        false,
    )
    .unwrap();
    assert!(catalog.image_request_prices("a/image-test").is_empty());
    assert_eq!(
        catalog.image_request_prices("image-test")[0].micro_usd,
        30_000
    );
}

#[test]
fn canonical_index_keeps_conflicting_aliases_unpriced() {
    let catalog = PricingCatalog::from_litellm_payload(
        &json!({
            "a/gpt-test": {"litellm_provider": "openai", "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6},
            "b/gpt-test": {"litellm_provider": "openai", "input_cost_per_token": 3e-6, "output_cost_per_token": 2e-6},
            "c/gpt-test": {"litellm_provider": "openai", "input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6},
            "other/gpt-test": {"litellm_provider": "other", "input_cost_per_token": 4e-6, "output_cost_per_token": 2e-6}
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
    let independent = catalog.resolve_account("alias/gpt-test", Some(" OTHER "));
    assert_eq!(independent.quote.unwrap().input, 4_000_000);
}

#[test]
fn replacing_catalog_replaces_both_indexes_without_mutating_old_snapshots() {
    let make_catalog = |price| {
        PricingCatalog::from_litellm_payload(
        &json!({
            "openai/gpt-test": {"litellm_provider": "openai", "input_cost_per_token": price, "output_cost_per_token": price},
            "image-test": {"mode": "image_generation", "output_cost_per_image": price}
        }),
        None,
        None,
        false,
    ).unwrap()
    };
    let handle = make_catalog(0.01).handle();
    let old = handle.snapshot();
    handle.replace(make_catalog(0.02));
    let current = handle.snapshot();
    assert_eq!(old.image_request_prices("image-test")[0].micro_usd, 10_000);
    assert_eq!(
        current.image_request_prices("image-test")[0].micro_usd,
        20_000
    );
    assert_eq!(
        old.resolve_account("gpt-test", Some("openai"))
            .quote
            .unwrap()
            .input,
        10_000_000_000
    );
    assert_eq!(
        current
            .resolve_account("gpt-test", Some("openai"))
            .quote
            .unwrap()
            .input,
        20_000_000_000
    );
    handle.replace(PricingCatalog::empty());
    assert!(
        handle
            .snapshot()
            .image_request_prices("image-test")
            .is_empty()
    );
    assert_eq!(
        handle
            .snapshot()
            .resolve_account("gpt-test", Some("openai"))
            .source,
        PriceSource::Unpriced
    );
}
