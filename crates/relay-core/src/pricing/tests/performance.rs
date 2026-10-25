use super::*;
use std::{hint::black_box, time::Instant};

#[test]
fn bulk_catalog_build_keeps_aliases_and_skipped_rows_aligned() {
    let rate = json!({"input_cost_per_token":0.000001,"output_cost_per_token":0.000002});
    let mut records: serde_json::Map<String, serde_json::Value> = (0..40)
        .map(|index| (format!("model-{index}"), rate.clone()))
        .collect();
    records.insert(
        "a-invalid".into(),
        json!({"input_cost_per_token":"invalid"}),
    );
    records.insert("Sample/Alias".into(), rate.clone());
    records.insert("sample/alias".into(), rate.clone());
    records.insert("Sample/Conflict".into(), rate.clone());
    records.insert(
        "sample/conflict".into(),
        json!({"input_cost_per_token":0.000003,"output_cost_per_token":0.000004}),
    );
    let first_alias = records
        .keys()
        .find(|key| key.eq_ignore_ascii_case("sample/alias"))
        .unwrap()
        .clone();
    let catalog = PricingCatalog::from_litellm_payload(&records.into(), None, None, false).unwrap();
    assert_eq!(catalog.entries.len(), 44);
    assert!(!catalog.entries.contains_key("a-invalid"));
    assert_eq!(catalog.unique.get("sample/alias"), Some(&first_alias));
    assert_eq!(
        catalog
            .resolve_source("sample/alias", None, None, None, None)
            .quote,
        catalog.entries[&first_alias].token
    );
    assert!(catalog.conflicts.contains("sample/conflict"));
    assert!(!catalog.unique.contains_key("sample/conflict"));
    assert!(catalog
        .resolve_source("sample/conflict", None, None, None, None)
        .quote
        .is_none());
    assert!(catalog
        .entries
        .iter()
        .all(|(key, entry)| *key == entry.model_id));
}

#[test]
#[ignore = "synthetic snapshot catalog lookup timing; no provider requests"]
fn measure_snapshot_catalog_lookups() {
    let payload: serde_json::Map<String, serde_json::Value> = (0..6_000)
        .map(|index| {
            (
                format!("openai/gpt-synthetic-{index}"),
                json!({
                    "litellm_provider": "openai",
                    "input_cost_per_token": 0.000001,
                    "output_cost_per_token": 0.000002
                }),
            )
        })
        .collect();
    let catalog = PricingCatalog::from_litellm_payload(&payload.into(), None, None, false).unwrap();
    for phase in ["cold", "warm"] {
        let started = Instant::now();
        for model in 0..600 {
            assert!(
                black_box(catalog.image_request_prices(&format!("gpt-synthetic-{model}")))
                    .is_empty()
            );
        }
        println!(
            "snapshot_catalog phase={phase} stage=image_prices elapsed_ms={:.2}",
            started.elapsed().as_secs_f64() * 1000.0
        );
        let started = Instant::now();
        for model in 0..600 {
            let price = black_box(catalog.resolve_source(
                &format!("relay/gpt-synthetic-{model}"),
                None,
                None,
                None,
                None,
            ));
            assert_eq!(price.source, PriceSource::LiteLlmCanonical);
        }
        println!(
            "snapshot_catalog phase={phase} stage=canonical_prices elapsed_ms={:.2}",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
}
