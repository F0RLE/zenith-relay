use super::super::*;

fn catalog(raw: &str) -> ModelMetadataCatalog {
    ModelMetadataCatalog::from_models_dev_json(raw).unwrap()
}

#[test]
fn resolves_exact_and_unique_leaf_ids() {
    let catalog = catalog(
        r#"{
            "openai/gpt-test":{"name":"GPT Test","family":"gpt","release_date":"2026-01-01"},
            "anthropic/claude-test":{"name":"Claude Test","family":"claude","release_date":"2025-01-01"}
        }"#,
    );
    assert_eq!(
        catalog
            .resolve("openai/gpt-test")
            .unwrap()
            .family
            .as_deref(),
        Some("gpt")
    );
    assert_eq!(
        catalog.resolve("relay/gpt-test").unwrap().provider,
        "openai"
    );
}

#[test]
fn codex_labels_use_catalog_names_without_guessing_ambiguous_models() {
    let catalog = catalog(
        r#"{
        "openai/new-family": {"name": "Future Native Name"},
        "alpha/shared": {"name": "Alpha Name"},
        "beta/shared": {"name": "Beta Name"},
        "openai/invalid-name": {"name": "Bad\nName"},
        "openai/unnamed": {"family": "future"}
    }"#,
    );
    for (id, name) in [
        ("openai/new-family", "Future Native Name"),
        ("new-family", "Future Native Name"),
        ("relay/new-family", "Future Native Name"),
        ("alpha/shared", "Alpha Name"),
        ("beta/shared", "Beta Name"),
        ("shared", "Shared"),
        ("unrelated/shared", "Shared"),
        ("invalid-name", "Invalid Name"),
        ("unnamed", "Unnamed"),
        ("gpt-123-future", "123 Future"),
    ] {
        assert_eq!(catalog.codex_display_name(id), name, "{id}");
    }
}

#[test]
fn accepts_models_dev_data_payload_and_maps_capabilities() {
    let catalog = catalog(
        r#"{
            "data": [
                {
                    "id": "openai/gpt-array",
                    "name": "GPT Array",
                    "architecture": {
                        "input_modalities": ["text", "image"],
                        "output_modalities": ["text"]
                    },
                    "context_length": 128000,
                    "supported_parameters": ["reasoning", "structured_outputs", "tools"]
                }
            ]
        }"#,
    );
    let metadata = catalog.resolve("relay/gpt-array").unwrap();
    assert_eq!(metadata.provider, "openai");
    assert_eq!(metadata.name.as_deref(), Some("GPT Array"));
    assert_eq!(metadata.capabilities.context_limit, Some(128000));
    assert_eq!(metadata.capabilities.input_modalities, ["text", "image"]);
    assert_eq!(metadata.capabilities.reasoning, Some(true));
    assert_eq!(metadata.capabilities.tool_call, Some(true));
    assert_eq!(metadata.capabilities.structured_output, Some(true));
}

#[test]
fn keeps_records_without_a_descriptive_name() {
    let catalog = catalog(
        r#"{
            "openai/unnamed": {
                "family": "gpt",
                "release_date": "2026-01-01"
            }
        }"#,
    );
    let metadata = catalog.resolve("openai/unnamed").unwrap();
    assert_eq!(metadata.name, None);
    assert_eq!(metadata.family.as_deref(), Some("gpt"));
}

#[test]
fn ambiguous_leaf_ids_are_not_guessed() {
    let catalog = catalog(
        r#"{
            "openai/shared":{"name":"OpenAI Shared","family":"gpt"},
            "anthropic/shared":{"name":"Anthropic Shared","family":"claude"}
        }"#,
    );
    assert!(catalog.resolve("shared").is_none());
    assert!(catalog.resolve("relay/shared").is_none());
    assert!(catalog.resolve("openai/shared").is_some());
}

#[test]
fn default_company_order_preserves_manual_override_and_unknown_models() {
    let catalog = catalog(
        r#"{
        "xai/model-x":{"family":"line-x"},
        "google/model-g":{"family":"line-g"},
        "openai/model-o":{"family":"line-o"},
        "anthropic/model-a":{"family":"line-a"},
        "another/model-n":{"family":"line-n"}
    }"#,
    );
    let inventory = [
        "unknown", "model-x", "model-n", "model-a", "model-g", "model-o",
    ];
    assert_eq!(
        catalog.order_model_ids(inventory),
        ["model-o", "model-a", "model-g", "model-x", "model-n", "unknown"]
    );
    let manual = inventory.map(str::to_string);
    assert_eq!(catalog.merge_display_order(inventory, &manual), inventory);
    assert_eq!(
        catalog.merge_display_order(inventory, &[]),
        catalog.order_model_ids(inventory)
    );
}
#[test]
fn order_uses_release_dates_and_keeps_unknown_models_last() {
    let catalog = catalog(
        r#"{
            "openai/old":{"name":"Old","family":"gpt","release_date":"2025-01-01"},
            "openai/new":{"name":"New","family":"gpt","release_date":"2026-01-01"}
        }"#,
    );
    assert_eq!(
        catalog.order_model_ids(["unknown", "old", "new"]),
        ["new", "old", "unknown"]
    );
}

#[test]
fn same_generation_families_use_stable_family_order_not_release_date() {
    let catalog = catalog(
        r#"{
            "openai/gpt-6-sol":{"name":"GPT-6 Sol","family":"gpt-sol","release_date":"2026-09-22"},
            "openai/gpt-6-sol-max":{"name":"GPT-6 Sol","family":"gpt-sol","release_date":"2026-09-22"},
            "openai/gpt-5.6-sol":{"name":"GPT-5.6 Sol","family":"gpt-sol","release_date":"2026-07-09"},
            "openai/gpt-5.6-sol-max":{"name":"GPT-5.6 Sol","family":"gpt-sol","release_date":"2026-07-09"},
            "openai/gpt-6-astra":{"name":"GPT-6 Astra","family":"gpt-astra","release_date":"2026-09-04"},
            "openai/gpt-6.1-sol":{"name":"GPT-6.1 Sol","family":"gpt-sol","release_date":"2026-09-29"},
            "openai/gpt-7-sol":{"name":"GPT-7 Sol","family":"gpt-sol","release_date":"2027-01-15"},
            "openai/gpt-6.2-luna":{"name":"GPT-6.2 Luna","family":"gpt-luna","release_date":"2026-10-01"},
            "openai/gpt-5.6-luna":{"name":"GPT-5.6 Luna","family":"gpt-luna","release_date":"2026-07-09"},
            "openai/gpt-5.6-terra":{"name":"GPT-5.6 Terra","family":"gpt-terra","release_date":"2026-07-09"},
            "openai/gpt-9-nova":{"name":"GPT-9 Nova","family":"gpt-nova","release_date":"2028-01-01"}
        }"#,
    );
    let expected = [
        "gpt-6-astra",
        "gpt-7-sol",
        "gpt-6.1-sol",
        "gpt-6-sol",
        "gpt-6-sol-max",
        "gpt-5.6-sol",
        "gpt-5.6-sol-max",
        "gpt-5.6-terra",
        "gpt-6.2-luna",
        "gpt-5.6-luna",
        "gpt-9-nova",
    ];
    assert_eq!(
        catalog.order_model_ids(expected.into_iter().rev()),
        expected
    );
    assert_eq!(catalog.order_model_ids(expected), expected);
}

#[test]
fn product_tiers_survive_a_shared_or_missing_family() {
    let shared_family = catalog(
        r#"{
            "openai/gpt-5.6-luna":{"name":"GPT-5.6 Luna","family":"gpt","release_date":"2026-08-01"},
            "openai/gpt-5.6-sol":{"name":"GPT-5.6 Sol","family":"gpt","release_date":"2026-07-09"},
            "openai/gpt-8-terra":{"name":"GPT-8 Terra","family":"gpt","release_date":"2028-01-01"},
            "openai/gpt-4o":{"name":"GPT-4o","family":"gpt","release_date":"2024-05-13"}
        }"#,
    );
    assert_eq!(
        shared_family.order_model_ids(["gpt-5.6-luna", "gpt-4o", "gpt-8-terra", "gpt-5.6-sol",]),
        ["gpt-5.6-sol", "gpt-8-terra", "gpt-5.6-luna", "gpt-4o"]
    );

    let long_family = catalog(
        r#"{
            "openai/gpt-6.2-luna":{"name":"GPT-6.2 Luna","family":"gpt-6.2-luna","release_date":"2026-10-01"},
            "openai/gpt-5.6-sol":{"name":"GPT-5.6 Sol","family":"gpt-5.6-sol","release_date":"2026-07-09"}
        }"#,
    );
    assert_eq!(
        long_family.order_model_ids(["gpt-6.2-luna", "gpt-5.6-sol"]),
        ["gpt-5.6-sol", "gpt-6.2-luna"]
    );

    let unlabeled = catalog(
        r#"{
            "openai/gpt-5.6-luna":{"name":"GPT-5.6 Luna","release_date":"2026-09-01"},
            "openai/gpt-9-sol":{"name":"GPT-9 Sol","release_date":"2028-02-01"},
            "anthropic/claude-haiku-9":{"name":"Claude Haiku 9","release_date":"2028-03-01"},
            "anthropic/claude-opus-9":{"name":"Claude Opus 9","release_date":"2027-01-01"}
        }"#,
    );
    assert_eq!(
        unlabeled.order_model_ids([
            "claude-haiku-9",
            "gpt-5.6-luna",
            "claude-opus-9",
            "gpt-9-sol",
        ]),
        [
            "gpt-9-sol",
            "gpt-5.6-luna",
            "claude-opus-9",
            "claude-haiku-9"
        ]
    );
}

#[test]
fn image_generation_models_follow_text_models_from_the_same_company() {
    let catalog = catalog(
        r#"{
            "openai/gpt-6-luna":{"name":"GPT-6 Luna","family":"gpt-luna","release_date":"2026-09-01"},
            "openai/gpt-5.6-terra":{"name":"GPT-5.6 Terra","family":"gpt-terra","release_date":"2026-07-09"},
            "openai/gpt-image-2.5-flare":{"name":"GPT Image 2.5 Flare","family":"gpt-image","release_date":"2026-09-20"},
            "openai/gpt-image-2.5-sunburst":{"name":"GPT Image 2.5 Sunburst","family":"gpt-image","release_date":"2026-09-10"},
            "openai/gpt-image-2":{"name":"GPT-Image-2","family":"gpt-image","release_date":"2026-04-21"},
            "openai/gpt-5.5":{"name":"GPT-5.5","family":"gpt","release_date":"2026-04-01"},
            "openai/codex-auto-review":{"name":"Codex Auto Review","family":"codex","release_date":"2026-03-01"},
            "openai/gpt-4o":{"name":"GPT-4o","family":"gpt","release_date":"2024-05-13","architecture":{"input_modalities":["text","image"],"output_modalities":["text"]}}
        }"#,
    );
    assert_eq!(
        catalog.order_model_ids([
            "gpt-image-2",
            "gpt-4o",
            "codex-auto-review",
            "gpt-5.5",
            "gpt-image-2.5-sunburst",
            "gpt-5.6-terra",
            "gpt-image-2.5-flare",
            "gpt-6-luna",
        ]),
        [
            "gpt-5.6-terra",
            "gpt-6-luna",
            "gpt-5.5",
            "gpt-4o",
            "codex-auto-review",
            "gpt-image-2.5-sunburst",
            "gpt-image-2.5-flare",
            "gpt-image-2",
        ]
    );
}

#[test]
fn unresolved_company_text_models_stay_before_images() {
    let catalog = catalog(
        r#"{
            "openai/gpt-5.5":{"name":"GPT-5.5","family":"gpt","release_date":"2026-04-01"},
            "openai/gpt-image-2.5-sunburst":{"name":"GPT Image 2.5 Sunburst","family":"gpt-image","release_date":"2026-09-10"},
            "openai/gpt-image-2":{"name":"GPT-Image-2","family":"gpt-image","release_date":"2026-04-21"}
        }"#,
    );
    assert_eq!(
        catalog.order_model_ids([
            "gpt-reserve",
            "mystery-model",
            "gpt-image-2",
            "codex-auto-review",
            "gpt-5.5",
            "gpt-image-2.5-sunburst",
        ]),
        [
            "gpt-5.5",
            "codex-auto-review",
            "gpt-reserve",
            "gpt-image-2.5-sunburst",
            "gpt-image-2",
            "mystery-model",
        ]
    );
}

#[test]
fn company_blocks_do_not_merge_matching_family_names() {
    let catalog = catalog(
        r#"{
            "alpha/shared-new":{"name":"Shared New","family":"shared","release_date":"2026-01-01"},
            "alpha/shared-old":{"name":"Shared Old","family":"shared","release_date":"2024-01-01"},
            "beta/shared-model":{"name":"Shared Model","family":"shared","release_date":"2025-01-01"}
        }"#,
    );
    assert_eq!(
        catalog.order_model_ids(["shared-old", "shared-model", "shared-new"]),
        ["shared-new", "shared-old", "shared-model"]
    );
}

#[test]
fn catalog_families_group_versions_for_every_company_without_name_rules() {
    let catalog = catalog(
        r#"{
            "anthropic/fable-new":{"family":"claude-fable","release_date":"2026-09-01"},
            "anthropic/fable-old":{"family":"claude-fable","release_date":"2026-06-09"},
            "anthropic/opus-new":{"family":"claude-opus","release_date":"2026-07-24"},
            "anthropic/opus-old":{"family":"claude-opus","release_date":"2026-02-05"},
            "anthropic/sonnet":{"family":"claude-sonnet","release_date":"2026-06-30"},
            "anthropic/haiku":{"family":"claude-haiku","release_date":"2025-10-15"},
            "google/flash-new":{"family":"gemini-flash","release_date":"2026-09-10"},
            "google/flash-old":{"family":"gemini-flash","release_date":"2025-01-01"},
            "google/pro":{"family":"gemini-pro","release_date":"2026-02-19"},
            "openai/full-new":{"family":"gpt","release_date":"2026-09-04"},
            "openai/full-old":{"family":"gpt","release_date":"2025-01-01"},
            "openai/mini":{"family":"gpt-mini","release_date":"2026-03-05"},
            "future-company/next-new":{"family":" NEW-LINE ","release_date":"2028-01-01"},
            "future-company/next-old":{"family":"new-line","release_date":"2024-01-01"},
            "future-company/mid":{"family":"other-line","release_date":"2027-01-01"}
        }"#,
    );
    let expected = [
        "full-new",
        "full-old",
        "mini",
        "fable-new",
        "fable-old",
        "opus-new",
        "opus-old",
        "sonnet",
        "haiku",
        "flash-new",
        "flash-old",
        "pro",
        "next-new",
        "next-old",
        "mid",
    ];
    assert_eq!(
        catalog.order_model_ids(expected.into_iter().rev()),
        expected
    );
    assert_eq!(
        catalog.merge_display_order(expected.into_iter().rev(), &[]),
        expected
    );
}

#[test]
fn anthropic_families_use_stable_tier_order_before_release_dates() {
    let catalog = catalog(
        r#"{
            "anthropic/claude-opus-5-5":{"family":"claude-opus","release_date":"2026-09-22"},
            "anthropic/claude-opus-5":{"family":"claude-opus","release_date":"2026-07-24"},
            "anthropic/claude-fable-5-1":{"family":"claude-fable","release_date":"2026-09-01"},
            "anthropic/claude-fable-5":{"family":"claude-fable","release_date":"2026-06-09"},
            "anthropic/claude-sonnet-5":{"family":"claude-sonnet","release_date":"2026-06-30"},
            "anthropic/claude-sonnet-4-6":{"family":"claude-sonnet","release_date":"2026-02-17"},
            "anthropic/claude-haiku-4-5":{"family":"claude-haiku","release_date":"2025-10-15"},
            "anthropic/claude-next-1":{"family":"claude-next","release_date":"2027-01-01"}
        }"#,
    );

    assert_eq!(
        catalog.order_model_ids([
            "claude-opus-5-5",
            "claude-haiku-4-5",
            "claude-fable-5",
            "claude-sonnet-4-6",
            "claude-opus-5",
            "claude-fable-5-1",
            "claude-sonnet-5",
            "claude-next-1",
        ]),
        [
            "claude-fable-5-1",
            "claude-fable-5",
            "claude-opus-5-5",
            "claude-opus-5",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
            "claude-haiku-4-5",
            "claude-next-1",
        ]
    );
}

#[test]
fn company_order_keeps_families_together_and_missing_families_last() {
    let catalog = catalog(
        r#"{
            "alpha/new":{"family":"large","release_date":"2026-06-01"},
            "alpha/old":{"family":"large","release_date":"2024-01-01"},
            "alpha/middle":{"family":"small","release_date":"2026-01-01"},
            "alpha/no-family":{"release_date":"2026-03-01"},
            "alpha/undated":{"family":"small"},
            "beta/other":{"family":"large","release_date":"2025-01-01"}
        }"#,
    );
    assert_eq!(
        catalog.order_model_ids([
            "unknown",
            "old",
            "other",
            "undated",
            "middle",
            "no-family",
            "new"
        ]),
        [
            "new",
            "old",
            "middle",
            "undated",
            "no-family",
            "other",
            "unknown"
        ]
    );
}

#[test]
fn equal_dates_use_stable_family_and_model_ids() {
    let catalog = catalog(
        r#"{
            "alpha/first":{"family":"small","release_date":"2026-01-01"},
            "alpha/second":{"family":"large","release_date":"2026-01-01"},
            "alpha/third":{"family":"small","release_date":"2026-01-01"}
        }"#,
    );
    assert_eq!(
        catalog.order_model_ids(["first", "second", "third"]),
        ["second", "first", "third"]
    );
    assert_eq!(
        catalog.order_model_ids(["third", "first", "second"]),
        ["second", "first", "third"]
    );
}

#[test]
fn invalid_or_missing_dates_use_stable_ids_and_unknown_models_stay_last() {
    let catalog = catalog(
        r#"{
            "alpha/first":{"name":"First","family":"alpha","release_date":"not-a-date"},
            "alpha/second":{"name":"Second","family":"alpha"}
        }"#,
    );
    assert_eq!(
        catalog.order_model_ids(["second", "first", "unknown"]),
        ["first", "second", "unknown"]
    );
    assert_eq!(
        catalog.order_model_ids(["unknown", "first", "second"]),
        ["first", "second", "unknown"]
    );
}

#[test]
fn clearing_saved_order_restores_catalog_groups_and_newest_models_first() {
    let catalog = catalog(
        r#"{
            "anthropic/claude-old":{"release_date":"2025-10-15","last_updated":"2026-09-19"},
            "anthropic/claude-new":{"release_date":"2026-09-01"},
            "openai/gpt-old":{"release_date":"2025-01-01"},
            "openai/gpt-new":{"release_date":"2026-09-04"},
            "google/gemini-old":{"release_date":"2025-01-01"},
            "google/gemini-new":{"release_date":"2026-02-19"},
            "google/gemini-undated":{}
        }"#,
    );
    let inventory = [
        "claude-old",
        "gpt-old",
        "gemini-undated",
        "gemini-old",
        "claude-new",
        "gpt-new",
        "gemini-new",
        "unknown",
    ];
    let manual = inventory.map(str::to_string);
    assert_eq!(catalog.merge_display_order(inventory, &manual), manual);
    let reset = catalog.merge_display_order(inventory, &[]);
    assert_eq!(
        reset,
        [
            "gpt-new",
            "gpt-old",
            "claude-new",
            "claude-old",
            "gemini-new",
            "gemini-old",
            "gemini-undated",
            "unknown",
        ]
    );
    assert_eq!(catalog.merge_display_order(&reset, &[]), reset);
}

#[test]
fn saved_models_keep_their_relative_order() {
    let catalog = catalog(
        r#"{
            "openai/new":{"name":"New","family":"gpt","release_date":"2026-01-01"},
            "openai/middle":{"name":"Middle","family":"gpt","release_date":"2025-01-01"},
            "openai/old":{"name":"Old","family":"gpt","release_date":"2024-01-01"}
        }"#,
    );
    let ordered = catalog.merge_display_order(
        ["old", "middle", "new"],
        &["old".to_string(), "new".to_string()],
    );
    assert_eq!(ordered, ["middle", "old", "new"]);
    assert_eq!(
        ordered
            .iter()
            .filter(|model| ["old", "new"].contains(&model.as_str()))
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["old", "new"]
    );
}

#[test]
fn cache_hash_rejects_modified_payload() {
    let payload = serde_json::json!({"openai/test":{"name":"Test","family":"gpt"}});
    let mut envelope = MetadataCacheEnvelope::new(payload, 1).unwrap();
    envelope.payload = serde_json::json!({"openai/other":{"name":"Other","family":"gpt"}});
    assert_eq!(envelope.validate(), Err(ModelMetadataError::InvalidCache));
}
