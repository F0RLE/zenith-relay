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
fn provider_blocks_preserve_source_order_and_manual_override() {
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
fn provider_order_does_not_use_release_dates() {
    let catalog = catalog(
        r#"{
            "openai/old":{"name":"Old","family":"gpt","release_date":"2025-01-01"},
            "openai/new":{"name":"New","family":"gpt","release_date":"2026-01-01"}
        }"#,
    );
    assert_eq!(
        catalog.order_model_ids(["unknown", "old", "new"]),
        ["old", "new", "unknown"]
    );
    assert_eq!(
        catalog.order_model_ids(["unknown", "new", "old"]),
        ["new", "old", "unknown"]
    );
}

#[test]
fn inferred_provider_blocks_match_native_model_ids() {
    let catalog = catalog(r#"{"openai/placeholder":{}}"#);
    assert_eq!(
        catalog.order_model_ids([
            "grok-4",
            "mystery-model",
            "gemini-2.5",
            "claude-sonnet-5",
            "gpt-6-luna",
            "o3",
        ]),
        [
            "gpt-6-luna",
            "o3",
            "claude-sonnet-5",
            "gemini-2.5",
            "grok-4",
            "mystery-model",
        ]
    );
}

#[test]
fn other_provider_blocks_are_alphabetical_and_stable() {
    let catalog = catalog(
        r#"{
            "beta/model-b":{},
            "alpha/model-a2":{},
            "alpha/model-a1":{},
            "openai/model-o":{}
        }"#,
    );
    assert_eq!(
        catalog.order_model_ids(["model-b", "model-a2", "model-a1", "model-o"]),
        ["model-o", "model-a2", "model-a1", "model-b"]
    );
}

#[test]
fn clearing_saved_order_restores_provider_blocks() {
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
            "gpt-old",
            "gpt-new",
            "claude-old",
            "claude-new",
            "gemini-undated",
            "gemini-old",
            "gemini-new",
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
    assert_eq!(ordered, ["old", "middle", "new"]);
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
fn keeps_source_and_canonical_model_identity_separate() {
    let catalog = catalog(
        r#"{
            "google-vertex/claude-sonnet-6-1": {
                "name": "Claude Sonnet",
                "canonical_model_id": "anthropic/claude-sonnet-6-1"
            }
        }"#,
    );
    let metadata = catalog.resolve("google-vertex/claude-sonnet-6-1").unwrap();
    assert_eq!(metadata.source_model_id, "google-vertex/claude-sonnet-6-1");
    assert_eq!(
        metadata.canonical_model_id.as_deref(),
        Some("anthropic/claude-sonnet-6-1")
    );
}

#[test]
fn cache_hash_rejects_modified_payload() {
    let payload = serde_json::json!({"openai/test":{"name":"Test","family":"gpt"}});
    let mut envelope = MetadataCacheEnvelope::new(payload, 1).unwrap();
    envelope.payload = serde_json::json!({"openai/other":{"name":"Other","family":"gpt"}});
    assert_eq!(envelope.validate(), Err(ModelMetadataError::InvalidCache));
}
