use super::*;
use serde_json::json;

fn endpoint(protocol: &str, fields: Value) -> zenith_relay_core::ModelEndpointCapability {
    let mut endpoint = json!({
        "modelId":"vendor/synthetic","upstreamWireApi":protocol,"status":"declared",
        "origin":"catalog","checkedAtMs":1
    });
    endpoint
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    serde_json::from_value(endpoint).unwrap()
}

fn metadata(fields: Value) -> ModelMetadataCatalog {
    ModelMetadataCatalog::from_models_dev_json(&json!({"vendor/synthetic":fields}).to_string())
        .unwrap()
}

#[test]
fn direct_codex_uses_exact_responses_endpoint_controls_without_changing_reference() {
    let (root, home, _backups) = profile_dirs("direct-codex-endpoint-controls");
    ensure_test_native_catalog(&home);
    let metadata = metadata(json!({
        "tool_call":true,"parallel_tool_call":true,"attachment":true,"reasoning":true,
        "reasoning_effort_levels":["low","high"],"default_reasoning_effort":"low",
        "limit":{"context":100000,"input":90000,"output":10000},
        "service_tiers":["priority"],
        "modalities":{"input":["text","image","audio"],"output":["text"]}
    }));
    let endpoints = [
        endpoint(
            "chat_completions",
            json!({
                "features":{"images":"declared","function_tools":"declared"},
                "reasoningEfforts":["low"],"tokenLimits":{"context":99999},
                "serviceTiers":["standard","fast"]
            }),
        ),
        endpoint(
            "responses",
            json!({
                "features":{"images":"unsupported","function_tools":"unsupported"},
                "reasoningEfforts":["high"],
                "tokenLimits":{"context":2000,"input":1800,"output":200},
                "serviceTiers":["standard"]
            }),
        ),
    ];
    let catalog = direct_source_model_catalog_with_capabilities(
        &home,
        &["vendor/synthetic".into()],
        &metadata,
        &endpoints,
    )
    .unwrap()
    .unwrap();
    let catalog: Value = serde_json::from_str(&catalog).unwrap();
    let model = &catalog["models"][0];
    assert_eq!(model["input_modalities"], json!(["text", "audio"]));
    assert_eq!(model["supports_parallel_tool_calls"], false);
    assert_eq!(model["max_context_window"], 2000);
    assert_eq!(
        model["supported_reasoning_levels"],
        json!([{"effort":"high","description":"high"}])
    );
    assert!(model.get("default_reasoning_level").is_none());
    assert_eq!(model["service_tiers"], json!([]));
    assert_eq!(model["additional_speed_tiers"], json!([]));
    assert_eq!(
        catalog["relay_context_limits"]["vendor/synthetic"],
        json!({
            "context":2000,"input":1800,"output":200
        })
    );
    let reference = metadata.capabilities_for("vendor/synthetic");
    assert_eq!(reference.context_limit, Some(100000));
    assert_eq!(reference.reasoning_effort_levels, ["low", "high"]);
    assert_eq!(reference.input_modalities, ["text", "image", "audio"]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_codex_streaming_denials_win_and_unknown_support_keeps_existing_models() {
    let (root, home, _backups) = profile_dirs("direct-codex-streaming");
    ensure_test_native_catalog(&home);
    for (reference, declared, expected) in [
        (Some(true), "unsupported", false),
        (Some(false), "confirmed", false),
        (None, "unknown", true),
    ] {
        let metadata = metadata(json!({"streaming":reference}));
        let catalog = direct_source_model_catalog_with_capabilities(
            &home,
            &["vendor/synthetic".into()],
            &metadata,
            &[endpoint(
                "responses",
                json!({"features":{"streaming":declared}}),
            )],
        )
        .unwrap();
        assert_eq!(catalog.is_some(), expected);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_codex_endpoint_claims_cannot_revive_reference_exclusions() {
    let (root, home, _backups) = profile_dirs("direct-codex-reference-exclusions");
    ensure_test_native_catalog(&home);
    let metadata = metadata(json!({
        "tool_call":false,"parallel_tool_call":true,"attachment":false,
        "modalities":{"input":["text","image"],"output":["text"]}
    }));
    let catalog = direct_source_model_catalog_with_capabilities(
        &home,
        &["vendor/synthetic".into()],
        &metadata,
        &[endpoint(
            "responses",
            json!({"features":{"images":"declared","function_tools":"declared"}}),
        )],
    )
    .unwrap()
    .unwrap();
    let catalog: Value = serde_json::from_str(&catalog).unwrap();
    assert_eq!(catalog["models"][0]["input_modalities"], json!(["text"]));
    assert_eq!(catalog["models"][0]["supports_parallel_tool_calls"], false);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn authoritative_empty_refresh_clears_only_owned_catalog_and_can_recover() {
    let (root, home, backups) = profile_dirs("codex-authoritative-empty-refresh");
    let secrets = MemorySecrets::default();
    let source = r#"{"models":[{"slug":"vendor/synthetic","use_responses_lite":true}]}"#;
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "synthetic-key",
        source,
        &secrets,
    )
    .unwrap();
    let catalog_path = managed_model_catalog_path(&backups).unwrap();
    let before_catalog = fs::read(&catalog_path).unwrap();
    let before_config = fs::read(home.join(CONFIG_FILE)).unwrap();
    let before_auth = fs::read(home.join(AUTH_FILE)).unwrap();
    assert!(refresh_managed_model_catalog(&home, &backups, r#"{"models":null}"#, None).is_err());
    assert_eq!(fs::read(&catalog_path).unwrap(), before_catalog);
    let empty = r#"{"models":[]}"#;
    assert!(refresh_managed_model_catalog(&home, &backups, empty, None).unwrap());
    let cleared: Value = serde_json::from_slice(&fs::read(&catalog_path).unwrap()).unwrap();
    assert_eq!(cleared["models"], json!([]));
    assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), before_config);
    assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), before_auth);
    assert!(!refresh_managed_model_catalog(&home, &backups, empty, None).unwrap());
    assert!(refresh_managed_model_catalog(&home, &backups, source, None).unwrap());
    restore_with(&home, &backups, &secrets).unwrap();
    fs::remove_dir_all(root).unwrap();
}
