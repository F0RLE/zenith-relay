use super::*;
use serde_json::json;

#[test]
fn api_sources_generate_strict_codex_models_without_hidden_or_media_rows() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(ProviderSource {
            id: "source".into(),
            name: "source".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "upstream-secret".into(),
            wire_api: WireApi::Responses,
            models: vec![
                "vendor/claude-opus-4-8".into(),
                "gpt-image-2".into(),
                "hidden-code".into(),
                "disabled-code".into(),
            ],
        })],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "key".into(),
            secret: "secret".into(),
        })],
        GatewayRuntimeOptions {
            hidden_models: vec!["hidden-code".into()],
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
    let upstream = json!({"models": [
        {"slug": "gpt-image-2", "supported_in_api": true},
        {"slug": "disabled-code", "supported_in_api": false}
    ]});

    let response = build_codex_models_response(&runtime, &key, &visible, Some(&upstream))
        .expect("coding model catalog");
    let models = response["models"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    for model in models {
        assert!(codex_catalog_entry_is_compatible(model));
    }
    let claude = models
        .iter()
        .find(|model| model["slug"] == crate::codex_model_alias("vendor/claude-opus-4-8"))
        .expect("routed Claude model");
    assert_eq!(claude["display_name"], "Claude Opus 4.8");
    assert_eq!(claude["supported_reasoning_levels"], json!([]));
    assert!(claude.get("default_reasoning_level").is_none());
    assert_eq!(claude["input_modalities"], json!(["text", "image"]));
    assert!(models
        .iter()
        .any(|model| { model["slug"] == crate::codex_model_alias("disabled-code") }));
}
#[test]
fn api_gpt_picker_ids_stay_native_without_account_cards_or_extra_models() {
    let runtime = capability_test_runtime(
        &[
            "gpt-6-astra",
            "gpt-5.6-sol",
            "gpt-future",
            "provider/gpt-6-astra",
            "claude-future",
            "gpt-hidden",
        ],
        GatewayRuntimeOptions {
            hidden_models: vec!["gpt-hidden".into()],
            ..GatewayRuntimeOptions::default()
        },
    );
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
    let mut unrelated = crate::routed_codex_catalog_entry(None, "gpt-not-in-pool", 1_000, None);
    unrelated["slug"] = json!("gpt-not-in-pool");
    unrelated["supports_parallel_tool_calls"] = json!(true);
    unrelated["use_responses_lite"] = json!(true);
    let response = build_codex_models_response(
        &runtime,
        &key,
        &visible,
        Some(&json!({"models": [unrelated]})),
    )
    .unwrap();
    let models = response["models"].as_array().unwrap();
    assert_eq!(models.len(), 5);
    for id in ["gpt-6-astra", "gpt-5.6-sol", "gpt-future"] {
        let model = models.iter().find(|model| model["slug"] == id).unwrap();
        assert!(codex_catalog_entry_is_compatible(model));
        assert_eq!(model["comp_hash"], crate::CODEX_RELAY_CATALOG_HASH);
        assert_eq!(model["supports_parallel_tool_calls"], true);
        assert_eq!(model["supported_reasoning_levels"], json!([]));
        assert!(model.get("use_responses_lite").is_none());
    }
    for id in ["provider/gpt-6-astra", "claude-future"] {
        assert!(models
            .iter()
            .any(|model| model["slug"] == crate::codex_model_alias(id)));
    }
}
#[test]
fn known_non_native_model_publishes_reference_context_for_auto_compact() {
    use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};
    let catalog = ModelMetadataCatalog::from_models_dev_json(
        r#"{
            "vendor/claude-fable-5": {"reasoning": true, "reasoning_effort_levels": ["low", "high"],
            "default_reasoning_effort": "high", "tool_call": true,
            "modalities": {"input": ["text"], "output": ["text"]}, "limit": {"context": 64000}}
        }"#,
    )
    .unwrap();
    let runtime = capability_test_runtime(
        &["vendor/claude-fable-5"],
        GatewayRuntimeOptions {
            model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(catalog)),
            ..GatewayRuntimeOptions::default()
        },
    );
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
    let response = build_codex_models_response(&runtime, &key, &visible, None).unwrap();
    let model_row = &response["models"][0];
    assert_eq!(model_row["input_modalities"], json!(["text"]));
    assert_eq!(model_row["context_window"], 64_000);
    assert_eq!(model_row["max_context_window"], 64_000);
    assert_eq!(model_row["auto_compact_token_limit"], 57_600);
    assert_eq!(model_row["effective_context_window_percent"], 95);
    assert_eq!(model_row["supports_parallel_tool_calls"], true);
    assert_eq!(model_row["default_reasoning_level"], "high");
    assert_eq!(
        model_row["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(codex_catalog_entry_is_compatible(model_row));
}
#[test]
fn messages_bridge_does_not_invent_codex_ultra_from_max() {
    use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};
    let model = "anthropic/claude-fable-5-1";
    let catalog = ModelMetadataCatalog::from_models_dev_json(
        r#"{
            "anthropic/claude-fable-5-1": {
                "reasoning": true,
                "reasoning_effort_levels": ["low", "medium", "high", "xhigh", "max"]
            }}"#,
    )
    .unwrap();
    let source = ProviderSource {
        id: "source".into(),
        name: "source".into(),
        base_url: "https://example.test/v1".into(),
        api_key: "upstream-secret".into(),
        wire_api: WireApi::Responses,
        models: vec![model.into()],
    };
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource {
            protocol_config: crate::SourceProtocolConfig {
                endpoint_hint: Some(WireApi::Messages),
                ..Default::default()
            },
            ..RuntimeSource::unrestricted(source)
        }],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "key".into(),
            secret: "secret".into(),
        })],
        GatewayRuntimeOptions {
            model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(catalog)),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
    let response = build_codex_models_response(&runtime, &key, &visible, None).unwrap();

    assert_eq!(
        response["models"][0]["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|level| level["effort"].as_str())
            .collect::<Vec<_>>(),
        ["low", "medium", "high", "max"]
    );
    // The adapter can forward Max but cannot forward xhigh. Codex Ultra
    // is a client orchestration mode, not a synonym for this route's Max.
    assert_eq!(
        runtime.model_capabilities(model).reasoning_effort_levels,
        ["low", "medium", "high", "xhigh", "max"]
    );
}
#[test]
fn manual_overrides_do_not_grant_unknown_reasoning_capabilities() {
    let runtime =
        capability_test_runtime(&["vendor/claude-fable-5"], GatewayRuntimeOptions::default());
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
    let response =
        build_codex_models_response(&runtime, &key, &visible, None).expect("coding model catalog");
    let model = &response["models"][0];

    assert_eq!(
        model["slug"],
        crate::codex_model_alias("vendor/claude-fable-5")
    );
    assert!(model.get("default_reasoning_level").is_none());
    assert_eq!(model["supported_reasoning_levels"], json!([]));
    assert_eq!(model["supports_reasoning_summary_parameter"], false);
    assert_eq!(model["supports_reasoning_summaries"], false);
    assert_eq!(model["default_reasoning_summary"], "none");
    assert!(codex_catalog_entry_is_compatible(model));

    runtime
        .set_model_reasoning_allowed_levels(std::collections::BTreeMap::from([(
            "vendor/claude-fable-5".to_string(),
            vec!["ultra".to_string()],
        )]))
        .unwrap();
    let configured =
        build_codex_models_response(&runtime, &key, &visible, None).expect("coding model catalog");
    let configured_model = &configured["models"][0];
    assert!(configured_model.get("default_reasoning_level").is_none());
    assert_eq!(configured_model["supported_reasoning_levels"], json!([]));

    runtime
        .set_model_reasoning_allowed_levels(std::collections::BTreeMap::new())
        .unwrap();
    let no_manual_selection =
        build_codex_models_response(&runtime, &key, &visible, None).expect("coding model catalog");
    assert_eq!(
        no_manual_selection["models"][0]["supported_reasoning_levels"],
        json!([])
    );
}
#[test]
fn codex_catalog_names_do_not_merge_routes_or_grant_native_transport() {
    use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};

    let metadata = ModelMetadataCatalog::from_models_dev_json(
        r#"{
            "openai/future-model":{"name":"Future Name"},
            "alpha/shared":{"name":"Same Display Name"},
            "beta/shared":{"name":"Same Display Name"}
        }"#,
    )
    .unwrap();
    let runtime = capability_test_runtime(
        &[
            "openai/future-model",
            "alpha/shared",
            "beta/shared",
            "unknown-model",
        ],
        GatewayRuntimeOptions {
            model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(metadata)),
            ..GatewayRuntimeOptions::default()
        },
    );
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
    let response = build_codex_models_response(&runtime, &key, &visible, None).unwrap();
    let rows = response["models"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    for (row, id) in rows.iter().zip(&visible) {
        let expected = match id.as_str() {
            "openai/future-model" => "Future Name",
            "alpha/shared" | "beta/shared" => "Same Display Name",
            _ => "Unknown Model",
        };
        assert_eq!(row["display_name"], expected);
        assert_eq!(
            runtime
                .resolve_configured_model(
                    &key,
                    row["slug"].as_str().unwrap(),
                    &[WireApi::Responses]
                )
                .as_ref(),
            Some(id)
        );
        assert_eq!(row["supports_parallel_tool_calls"], true);
        assert_eq!(row["supported_reasoning_levels"], json!([]));
        assert!(row.get("use_responses_lite").is_none());
        assert_eq!(
            row["service_tiers"].as_array().map(Vec::len),
            Some(if id == "openai/future-model" { 2 } else { 0 })
        );
    }
    assert_ne!(rows[1]["slug"], rows[2]["slug"]);
}
#[test]
fn codex_catalog_uses_shared_image_defaults_for_unknown_models() {
    let runtime =
        capability_test_runtime(&["vendor/claude-fable-5"], GatewayRuntimeOptions::default());
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
    let response =
        build_codex_models_response(&runtime, &key, &visible, None).expect("coding model catalog");

    assert_eq!(
        response["models"][0]["input_modalities"],
        json!(["text", "image"])
    );
    assert!(codex_catalog_entry_is_compatible(&response["models"][0]));
}
#[test]
fn codex_catalog_uses_unique_priorities_and_shared_capability_defaults() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(ProviderSource {
            id: "source".into(),
            name: "source".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "upstream-secret".into(),
            wire_api: WireApi::Responses,
            models: vec![
                "vendor/glm-5.2".into(),
                "vendor/grok-4.5".into(),
                "vendor/gemini-3.6-flash".into(),
                "vendor/claude-opus-4-8".into(),
                "gpt-5.4".into(),
            ],
        })],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "key".into(),
            secret: "secret".into(),
        })],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
    let upstream = json!({"models": [{
        "slug": "vendor/glm-5.2",
        "supported_in_api": true
    }, {
        "slug": "vendor/grok-4.5",
        "supported_in_api": true
    }, {
        "slug": "vendor/gemini-3.6-flash",
        "supported_in_api": true
    }, {
        "slug": "vendor/claude-opus-4-8",
        "supported_in_api": true
    }, {
        "slug": "gpt-5.4",
        "use_responses_lite": true,
        "supports_parallel_tool_calls": true
    }]});

    let response = build_codex_models_response(&runtime, &key, &visible, Some(&upstream))
        .expect("coding model catalog");
    let models = response["models"].as_array().unwrap();
    let priorities = models
        .iter()
        .filter_map(|model| model["priority"].as_u64())
        .collect::<Vec<_>>();
    let display_names = models
        .iter()
        .filter_map(|model| model["display_name"].as_str())
        .collect::<Vec<_>>();

    assert_eq!(priorities, [1_000, 1_001, 1_002, 1_003, 1_004]);
    assert_eq!(
        display_names,
        [
            "GLM 5.2",
            "Grok 4.5",
            "Gemini 3.6 Flash",
            "Claude Opus 4.8",
            "5.4",
        ]
    );
    assert!(models.iter().all(codex_catalog_entry_is_compatible));
    // A generic Responses source can reuse an OpenAI-looking model ID
    // without supporting Codex's native tool contract. Only account
    // manifests are authoritative for this capability.
    assert_eq!(models[0]["supports_parallel_tool_calls"], true);
}
#[test]
fn mixed_upstream_and_fallback_catalog_rows_get_unique_priorities() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(ProviderSource {
            id: "source".into(),
            name: "source".into(),
            base_url: "https://example.test/v1".into(),
            api_key: "upstream-secret".into(),
            wire_api: WireApi::Responses,
            models: vec![
                "gpt-5.6-sol".into(),
                "vendor/claude-opus".into(),
                "vendor/grok".into(),
            ],
        })],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "key".into(),
            secret: "secret".into(),
        })],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
    let upstream = json!({
        "models": [
            {"slug": "gpt-5.6-sol", "priority": 1_000},
        ]
    });

    let response = build_codex_models_response(&runtime, &key, &visible, Some(&upstream))
        .expect("coding model catalog");
    let models = response["models"].as_array().unwrap();
    let priorities = models
        .iter()
        .map(|model| model["priority"].as_u64().expect("priority"))
        .collect::<Vec<_>>();

    assert_eq!(priorities, [1_000, 1_001, 1_002]);
    assert_eq!(
        models
            .iter()
            .map(|model| model["display_name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["5.6 Sol", "Claude Opus", "Grok"]
    );
}
#[test]
fn provider_context_does_not_replace_the_reference_window() {
    let runtime = capability_test_runtime(&["gpt-5.4"], GatewayRuntimeOptions::default());
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], now_ms());
    let upstream = json!({"models": [{
        "slug": "gpt-5.4",
        "context_window": 128_000,
        "max_context_window": 128_000,
        "auto_compact_token_limit": 122_000,
        "effective_context_window_percent": 95
    }]});
    let response = build_codex_models_response(&runtime, &key, &visible, Some(&upstream))
        .expect("coding model catalog");
    let model = &response["models"][0];

    assert_eq!(model["context_window"], 272_000);
    assert_eq!(model["max_context_window"], 272_000);
    assert_eq!(model["auto_compact_token_limit"], 244_800);
    assert_ne!(model["context_window"], 128_000);
    assert_ne!(model["auto_compact_token_limit"], 122_000);
}
