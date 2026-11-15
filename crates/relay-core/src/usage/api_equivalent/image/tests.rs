use super::*;
use crate::pricing::{PricingCatalog, PricingContext};
use serde_json::json;

fn catalog() -> PricingCatalog {
    PricingCatalog::from_litellm_payload(
        &json!({
            "gpt-image-synthetic": {
                "litellm_provider":"openai", "input_cost_per_token":0.000005,
                "cache_read_input_token_cost":0.00000125,
                "input_cost_per_image_token":0.000008,
                "cache_read_input_image_token_cost":0.000002,
                "output_cost_per_image_token":0.00003,
                "output_cost_per_image":0.99
            },
            "synthetic-svg": {
                "litellm_provider":"openai", "input_cost_per_token":0.000001,
                "output_cost_per_token":0.000002
            }
        }),
        None,
        None,
        false,
    )
    .unwrap()
}

fn usage() -> ApiEquivalentUsage {
    ApiEquivalentUsage {
        input_tokens: Some(45),
        output_tokens: Some(515),
        total_tokens: Some(560),
        ..Default::default()
    }
}

fn image() -> ImageUsage {
    ImageUsage {
        input_text_tokens: Some(45),
        input_image_tokens: Some(0),
        output_image_tokens: Some(515),
        output_text_tokens: Some(0),
        generated_images: 1,
        ..Default::default()
    }
}

#[test]
fn per_image_cost_remains_measured_without_token_counts() {
    let catalog = PricingCatalog::from_litellm_json(
        r#"{"gpt-image-counted":{"litellm_provider":"openai","output_cost_per_image":0.04}}"#,
    )
    .unwrap();
    let context = PricingContext::default();
    let resolver = CatalogPriceResolver::new(&catalog, &context);
    let (summary, detail) = resolver.estimate_image_breakdown(
        "account",
        "synthetic",
        Some("gpt-image-counted"),
        ApiEquivalentUsage::default(),
        &ImageUsage {
            generated_images: 2,
            ..Default::default()
        },
    );
    assert_eq!(summary.micro_usd, 80_000);
    assert_eq!(summary.priced_tokens, 0);
    assert_eq!(summary.unpriced_tokens, 0);
    assert_eq!(detail.image.unwrap().output_image.unwrap(), summary);
}

#[test]
fn image_prices_modalities_without_adding_a_per_image_fee() {
    let catalog = catalog();
    let context = PricingContext::default();
    let resolver = CatalogPriceResolver::new(&catalog, &context);
    let (total, detail) = resolver.estimate_image_breakdown(
        "account",
        "synthetic",
        Some("gpt-image-synthetic"),
        usage(),
        &image(),
    );
    assert_eq!(
        total,
        ApiEquivalentSummary {
            micro_usd: 15_675,
            priced_tokens: 560,
            unpriced_tokens: 0
        }
    );
    assert_eq!(detail.input.unwrap().micro_usd, 225);
    assert_eq!(detail.output.unwrap().micro_usd, 15_450);
    assert_eq!(
        detail.image.unwrap().output_image.unwrap().priced_tokens,
        515
    );
}

#[test]
fn image_missing_or_inconsistent_modalities_are_not_text_priced() {
    let catalog = catalog();
    let context = PricingContext::default();
    let resolver = CatalogPriceResolver::new(&catalog, &context);
    let mut image = image();
    image.input_details_consistent = Some(false);
    let (total, _) = resolver.estimate_image_breakdown(
        "account",
        "synthetic",
        Some("gpt-image-synthetic"),
        usage(),
        &image,
    );
    assert_eq!(total.micro_usd, 15_450);
    assert_eq!(total.unpriced_tokens, 45);
    image.input_details_consistent = None;
    image.input_text_tokens = Some(100);
    let (total, _) = resolver.estimate_image_breakdown(
        "account",
        "synthetic",
        Some("gpt-image-synthetic"),
        usage(),
        &image,
    );
    assert_eq!(total.unpriced_tokens, 45);
}

#[test]
fn image_unsplit_cache_is_not_discounted_as_text() {
    let mut image = image();
    image.input_image_tokens = Some(20);
    image.input_text_tokens = Some(25);
    image.cached_input_tokens = Some(10);
    assert_eq!(cache_split(&image), None);
    image.cached_text_tokens = Some(4);
    assert_eq!(cache_split(&image), Some((4, 6)));
    image.cached_image_tokens = Some(7);
    assert_eq!(cache_split(&image), None);
}

#[test]
fn image_cached_modalities_use_their_own_rates() {
    let catalog = catalog();
    let context = PricingContext::default();
    let resolver = CatalogPriceResolver::new(&catalog, &context);
    let image = ImageUsage {
        input_text_tokens: Some(100),
        input_image_tokens: Some(100),
        cached_input_tokens: Some(40),
        cached_text_tokens: Some(20),
        cached_image_tokens: Some(20),
        output_image_tokens: Some(10),
        ..Default::default()
    };
    let usage = ApiEquivalentUsage {
        input_tokens: Some(200),
        output_tokens: Some(10),
        total_tokens: Some(210),
        ..Default::default()
    };
    let (value, detail) = resolver.estimate_image_breakdown(
        "account",
        "synthetic",
        Some("gpt-image-synthetic"),
        usage,
        &image,
    );
    assert_eq!(
        value,
        ApiEquivalentSummary {
            micro_usd: 1_405,
            priced_tokens: 210,
            unpriced_tokens: 0
        }
    );
    assert_eq!(detail.cache_read.unwrap().micro_usd, 65);
    assert_eq!(detail.image.unwrap().input_image.unwrap().micro_usd, 680);
    let incomplete = ImageUsage {
        input_image_tokens: None,
        cached_text_tokens: None,
        cached_image_tokens: None,
        ..image
    };
    assert_eq!(cache_split(&incomplete), None);
}

#[test]
fn image_catalog_mode_prevents_scalar_pricing_of_custom_model_ids() {
    let catalog = PricingCatalog::from_litellm_json(
        r#"{"synthetic-renderer":{
        "mode":"image_generation","input_cost_per_token":0.000005,"output_cost_per_token":0.00001,
        "output_cost_per_image_token":0.00003}}"#,
    )
    .unwrap();
    let context = PricingContext::default();
    let resolver = CatalogPriceResolver::new(&catalog, &context);
    assert_eq!(
        resolver
            .estimate("source", "synthetic", Some("synthetic-renderer"), usage())
            .priced_tokens,
        0
    );
    assert_eq!(
        resolver
            .estimate_image_breakdown(
                "source",
                "synthetic",
                Some("synthetic-renderer"),
                usage(),
                &image()
            )
            .0
            .micro_usd,
        15_675
    );
}

#[test]
fn image_svg_uses_its_author_price_once_and_keeps_unknown_models_unpriced() {
    let catalog = catalog();
    let context = PricingContext::default();
    let resolver = CatalogPriceResolver::new(&catalog, &context);
    let mut image = image();
    image.svg_usage = Some(crate::usage::SvgUsage {
        model: Some("synthetic-svg".into()),
        input_tokens: Some(10),
        output_tokens: Some(20),
        reasoning_tokens: Some(15),
        total_tokens: Some(30),
    });
    let usage = ApiEquivalentUsage {
        input_tokens: Some(55),
        output_tokens: Some(535),
        total_tokens: Some(590),
        ..Default::default()
    };
    let (total, detail) = resolver.estimate_image_breakdown(
        "account",
        "synthetic",
        Some("gpt-image-synthetic"),
        usage,
        &image,
    );
    assert_eq!(total.micro_usd, 15_725);
    assert_eq!(total.priced_tokens, 590);
    assert_eq!(detail.image.unwrap().svg_output.unwrap().micro_usd, 40);
    image.svg_usage.as_mut().unwrap().model = None;
    let (total, _) = resolver.estimate_image_breakdown(
        "account",
        "synthetic",
        Some("gpt-image-synthetic"),
        usage,
        &image,
    );
    assert_eq!(total.micro_usd, 15_675);
    assert_eq!(total.unpriced_tokens, 30);
}

#[test]
fn image_unknown_tiers_and_wrong_account_family_stay_unpriced() {
    let catalog = catalog();
    let mut context = PricingContext::default();
    context
        .account_provider_families
        .insert("other".into(), "anthropic".into());
    let resolver = CatalogPriceResolver::new(&catalog, &context);
    for (id, usage) in [
        ("other", usage()),
        (
            "synthetic",
            ApiEquivalentUsage {
                price_class: UsagePriceClass::Flex,
                ..usage()
            },
        ),
    ] {
        let (total, _) = resolver.estimate_image_breakdown(
            "account",
            id,
            Some("gpt-image-synthetic"),
            usage,
            &image(),
        );
        assert_eq!(total.micro_usd, 0);
        assert_eq!(total.unpriced_tokens, 560);
    }
}
