use super::*;
use serde_json::json;

#[test]
fn relay_attach_clears_native_selection_and_named_profile_overrides() {
    let (root, home, backups) = profile_dirs("relay-clears-native-routing");
    let previous_config = r#"model_provider = "openai"
model = "gpt-native"
review_model = "gpt-native-review"
model_catalog_json = "native-catalog.json"
chatgpt_base_url = "https://chatgpt.example.com/v1"
openai_base_url = "https://openai.example.com/v1"
model_reasoning_effort = "high"

[profiles.work]
model_provider = "openai"
model = "profile-native"
model_catalog_json = "profile-native-catalog.json"
openai_base_url = "https://profile.example.com/v1"
"#;
    fs::write(home.join(CONFIG_FILE), previous_config).unwrap();
    let secrets = MemorySecrets::default();
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        r#"{"models":[{"slug":"vendor/relay-model"}]}"#,
        &secrets,
    )
    .unwrap();

    let attached = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(root_model_provider(&attached).as_deref(), Some(PROVIDER_ID));
    assert!(root_model(&attached).is_none());
    assert!(root_review_model(&attached).is_none());
    assert!(root_chatgpt_base_url(&attached).is_none());
    assert!(root_openai_base_url(&attached).is_none());
    assert!(attached["profiles"]["work"].get("model").is_none());
    assert!(attached["profiles"]["work"].get("model_provider").is_none());
    assert!(attached["profiles"]["work"]
        .get("model_catalog_json")
        .is_none());
    assert!(attached["profiles"]["work"]
        .get("openai_base_url")
        .is_none());
    let providers = attached["model_providers"].as_table_like().unwrap();
    assert_eq!(providers.len(), 1);
    assert!(providers.get(PROVIDER_ID).is_some());

    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        previous_config
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn relay_attach_deactivates_external_provider_but_keeps_its_definition() {
    let (root, home, backups) = profile_dirs("relay-clears-active-external-provider");
    let previous_config = r#"model_provider = "external_provider"
model = "external-model"

[model_providers.external_provider]
name = "External Provider"
base_url = "https://provider.example.com/v1"

[model_providers.custom]
name = "Custom"
base_url = "https://custom.example.com/v1"
"#;
    fs::write(home.join(CONFIG_FILE), previous_config).unwrap();
    let secrets = MemorySecrets::default();
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        r#"{"models":[{"slug":"vendor/relay-model"}]}"#,
        &secrets,
    )
    .unwrap();

    let attached = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(attached.contains("[model_providers.external_provider]"));
    assert!(attached.contains("[model_providers.custom]"));
    assert!(attached.contains("[model_providers.zenith_relay_local]"));

    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        previous_config
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_catalog_attach_and_restore_preserve_user_config_and_cache() {
    let (root, home, backups) = profile_dirs("model-catalog-restore");
    let previous_catalog_path = root.join("previous-codex-models.json");
    write_test_catalog_file(&previous_catalog_path, "native-user-model");
    let previous_catalog = previous_catalog_path.to_string_lossy().replace('\\', "/");
    fs::write(
        home.join(CONFIG_FILE),
        format!("model_provider = \"openai\"\nmodel_catalog_json = \"{previous_catalog}\"\n"),
    )
    .unwrap();
    let cache_path = home.join(MODELS_CACHE_FILE);
    let fresh_cache =
        r#"{"fetched_at":"2026-07-30T00:00:00Z","etag":"v1","models":[{"slug":"cached"}]}"#;
    fs::write(&cache_path, fresh_cache).unwrap();
    let secrets = MemorySecrets::default();
    let catalog = r#"{"models":[{"slug":"vendor/claude-opus-4-8","service_tiers":[{"id":"priority","name":"Fast","description":"Fast tier"}],"additional_speed_tiers":["fast"],"default_service_tier":"priority","default_reasoning_level":"high","supported_reasoning_levels":[{"effort":"low","description":"Low"},{"effort":"high","description":"High"},{"effort":"ultra","description":"Ultra"}],"supports_reasoning_summary_parameter":true,"supports_reasoning_summaries":true,"default_reasoning_summary":"detailed","supports_parallel_tool_calls":true}]}"#;

    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        catalog,
        &secrets,
    )
    .unwrap();

    let catalog_path = managed_model_catalog_path(&backups).unwrap();
    let attached = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(
        root_model_catalog_json(&attached).as_deref(),
        Some(portable_path_string(&catalog_path).as_ref())
    );
    let managed_catalog: Value =
        serde_json::from_str(&fs::read_to_string(&catalog_path).unwrap()).unwrap();
    let models = managed_catalog["models"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["slug"], "vendor/claude-opus-4-8");
    assert_ne!(models[0]["slug"], "native-user-model");
    assert_eq!(models[0]["default_reasoning_level"], "high");
    assert_eq!(
        models[0]["supported_reasoning_levels"][2]["effort"],
        "ultra"
    );
    assert_eq!(models[0]["service_tiers"], json!([]));
    assert_eq!(models[0]["additional_speed_tiers"], json!([]));
    assert!(models[0].get("default_service_tier").is_none());
    assert_eq!(models[0]["supports_reasoning_summary_parameter"], true);
    assert_eq!(models[0]["supports_reasoning_summaries"], true);
    assert_eq!(models[0]["default_reasoning_summary"], "detailed");
    assert_eq!(models[0]["supports_parallel_tool_calls"], true);
    assert!(!cache_path.exists());

    fs::write(&cache_path, fresh_cache).unwrap();
    restore_with(&home, &backups, &secrets).unwrap();

    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(
        root_model_catalog_json(&restored).as_deref(),
        Some(previous_catalog.as_str())
    );
    assert!(!catalog_path.exists());
    assert!(!cache_path.exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn direct_source_catalog_contains_only_selected_source_models_without_native_capabilities() {
    let (root, home, _backups) = profile_dirs("direct-source-catalog");
    let mut native = routed_codex_catalog_entry(None, "gpt-5.6-sol", 1, None);
    native["slug"] = Value::String("gpt-5.6-sol".into());
    native["display_name"] = Value::String("GPT-5.6 Sol".into());
    native["description"] = Value::String("Native test model".into());
    native["comp_hash"] = Value::String("official".into());
    native["default_reasoning_level"] = Value::String("low".into());
    native["supported_reasoning_levels"] = json!([
        {"effort": "low", "description": "Low"},
        {"effort": "ultra", "description": "Ultra"}
    ]);
    let mut relay_owned = routed_codex_catalog_entry(None, "gpt-fake", 2, None);
    relay_owned["slug"] = Value::String("gpt-fake".into());
    relay_owned["comp_hash"] = Value::String(CODEX_RELAY_CATALOG_HASH.into());
    fs::write(
        home.join(MODELS_CACHE_FILE),
        serde_json::to_string_pretty(&json!({"models": [native, relay_owned]})).unwrap(),
    )
    .unwrap();

    let catalog = direct_source_model_catalog(
        &home,
        &[
            "gpt-5.6-sol".into(),
            "vendor/claude".into(),
            "gpt-fake".into(),
            "zenith/alias".into(),
        ],
    )
    .unwrap()
    .expect("catalog");
    let models = serde_json::from_str::<Value>(&catalog).unwrap()["models"]
        .as_array()
        .unwrap()
        .clone();

    assert_eq!(models.len(), 3);
    assert_eq!(models[0]["slug"], "gpt-5.6-sol");
    assert_eq!(models[1]["slug"], "vendor/claude");
    assert_eq!(models[2]["slug"], "gpt-fake");
    assert_eq!(
        models
            .iter()
            .map(|model| model["priority"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1_000, 1_001, 1_002]
    );
    assert!(models[0].get("default_reasoning_level").is_none());
    assert_eq!(models[0]["supported_reasoning_levels"], json!([]));
    for model in &models[1..] {
        assert!(model.get("default_reasoning_level").is_none());
        assert_eq!(model["supported_reasoning_levels"], json!([]));
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn direct_source_catalog_preserves_each_models_declared_modalities() {
    let (root, home, _backups) = profile_dirs("direct-source-image-capability");
    let manifest = json!({
        "data": [
            {
                "id": "provider/vision",
                "input_modalities": ["text", "image"]
            },
            {
                "id": "provider/text",
                "input_modalities": ["text"]
            }
        ]
    });

    let catalog = direct_source_model_catalog_with_manifest(
        &home,
        &["provider/vision".into(), "provider/text".into()],
        Some(&manifest),
    )
    .unwrap()
    .expect("catalog");
    let parsed_catalog = serde_json::from_str::<Value>(&catalog).unwrap();
    let models = parsed_catalog["models"].as_array().unwrap();

    assert_eq!(models[0]["slug"], "provider/vision");
    assert_eq!(models[0]["input_modalities"], json!(["text", "image"]));
    assert_eq!(models[1]["slug"], "provider/text");
    assert_eq!(models[1]["input_modalities"], json!(["text"]));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn direct_source_catalog_resolves_the_configured_relative_template() {
    let (root, home, _backups) = profile_dirs("direct-source-relative-template");
    write_test_catalog_file(&home.join("native-catalog.json"), "gpt-5.6-sol");
    fs::write(
        home.join(CONFIG_FILE),
        "model_catalog_json = \"native-catalog.json\"\n",
    )
    .unwrap();

    let catalog = direct_source_model_catalog(&home, &["vendor/claude-opus".into()])
        .unwrap()
        .expect("catalog");
    let models = serde_json::from_str::<Value>(&catalog).unwrap()["models"]
        .as_array()
        .unwrap()
        .clone();

    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["slug"], "vendor/claude-opus");
    assert_eq!(models[0]["supported_reasoning_levels"], json!([]));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn managed_catalog_preserves_native_model_settings() {
    let (root, home, _backups) = profile_dirs("managed-native-settings");
    let mut native = routed_codex_catalog_entry(None, "gpt-native", 1, None);
    native["slug"] = Value::String("gpt-native".into());
    native["comp_hash"] = Value::String("official".into());
    native["display_name"] = json!("Native model title");
    native["input_modalities"] = json!(["text", "image"]);
    native["default_reasoning_level"] = Value::String("ultra".into());
    native["supported_reasoning_levels"] = json!([
        {"effort": "low", "description": "Low"},
        {"effort": "ultra", "description": "Ultra"}
    ]);
    native["service_tiers"] = json!([{
        "id": "priority",
        "name": "Fast",
        "description": "Native fast tier"
    }]);
    native["default_service_tier"] = Value::String("priority".into());
    native["context_window"] = 128_000.into();
    native["max_context_window"] = 120_000.into();
    native["auto_compact_token_limit"] = 110_000.into();
    native["native_setting"] = Value::String("keep-me".into());
    let catalog = serde_json::to_string(&json!({"models": [native]})).unwrap();

    let managed = catalog::build_managed_model_catalog(&home, None, None, &catalog).unwrap();
    let model = &serde_json::from_str::<Value>(&managed).unwrap()["models"][0];

    assert_eq!(model["display_name"], "Native model title");
    assert_eq!(model["input_modalities"], json!(["text", "image"]));
    assert_eq!(model["default_reasoning_level"], "ultra");
    assert_eq!(
        model["supported_reasoning_levels"],
        json!([
            {"effort": "low", "description": "Low"},
            {"effort": "ultra", "description": "Ultra"}
        ])
    );
    assert_eq!(model["service_tiers"][0]["id"], "priority");
    assert_eq!(model["default_service_tier"], "priority");
    assert_eq!(model["context_window"], 128_000);
    assert_eq!(model["max_context_window"], 120_000);
    assert_eq!(model["auto_compact_token_limit"], 110_000);
    assert_eq!(model["native_setting"], "keep-me");
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn managed_gpt_catalog_refresh_keeps_native_ids_without_copying_cached_capabilities() {
    let (root, home, _backups) = profile_dirs("managed-gpt-identity-refresh");
    let mut cached = routed_codex_catalog_entry(None, "gpt-6-astra", 1_000, None);
    cached["slug"] = json!("gpt-6-astra");
    cached["comp_hash"] = json!("official");
    cached["default_reasoning_level"] = json!("ultra");
    cached["supported_reasoning_levels"] = json!([{"effort": "ultra"}]);
    cached["supports_parallel_tool_calls"] = json!(true);
    fs::write(
        home.join(MODELS_CACHE_FILE),
        json!({"models": [cached]}).to_string(),
    )
    .unwrap();
    let ids = ["gpt-6-astra", "gpt-5.6-sol"];
    let previous = json!({"models": ids.iter().map(|id|
        routed_codex_catalog_entry(None, id, 1_000, None)
    ).collect::<Vec<_>>()})
    .to_string();
    let expected = ids
        .iter()
        .map(|id| {
            let mut model = routed_codex_catalog_entry(None, id, 1_000, None);
            model["slug"] = json!(id);
            zenith_relay_core::model_metadata::ModelCapabilities::unknown_model()
                .apply_to_codex(&mut model);
            model
        })
        .collect::<Vec<_>>();
    let catalog = json!({"models": expected}).to_string();

    let managed =
        catalog::build_managed_model_catalog(&home, None, Some(previous.as_bytes()), &catalog)
            .unwrap();
    let document: Value = serde_json::from_str(&managed).unwrap();
    let models = document["models"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    for ((model, id), label) in models.iter().zip(ids).zip(["6 Astra", "5.6 Sol"]) {
        assert_eq!(model["slug"], id);
        assert_eq!(model["display_name"], label);
        assert_eq!(model["comp_hash"], CODEX_RELAY_CATALOG_HASH);
        assert_eq!(model["supported_reasoning_levels"], json!([]));
        assert_eq!(model["supports_parallel_tool_calls"], true);
        assert!(model.get("context_window").is_none());
        assert!(model.get("default_reasoning_level").is_none());
        assert!(!catalog::is_native_catalog_entry(model));
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn generated_catalogs_do_not_require_cached_native_metadata() {
    let (root, home, _backups) = profile_dirs("catalog-metadata-fallback");

    let direct = direct_source_model_catalog(&home, &["vendor/direct".into()])
        .unwrap()
        .expect("direct catalog");
    assert_eq!(
        serde_json::from_str::<Value>(&direct).unwrap()["models"][0]["slug"],
        "vendor/direct"
    );

    let managed = catalog::build_managed_model_catalog(
        &home,
        None,
        None,
        r#"{"models":[{"slug":"vendor/managed"}]}"#,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&managed).unwrap()["models"][0]["slug"],
        "vendor/managed"
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn managed_catalog_does_not_add_context_to_an_incomplete_native_row() {
    let (root, home, _backups) = profile_dirs("managed-native-context-fallback");

    let managed = catalog::build_managed_model_catalog(
        &home,
        None,
        None,
        r#"{"models":[{"slug":"gpt-native","display_name":null}]}"#,
    )
    .unwrap();
    let model = &serde_json::from_str::<Value>(&managed).unwrap()["models"][0];

    assert_eq!(model["slug"], "gpt-native");
    assert!(model.get("context_window").is_none());
    assert!(model.get("max_context_window").is_none());
    assert!(model.get("auto_compact_token_limit").is_none());
    assert!(model.get("effective_context_window_percent").is_none());

    fs::remove_dir_all(root).unwrap();
}
#[test]
fn active_managed_catalog_refreshes_without_replacing_the_profile() {
    let (root, home, backups) = profile_dirs("model-catalog-refresh");
    let user_config = concat!(
        "model_context_window = 200000\n",
        "model_auto_compact_token_limit = 190000\n",
        "\n[profiles.short]\n",
        "model_context_window = 128000\n",
        "model_auto_compact_token_limit = 110000\n",
    );
    fs::write(home.join(CONFIG_FILE), user_config).unwrap();
    let cache_path = home.join(MODELS_CACHE_FILE);
    fs::write(
        &cache_path,
        r#"{"fetched_at":"2026-07-30T00:00:00Z","etag":"v1","models":[]}"#,
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        r#"{"models":[{"slug":"old-model"}]}"#,
        &secrets,
    )
    .unwrap();

    let attached_config = fs::read(home.join(CONFIG_FILE)).unwrap();
    let attached_auth = fs::read(home.join(AUTH_FILE)).unwrap();
    let attached = parse_config(std::str::from_utf8(&attached_config).unwrap()).unwrap();
    assert_eq!(attached["model_context_window"].as_integer(), Some(200_000));
    assert_eq!(
        attached["model_auto_compact_token_limit"].as_integer(),
        Some(190_000)
    );
    assert_eq!(
        attached["profiles"]["short"]["model_context_window"].as_integer(),
        Some(128_000)
    );
    assert_eq!(
        attached["profiles"]["short"]["model_auto_compact_token_limit"].as_integer(),
        Some(110_000)
    );
    assert!(refresh_managed_model_catalog(
        &home,
        &backups,
        r#"{"models":[{"slug":"new-model"}]}"#,
        None
    )
    .unwrap());
    assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), attached_config);
    assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), attached_auth);
    let catalog_path = managed_model_catalog_path(&backups).unwrap();
    let catalog: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(catalog_path).unwrap()).unwrap();
    assert!(catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .any(|model| model["slug"] == "new-model"));
    assert!(!cache_path.exists());
    assert!(!refresh_managed_model_catalog(
        &home,
        &backups,
        r#"{"models":[{"slug":"new-model"}]}"#,
        None,
    )
    .unwrap());
    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        user_config
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn stale_catalog_refresh_cannot_replace_a_different_profile_binding() {
    let (root, home, backups) = profile_dirs("catalog-owner-change");
    let secrets = MemorySecrets::default();
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        r#"{"models":[{"slug":"current-model"}]}"#,
        &secrets,
    )
    .unwrap();
    let current = profile_bindings(&home, &backups)
        .unwrap()
        .into_iter()
        .find(|binding| binding.active)
        .unwrap();
    let catalog_path = managed_model_catalog_path(&backups).unwrap();
    let before = fs::read(&catalog_path).unwrap();
    for account_changed in [false, true] {
        let mut stale = current.clone();
        if account_changed {
            stale.bound_oauth_account_id = Some("synthetic-previous-account".into());
        } else {
            stale.credential_id = "synthetic-previous-key".into();
        }
        assert!(!refresh_managed_model_catalog(
            &home,
            &backups,
            r#"{"models":[{"slug":"stale-model"}]}"#,
            Some(&stale)
        )
        .unwrap());
        assert_eq!(fs::read(&catalog_path).unwrap(), before);
    }
    assert!(refresh_managed_model_catalog(
        &home,
        &backups,
        r#"{"models":[{"slug":"new-model"}]}"#,
        Some(&current)
    )
    .unwrap());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn repeated_oauth_pool_attach_refreshes_missing_and_empty_speed_tiers() {
    let (root, home, backups) = profile_dirs("reattach-speed-tiers");
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new(
        "fixture-access",
        Some("fixture-refresh".into()),
        Some("fixture-id".into()),
        None,
        1,
        1,
    )
    .unwrap();
    let mut models = ["gpt-future-a", "gpt-future-b"]
        .iter()
        .map(|id| {
            let mut model = routed_codex_catalog_entry(None, id, 1_000, None);
            model["slug"] = json!(id);
            model
        })
        .collect::<Vec<_>>();
    for key in ["service_tiers", "additional_speed_tiers"] {
        models[0].as_object_mut().unwrap().remove(key);
        models[1][key] = json!([]);
    }
    let attach = |catalog: &str| {
        switch_to_local_with(
            &home,
            &backups,
            "fixture-key-id",
            "http://127.0.0.1:14998/v1",
            "fixture-key",
            LocalAttachOptions {
                catalog_json: Some(catalog),
                bound_oauth: Some(BoundOAuthProfile {
                    account_id: "fixture-account",
                    tokens: &tokens,
                    provider_account_id: "fixture-provider-account",
                }),
                ..LocalAttachOptions::default()
            },
            &secrets,
        )
        .unwrap()
    };
    attach(&json!({"models": models}).to_string());
    let auth_before = fs::read(home.join(AUTH_FILE)).unwrap();
    let config_before = fs::read(home.join(CONFIG_FILE)).unwrap();
    fs::write(
        home.join(MODELS_CACHE_FILE),
        json!({"models": models}).to_string(),
    )
    .unwrap();
    let tiers = json!([
        {"id": "priority", "name": "Fast", "description": ""},
        {"id": "ultrafast", "name": "Ultrafast", "description": ""}
    ]);
    for model in &mut models {
        model["service_tiers"] = tiers.clone();
        model["additional_speed_tiers"] = json!(["fast", "ultrafast"]);
    }
    let binding = attach(&json!({"models": models}).to_string());
    assert!(binding.active);
    assert_eq!(
        binding.bound_oauth_account_id.as_deref(),
        Some("fixture-account")
    );
    assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), auth_before);
    assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), config_before);
    assert!(!home.join(MODELS_CACHE_FILE).exists());
    let catalog_path = managed_model_catalog_path(&backups).unwrap();
    let saved = fs::read(&catalog_path).unwrap();
    let catalog: Value = serde_json::from_slice(&saved).unwrap();
    for (index, model) in catalog["models"].as_array().unwrap().iter().enumerate() {
        assert_eq!(model["slug"], models[index]["slug"]);
        assert_eq!(model["service_tiers"], tiers);
        assert_eq!(
            model["additional_speed_tiers"],
            json!(["fast", "ultrafast"])
        );
    }
    let backup = local_backup(&home, &backups).unwrap().unwrap();
    assert!(valid_managed_model_catalog(
        &backup,
        &catalog_path,
        &Some(saved)
    ));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn catalog_refresh_recovers_an_interrupted_catalog_commit() {
    let (root, home, backups) = profile_dirs("model-catalog-interrupted-refresh");
    let secrets = MemorySecrets::default();
    let next_source_catalog = r#"{"models":[{"slug":"new-model"}]}"#;
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        r#"{"models":[{"slug":"old-model"}]}"#,
        &secrets,
    )
    .unwrap();

    let catalog_path = managed_model_catalog_path(&backups).unwrap();
    let backup_path = backup_path(&backups);
    let previous_catalog = fs::read(&catalog_path).unwrap();
    let backup_bytes = read_optional_bytes(&backup_path).unwrap();
    let mut backup = parse_backup_snapshot(&backup_bytes, &backup_path)
        .unwrap()
        .expect("profile backup");
    let next_catalog = catalog::build_managed_model_catalog(
        &home,
        backup.previous_model_catalog_json.as_deref(),
        Some(&previous_catalog),
        next_source_catalog,
    )
    .unwrap();

    // Simulate a process stop after the catalog is written but before its
    // pending backup metadata is committed.
    backup.managed_model_catalog_pending_hash = Some(key_hash(&next_catalog));
    backup.managed_model_catalog_pending_remove = false;
    fs::write(&catalog_path, &next_catalog).unwrap();
    fs::write(&backup_path, serialize_backup(&backup).unwrap()).unwrap();

    assert!(
        !refresh_managed_model_catalog(&home, &backups, next_source_catalog, None).unwrap(),
        "the recovered catalog already matches the requested catalog"
    );
    let recovered = local_backup(&home, &backups)
        .unwrap()
        .expect("recovered backup");
    assert_eq!(
        recovered.managed_model_catalog_hash.as_deref(),
        Some(key_hash(&next_catalog).as_str())
    );
    assert!(recovered.managed_model_catalog_pending_hash.is_none());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn first_catalog_upgrade_preserves_legacy_user_catalog() {
    let (root, home, backups) = profile_dirs("legacy-model-catalog");
    let previous_catalog_path = root.join("legacy-models.json");
    write_test_catalog_file(&previous_catalog_path, "legacy-native-model");
    let previous_catalog = previous_catalog_path.to_string_lossy().replace('\\', "/");
    fs::write(
        home.join(CONFIG_FILE),
        format!("model_catalog_json = \"{previous_catalog}\"\n"),
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_old_key",
        &secrets,
    )
    .unwrap();
    let backup_path = backup_path(&backups);
    let mut legacy: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
    let object = legacy.as_object_mut().unwrap();
    object.remove("previousModelCatalogJson");
    object.remove("managedModelCatalogPath");
    object.remove("managedModelCatalogHash");
    fs::write(&backup_path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_new_key",
        r#"{"models":[{"slug":"vendor/model"}]}"#,
        &secrets,
    )
    .unwrap();
    restore_with(&home, &backups, &secrets).unwrap();

    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(
        root_model_catalog_json(&restored).as_deref(),
        Some(previous_catalog.as_str())
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn legacy_relay_catalog_metadata_is_adopted_without_overwriting_an_external_catalog() {
    let (root, home, backups) = profile_dirs("legacy-managed-catalog-metadata");
    let external_config =
        "model_provider = \"custom\"\nmodel_catalog_json = \"custom-catalog.json\"\n";
    write_test_catalog_file(&home.join("custom-catalog.json"), "native-user-model");
    fs::write(home.join(CONFIG_FILE), external_config).unwrap();
    let secrets = MemorySecrets::default();
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        r#"{"models":[{"slug":"vendor/model"}]}"#,
        &secrets,
    )
    .unwrap();

    let backup_path = backup_path(&backups);
    let mut legacy: Value =
        serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
    let object = legacy.as_object_mut().unwrap();
    for field in [
        "previousModelCatalogJson",
        "managedModelCatalogPath",
        "managedModelCatalogHash",
        "managedModelCatalogPendingHash",
        "managedModelCatalogPendingRemove",
    ] {
        object.remove(field);
    }
    fs::write(&backup_path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();
    fs::write(home.join(CONFIG_FILE), external_config).unwrap();

    let backup = local_backup(&home, &backups).unwrap().expect("backup");
    let catalog_path = managed_model_catalog_path(&backups).unwrap();
    assert_eq!(
        backup.managed_model_catalog_path.as_deref(),
        Some(portable_path_string(&catalog_path).as_ref())
    );
    assert_eq!(
        backup.managed_model_catalog_hash.as_deref(),
        Some(bytes_hash(&fs::read(&catalog_path).unwrap()).as_str())
    );
    assert_eq!(
        backup.previous_model_catalog_json.as_deref(),
        Some("custom-catalog.json")
    );
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        external_config
    );

    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        external_config
    );
    assert!(!catalog_path.exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn legacy_catalog_without_the_relay_marker_is_not_adopted() {
    let (root, home, backups) = profile_dirs("legacy-unowned-catalog-metadata");
    let secrets = MemorySecrets::default();
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        r#"{"models":[{"slug":"vendor/model"}]}"#,
        &secrets,
    )
    .unwrap();

    let backup_path = backup_path(&backups);
    let mut legacy: Value =
        serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
    let object = legacy.as_object_mut().unwrap();
    for field in [
        "managedModelCatalogPath",
        "managedModelCatalogHash",
        "managedModelCatalogPendingHash",
        "managedModelCatalogPendingRemove",
    ] {
        object.remove(field);
    }
    fs::write(&backup_path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

    let catalog_path = managed_model_catalog_path(&backups).unwrap();
    let mut catalog: Value =
        serde_json::from_str(&fs::read_to_string(&catalog_path).unwrap()).unwrap();
    for model in catalog["models"].as_array_mut().unwrap() {
        model["comp_hash"] = Value::String("external-catalog".into());
    }
    fs::write(
        &catalog_path,
        serde_json::to_string_pretty(&catalog).unwrap(),
    )
    .unwrap();
    let original_backup = fs::read(&backup_path).unwrap();

    let error = local_backup(&home, &backups).unwrap_err();
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert_eq!(fs::read(&backup_path).unwrap(), original_backup);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn missing_managed_catalog_is_migrated_for_safe_restore() {
    let (root, home, backups) = profile_dirs("missing-managed-catalog-restore");
    let secrets = MemorySecrets::default();
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        r#"{"models":[{"slug":"vendor/model"}]}"#,
        &secrets,
    )
    .unwrap();

    let catalog_path = managed_model_catalog_path(&backups).unwrap();
    fs::remove_file(&catalog_path).unwrap();

    let backup = local_backup(&home, &backups)
        .unwrap()
        .expect("profile backup");
    assert!(backup.restore_pending);
    assert!(valid_managed_model_catalog(&backup, &catalog_path, &None));

    restore_with(&home, &backups, &secrets).unwrap();
    assert!(!backup_path(&backups).exists());
    // The profile started without config.toml; restoring absence must not
    // manufacture an empty configuration file.
    assert!(!home.join(CONFIG_FILE).exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn externally_edited_managed_catalog_does_not_trap_profile_recovery() {
    for (case, external_catalog) in [
        ("json", r#"{"models":[{"slug":"external-model"}]}"#),
        ("other", "externally changed"),
    ] {
        let (root, home, backups) = profile_dirs(&format!("edited-managed-catalog-{case}"));
        let original_config = "model_provider = 'openai'\nuser_setting = 'keep'\n";
        let original_auth = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"original"}}"#;
        fs::write(home.join(CONFIG_FILE), original_config).unwrap();
        fs::write(home.join(AUTH_FILE), original_auth).unwrap();
        let secrets = MemorySecrets::default();
        attach_with_catalog_for_test(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            r#"{"models":[{"slug":"managed-model"}]}"#,
            &secrets,
        )
        .unwrap();

        let catalog_path = managed_model_catalog_path(&backups).unwrap();
        fs::write(&catalog_path, external_catalog).unwrap();
        assert!(local_backup(&home, &backups).unwrap().is_some());
        assert_eq!(
            credential_kind_locked(&home, &backups).unwrap(),
            Some(ProfileCredentialKind::LocalGateway)
        );
        assert_eq!(profile_bindings(&home, &backups).unwrap().len(), 1);
        let error = refresh_managed_model_catalog(
            &home,
            &backups,
            r#"{"models":[{"slug":"new-model"}]}"#,
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::ProfileRestoreBlocked);
        assert_eq!(fs::read_to_string(&catalog_path).unwrap(), external_catalog);

        restore_with(&home, &backups, &secrets).unwrap();
        assert_eq!(
            fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
            original_config
        );
        assert_eq!(
            fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
            original_auth
        );
        assert_eq!(fs::read_to_string(&catalog_path).unwrap(), external_catalog);
        assert!(!backup_path(&backups).exists());
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn changed_catalog_and_newer_login_survive_restore() {
    let (root, home, backups) = profile_dirs("edited-catalog-newer-login");
    let secrets = MemorySecrets::default();
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        r#"{"models":[{"slug":"managed-model"}]}"#,
        &secrets,
    )
    .unwrap();
    let catalog_path = managed_model_catalog_path(&backups).unwrap();
    let external_catalog = "externally changed";
    fs::write(&catalog_path, external_catalog).unwrap();
    let fresh_auth = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"fresh"}}"#;
    fs::write(home.join(AUTH_FILE), fresh_auth).unwrap();
    restore_with(&home, &backups, &secrets).unwrap();
    assert!(!fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap_or_default()
        .contains(PROVIDER_ID));
    assert_eq!(
        fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
        fresh_auth
    );
    assert_eq!(fs::read_to_string(&catalog_path).unwrap(), external_catalog);
    assert!(!backup_path(&backups).exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn invalid_catalog_backup_path_still_requires_recovery() {
    let (root, home, backups) = profile_dirs("invalid-managed-catalog-path");
    let secrets = MemorySecrets::default();
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        r#"{"models":[{"slug":"managed-model"}]}"#,
        &secrets,
    )
    .unwrap();
    let catalog_path = managed_model_catalog_path(&backups).unwrap();
    fs::write(&catalog_path, "externally changed").unwrap();
    let backup_path = backup_path(&backups);
    let mut backup: Value = serde_json::from_slice(&fs::read(&backup_path).unwrap()).unwrap();
    backup["managedModelCatalogPath"] = Value::String("other-catalog.json".into());
    fs::write(&backup_path, serde_json::to_vec(&backup).unwrap()).unwrap();

    let error = local_backup(&home, &backups).unwrap_err();
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert!(error.message.contains("model catalog reference"));
    assert_eq!(
        fs::read_to_string(&catalog_path).unwrap(),
        "externally changed"
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn snapshot_discard_removes_only_an_unchanged_managed_catalog() {
    let catalog = r#"{"models":[{"slug":"vendor/model"}]}"#;
    for changed in [false, true] {
        let (root, home, backups) = profile_dirs(if changed {
            "discard-changed-catalog"
        } else {
            "discard-managed-catalog"
        });
        let secrets = MemorySecrets::default();
        attach_with_catalog_for_test(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            catalog,
            &secrets,
        )
        .unwrap();
        let catalog_path = managed_model_catalog_path(&backups).unwrap();
        if changed {
            fs::write(&catalog_path, "externally changed").unwrap();
        }

        discard_managed_binding_locked(&home, &backups, &secrets).unwrap();

        assert_eq!(catalog_path.exists(), changed);
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn account_switch_clears_the_current_catalog_for_native_codex_models() {
    let (root, home, backups) = profile_dirs("oauth-account-native-catalog");
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new("access", Some("refresh".into()), None, None, 1, 1).unwrap();
    fs::write(
        home.join(CONFIG_FILE),
        "model = \"gpt-5.6-sol\"\nmodel_provider = \"openai\"\nmodel_catalog_json = \"official-catalog.json\"\n",
    )
    .unwrap();

    attach_account_with(
        &home,
        &backups,
        "account-native-catalog",
        &tokens,
        "provider-account",
        &secrets,
    )
    .unwrap();

    let attached = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert!(attached.get("model").is_none());
    assert_eq!(root_model_provider(&attached).as_deref(), Some("openai"));
    assert!(root_model_catalog_json(&attached).is_none());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn direct_source_catalog_publishes_known_limits_without_client_context_policy() {
    let (root, home, _backups) = profile_dirs("direct-source-models-dev");
    ensure_test_native_catalog(&home);
    let metadata = ModelMetadataCatalog::from_models_dev_json(r#"{
        "test/text-only":{"name":"Catalog Display Name","modalities":{"input":["text"],"output":["text"]},
        "reasoning":true,"reasoning_effort_levels":["low","high"],"tool_call":true,"limit":{"context":64000}}
    }"#).unwrap();
    let catalog = direct_source_model_catalog_with_capabilities(
        &home,
        &["text-only".into(), "unknown".into()],
        &metadata,
    )
    .unwrap()
    .unwrap();
    let value: Value = serde_json::from_str(&catalog).unwrap();
    assert_eq!(value["models"][0]["display_name"], "Catalog Display Name");
    assert_eq!(value["models"][0]["slug"], "text-only");
    assert_eq!(value["models"][1]["display_name"], "Unknown");
    assert_eq!(value["models"][0]["input_modalities"], json!(["text"]));
    assert!(value["models"][0].get("context_window").is_none());
    assert_eq!(value["models"][0]["max_context_window"], 64_000);
    assert!(value["models"][0].get("auto_compact_token_limit").is_none());
    assert!(value["models"][0]
        .get("effective_context_window_percent")
        .is_none());
    assert_eq!(
        value["models"][1]["input_modalities"],
        json!(["text", "image"])
    );
    assert_eq!(value["models"][1]["supported_reasoning_levels"], json!([]));
    assert!(value["models"][1].get("context_window").is_none());
    assert!(value["models"][1].get("auto_compact_token_limit").is_none());
    let image = direct_source_model_catalog(
        &home,
        &["gpt-image-2".into(), "vendor/degrade2-model".into()],
    )
    .unwrap()
    .expect("image and degraded ids stay in a direct catalog");
    let image: Value = serde_json::from_str(&image).unwrap();
    assert_eq!(image["models"][0]["slug"], "gpt-image-2");
    assert_eq!(image["models"][1]["slug"], "vendor/degrade2-model");
    let managed = catalog::build_managed_model_catalog(&home, None, None, &catalog).unwrap();
    let managed: Value = serde_json::from_str(&managed).unwrap();
    assert_eq!(managed["models"], value["models"]);
    fs::remove_dir_all(root).unwrap();
}
