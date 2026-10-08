use super::*;
use crate::codex_model_is_picker_eligible;
use serde_json::json;

#[test]
fn native_account_catalog_uses_reference_capabilities_despite_conflicting_account_fields() {
    use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};

    let catalog = ModelMetadataCatalog::from_models_dev_json(
        r#"{
            "gpt-native": {
                "name": "External catalog title",
                "reasoning": true,
                "reasoning_effort_levels": ["low"],
                "default_reasoning_effort": "low",
                "tool_call": true,
                "modalities": {"input": ["text"], "output": ["text"]}
            }
        }"#,
    )
    .unwrap();
    let runtime = native_catalog_test_runtime(None, Some(ModelMetadataCatalogHandle::new(catalog)));
    let key = runtime
        .authenticate(Some(&axum::http::HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
    assert!(!runtime
        .codex_model_native_responses_account_ids(&key, "gpt-native")
        .is_empty());
    let mut native_entry = routed_codex_catalog_entry(None, "gpt-native", 1_000, None)
        .as_object()
        .unwrap()
        .clone();
    native_entry.extend([
        ("slug".into(), json!("gpt-native")),
        ("display_name".into(), json!("Native GPT")),
        ("input_modalities".into(), json!(["text"])),
        ("output_modalities".into(), json!(["text"])),
        ("supports_parallel_tool_calls".into(), json!(true)),
        ("supports_search_tool".into(), json!(true)),
        (
            "supported_reasoning_levels".into(),
            json!([{ "effort": "high", "description": "Native high" }]),
        ),
        ("default_reasoning_level".into(), json!("high")),
    ]);
    let upstream = json!({"models": [Value::Object(native_entry)]});
    assert!(normalize_native_codex_catalog_entry(
        upstream["models"][0].as_object().unwrap(),
        "gpt-native",
        1_000,
        None,
    )
    .is_some());

    let response = build_codex_models_response(&runtime, &key, &visible, Some(&upstream))
        .expect("native catalog");
    let model = &response["models"][0];
    assert_eq!(model["slug"], "gpt-native");
    assert_eq!(model["display_name"], "External catalog title");
    assert_eq!(model["input_modalities"], json!(["text"]));
    assert_eq!(model["output_modalities"], json!(["text"]));
    assert_eq!(model["supports_parallel_tool_calls"], true);
    assert_eq!(model["supports_search_tool"], false);
    assert_eq!(
        model["supported_reasoning_levels"],
        json!([{"effort": "low", "description": "low"}])
    );
    assert_eq!(model["default_reasoning_level"], "low");

    // A missing account card uses this exact model's external name and
    // capabilities; it does not retain the native-only features above.
    let fallback = build_codex_models_response(&runtime, &key, &visible, None).unwrap();
    assert_eq!(
        fallback["models"][0]["display_name"],
        "External catalog title"
    );
    assert_eq!(fallback["models"][0]["slug"], "gpt-native");
    assert_eq!(fallback["models"][0]["supports_search_tool"], false);
    assert_eq!(fallback["models"][0]["default_reasoning_level"], "low");

    // A partial native card can supply capabilities without a title.
    // The normalizer's generated title must not hide a real catalog name.
    let mut unnamed = upstream.clone();
    unnamed["models"][0]
        .as_object_mut()
        .unwrap()
        .remove("display_name");
    let response = build_codex_models_response(&runtime, &key, &visible, Some(&unnamed)).unwrap();
    assert_eq!(
        response["models"][0]["display_name"],
        "External catalog title"
    );
    assert_eq!(response["models"][0]["supports_search_tool"], false);
    assert_eq!(response["models"][0]["default_reasoning_level"], "low");
}

#[test]
fn native_codex_ultra_survives_reference_projection_when_max_is_routable() {
    use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};

    let catalog = ModelMetadataCatalog::from_models_dev_json(
        r#"{"gpt-native":{"reasoning":true,"reasoning_effort_levels":["low","xhigh","max"]}}"#,
    )
    .unwrap();
    let runtime = native_catalog_test_runtime(None, Some(ModelMetadataCatalogHandle::new(catalog)));
    let key = runtime
        .authenticate(Some(&axum::http::HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
    let official = json!({"models": [{
        "slug": "gpt-native",
        "supported_reasoning_levels": [{"effort": "max"}, {"effort": "ultra"}],
        "multi_agent_version": "v2",
        "multi_agent_reasoning_effort": "xhigh"
    }]});
    let result = build_codex_models_response_from_manifests(
        &runtime,
        &key,
        &visible,
        [("native-account".into(), official)],
    )
    .unwrap();
    let row = &result["models"][0];
    assert_eq!(row["slug"], "gpt-native");
    assert_eq!(row["multi_agent_version"], "v2");
    assert_eq!(row["multi_agent_reasoning_effort"], "xhigh");
    assert_eq!(
        row["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|level| level["effort"].as_str())
            .collect::<Vec<_>>(),
        ["low", "xhigh", "max", "ultra"]
    );
    assert!(crate::codex_catalog_entry_is_compatible(row));
}

#[test]
fn native_catalog_follows_inventory_replacement_without_a_model_name_allowlist() {
    // Synthetic future identities deliberately include a non-GPT model.
    let retired_model_id = "gpt-123-retired";
    let replacement_model_ids = ["gpt-124-future", "next-family-synthetic"];
    let cards = [
        retired_model_id,
        replacement_model_ids[0],
        replacement_model_ids[1],
    ]
    .into_iter()
    .map(|id| {
        json!({
            "slug": id,
            "display_name": format!("Upstream title for {id}"),
            "supported_reasoning_levels": [{"effort": "high", "description": "High"}],
            "supports_parallel_tool_calls": true
        })
    })
    .collect::<Vec<_>>();
    // A retained manifest must not resurrect a model removed from the pool.
    for inventory in [&[retired_model_id][..], &replacement_model_ids[..]] {
        let runtime =
            native_catalog_test_runtime_with_accounts(None, None, &["native-account"], inventory);
        let key = runtime
            .authenticate(Some(&axum::http::HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
        let response = build_codex_models_response_from_manifests(
            &runtime,
            &key,
            &visible,
            [("native-account".into(), json!({"models": cards}))],
        )
        .unwrap();
        let models = response["models"].as_array().unwrap();
        assert_eq!(models.len(), inventory.len());
        assert_eq!(
            models
                .iter()
                .map(|row| row["slug"].as_str().unwrap())
                .collect::<Vec<_>>(),
            visible
                .iter()
                .map(String::as_str)
                .filter(|id| codex_model_is_picker_eligible(id))
                .collect::<Vec<_>>()
        );
        for (index, model) in models.iter().enumerate() {
            let id = model["slug"].as_str().unwrap();
            assert!(inventory.contains(&id));
            assert_eq!(model["display_name"], crate::codex_model_display_name(id));
            assert_eq!(model["supports_parallel_tool_calls"], true);
            assert_eq!(
                model["priority"],
                crate::CODEX_CATALOG_PRIORITY_BASE + index as u64
            );
            assert!(crate::codex_catalog_entry_is_compatible(model));
        }
    }
}

#[test]
fn native_card_selection_skips_invalid_owners_without_borrowing_foreign_cards() {
    let runtime = native_catalog_test_runtime_with_accounts(
        None,
        None,
        &["native-account", "second-account"],
        &["gpt-native"],
    );
    let key = runtime
        .authenticate(Some(&axum::http::HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
    let mut native = routed_codex_catalog_entry(None, "gpt-native", 1_000, None);
    native["slug"] = json!("gpt-native");
    native["display_name"] = json!("Native account name");
    native["supports_parallel_tool_calls"] = json!(true);
    native["supported_reasoning_levels"] = json!([{"effort": "high", "description": "High"}]);
    let mut invalid = native.clone();
    invalid["display_name"] = json!("First account name");
    invalid["use_responses_lite"] = json!("invalid");
    let mut foreign = native.clone();
    foreign["display_name"] = json!("Foreign account name");
    let manifests = [
        ("unrelated-account".into(), json!({"models": [foreign]})),
        ("native-account".into(), json!({"models": [invalid]})),
        ("second-account".into(), json!({"models": [native]})),
    ];
    for count in [2, 3] {
        let response = build_codex_models_response_from_manifests(
            &runtime,
            &key,
            &visible,
            manifests[..count].iter().cloned(),
        )
        .unwrap();
        let models = response["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        let model = &models[0];
        assert_eq!(model["slug"], "gpt-native");
        assert_eq!(model["display_name"], "GPT Native");
        assert_eq!(model["supports_parallel_tool_calls"], true);
        assert_eq!(
            model["supported_reasoning_levels"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(model["priority"], crate::CODEX_CATALOG_PRIORITY_BASE);
        assert!(crate::codex_catalog_entry_is_compatible(model));
    }
}

#[test]
fn missing_native_card_keeps_identity_without_inheriting_capabilities() {
    for prefix in [None, Some("local")] {
        let runtime = native_catalog_test_runtime(prefix, None);
        let key = runtime
            .authenticate(Some(&axum::http::HeaderValue::from_static("Bearer secret")))
            .unwrap();
        let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
        let display_id = prefix.map_or_else(
            || "gpt-native".to_string(),
            |prefix| format!("{prefix}/gpt-native"),
        );
        let mut foreign = routed_codex_catalog_entry(None, "gpt-native", 1_000, None);
        foreign["slug"] = json!("gpt-native");
        foreign["display_name"] = json!("Foreign name must not leak");
        foreign["supports_parallel_tool_calls"] = json!(true);
        foreign["use_responses_lite"] = json!(true);
        foreign["supported_reasoning_levels"] = json!([{"effort": "ultra"}]);
        foreign["future_native_capability"] = json!(true);
        foreign["context_window"] = json!(999_999);
        for manifests in [
            Vec::new(),
            vec![(
                "unrelated-account".into(),
                json!({"models": [foreign.clone()]}),
            )],
        ] {
            let response =
                build_codex_models_response_from_manifests(&runtime, &key, &visible, manifests)
                    .expect("native catalog");
            let model = &response["models"][0];
            assert_eq!(model["slug"], display_id);
            assert_eq!(model["display_name"], "GPT Native");
            assert_eq!(model["comp_hash"], crate::CODEX_RELAY_CATALOG_HASH);
            assert_eq!(model["supports_parallel_tool_calls"], true);
            assert_eq!(model["supported_reasoning_levels"], json!([]));
            for field in ["use_responses_lite", "future_native_capability"] {
                assert!(model.get(field).is_none(), "unexpected capability: {field}");
            }
            assert_eq!(model["context_window"], 272_000);
            assert_eq!(model["auto_compact_token_limit"], 244_800);
            assert!(crate::codex_catalog_entry_is_compatible(model));
        }
        let alias = crate::codex_model_alias(&display_id);
        for requested in [&display_id, &alias] {
            assert_eq!(
                runtime.resolve_configured_account_model(&key, requested),
                Some("gpt-native".into()),
            );
        }
    }
}

#[test]
fn native_card_preserves_the_key_model_prefix() {
    let runtime = native_catalog_test_runtime(Some("local"), None);
    let key = runtime
        .authenticate(Some(&axum::http::HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
    let mut native = routed_codex_catalog_entry(None, "gpt-native", 1_000, None);
    native["slug"] = json!("gpt-native");
    native["supports_parallel_tool_calls"] = json!(true);
    let response = build_codex_models_response_from_manifests(
        &runtime,
        &key,
        &visible,
        [("native-account".into(), json!({"models": [native]}))],
    )
    .unwrap();
    let model = &response["models"][0];
    assert_eq!(model["slug"], "local/gpt-native");
    assert_eq!(model["supports_parallel_tool_calls"], true);
    assert_eq!(
        runtime.resolve_configured_account_model(&key, model["slug"].as_str().unwrap()),
        Some("gpt-native".into()),
    );
}
