use super::*;
use serde_json::json;

#[test]
fn local_gateway_preserves_global_reasoning_override_and_restores_it() {
    let (root, home, backups) = profile_dirs("reasoning-effort-override");
    fs::write(
        home.join(CONFIG_FILE),
        "model_provider = \"openai\"\nmodel_reasoning_effort = \"ultra\"\n",
    )
    .unwrap();
    let secrets = MemorySecrets::default();

    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();

    let managed_config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(managed_config.contains("model_reasoning_effort = \"ultra\""));
    let backup = local_backup(&home, &backups)
        .unwrap()
        .expect("profile backup");
    assert_eq!(
        backup.previous_model_reasoning_effort.as_deref(),
        Some("ultra")
    );
    assert!(backup.managed_model_reasoning_effort_cleared);

    restore_with(&home, &backups, &secrets).unwrap();
    let restored_config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(restored_config.contains("model_provider = \"openai\""));
    assert!(restored_config.contains("model_reasoning_effort = \"ultra\""));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn local_gateway_writes_catalog_reasoning_default_and_removes_it_on_restore() {
    let (root, home, backups) = profile_dirs("catalog-reasoning-effort");
    fs::write(
        home.join(CONFIG_FILE),
        "model = \"vendor/claude-opus-4-8\"\n",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let catalog = r#"{"models":[{"slug":"vendor/claude-opus-4-8","service_tiers":[],"additional_speed_tiers":[],"default_service_tier":null,"default_reasoning_level":"high","supported_reasoning_levels":[{"effort":"low","description":"Low"},{"effort":"high","description":"High"},{"effort":"ultra","description":"Ultra"}],"supports_reasoning_summary_parameter":true,"supports_reasoning_summaries":true,"default_reasoning_summary":"none","supports_parallel_tool_calls":true}]}"#;

    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        catalog,
        &secrets,
    )
    .unwrap();

    let managed_config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(managed_config.contains("model_reasoning_effort = \"high\""));
    let backup = local_backup(&home, &backups)
        .unwrap()
        .expect("profile backup");
    assert_eq!(
        backup.managed_model_reasoning_effort.as_deref(),
        Some("high")
    );

    restore_with(&home, &backups, &secrets).unwrap();
    let restored_config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(!restored_config.contains("model_reasoning_effort"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn restore_preserves_reasoning_override_added_while_relay_is_active() {
    let (root, home, backups) = profile_dirs("reasoning-effort-user-override");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    let secrets = MemorySecrets::default();
    let test_key = format!("test-key-{}", std::process::id());

    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        &test_key,
        &secrets,
    )
    .unwrap();

    let mut managed = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    managed["model_reasoning_effort"] = value("ultra");
    fs::write(home.join(CONFIG_FILE), managed.to_string()).unwrap();

    restore_with(&home, &backups, &secrets).unwrap();

    let restored = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(restored.contains("model_provider = \"openai\""));
    assert!(restored.contains("model_reasoning_effort = \"ultra\""));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn direct_source_catalog_converts_provider_reasoning_metadata() {
    let (root, home, _backups) = profile_dirs("direct-source-reasoning-metadata");
    let manifest = json!({
        "data": [
            {
                "id": "provider/reasoning",
                "reasoningEffortModes": ["low", "medium", "high"],
                "defaultReasoningLevel": "high"
            },
            {
                "id": "gpt-5.6-sol",
                "reasoningEffortModes": []
            }
        ]
    });

    let catalog = direct_source_model_catalog_with_manifest(
        &home,
        &["provider/reasoning".into(), "gpt-5.6-sol".into()],
        Some(&manifest),
    )
    .unwrap()
    .expect("direct catalog");
    let models = serde_json::from_str::<Value>(&catalog).unwrap()["models"]
        .as_array()
        .unwrap()
        .clone();

    assert_eq!(
        models[0]["supported_reasoning_levels"],
        json!([
            {"effort": "low", "description": "low"},
            {"effort": "medium", "description": "medium"},
            {"effort": "high", "description": "high"}
        ])
    );
    assert_eq!(models[0]["default_reasoning_level"], "medium");
    assert_eq!(models[1]["supported_reasoning_levels"], json!([]));
    assert!(models[1].get("default_reasoning_level").is_none());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn attach_removes_unsupported_persistent_reasoning_effort() {
    let mut document: DocumentMut = r#"
[desktop]
enabled-reasoning-efforts = ["low", "persistent", "high"]
"#
    .parse()
    .unwrap();

    attach_config(
        &mut document,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        None,
        None,
        None,
        false,
    );

    assert_eq!(
        document["desktop"]["enabled-reasoning-efforts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|effort| effort.as_str())
            .collect::<Vec<_>>(),
        ["low", "high"]
    );
}
#[test]
fn attach_enables_ultra_picker_without_replacing_an_explicit_true() {
    let mut absent: DocumentMut = "model = \"gpt-6-sol\"\n".parse().unwrap();
    attach_config(
        &mut absent,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        None,
        None,
        None,
        false,
    );
    assert_eq!(
        absent["desktop"][DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY].as_bool(),
        Some(true)
    );

    let mut disabled: DocumentMut = "[desktop]\nshow-ultra-in-model-picker-slider = false\n"
        .parse()
        .unwrap();
    attach_config(
        &mut disabled,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        None,
        None,
        None,
        false,
    );
    assert_eq!(
        disabled["desktop"][DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY].as_bool(),
        Some(true)
    );

    let mut enabled: DocumentMut =
        "[desktop]\nshow-ultra-in-model-picker-slider = true\nother = \"keep\"\n"
            .parse()
            .unwrap();
    attach_config(
        &mut enabled,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        None,
        None,
        None,
        false,
    );
    assert_eq!(
        enabled["desktop"][DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY].as_bool(),
        Some(true)
    );
    assert_eq!(enabled["desktop"]["other"].as_str(), Some("keep"));
}
#[test]
fn attach_restores_the_previous_ultra_picker_switch() {
    let (root, home, backups) = profile_dirs("ultra-picker-switch");
    fs::write(
        home.join(CONFIG_FILE),
        "model_provider = \"openai\"\n\n[desktop]\nshow-ultra-in-model-picker-slider = false\n",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();
    let attached = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(
        attached["desktop"][DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY].as_bool(),
        Some(true)
    );

    let mut turned_off =
        parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    turned_off["desktop"][DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY] = value(false);
    fs::write(home.join(CONFIG_FILE), turned_off.to_string()).unwrap();
    restore_with(&home, &backups, &secrets).unwrap();
    let kept = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(kept["model_provider"].as_str(), Some("openai"));
    assert_eq!(
        kept["desktop"][DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY].as_bool(),
        Some(false)
    );

    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();
    restore_with(&home, &backups, &secrets).unwrap();
    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(restored["model_provider"].as_str(), Some("openai"));
    assert!(restored
        .get("desktop")
        .and_then(Item::as_table_like)
        .and_then(|desktop| desktop.get(DESKTOP_SHOW_ULTRA_IN_MODEL_PICKER_KEY))
        .is_none());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn direct_source_catalog_uses_medium_for_automatic_reasoning() {
    let (root, home, _backups) = profile_dirs("direct-source-reasoning-default");
    let manifest = json!({
        "models": [{
            "slug": "provider/reasoning",
            "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [
                {"effort": "low", "description": "Low"},
                {"effort": "medium", "description": "Medium"},
                {"effort": "ultra", "description": "Ultra"}
            ]
        }]
    });

    let catalog = direct_source_model_catalog_with_manifest(
        &home,
        &["provider/reasoning".into()],
        Some(&manifest),
    )
    .unwrap()
    .expect("direct catalog");
    let model = &serde_json::from_str::<Value>(&catalog).unwrap()["models"][0];

    assert_eq!(model["default_reasoning_level"], "medium");
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn managed_direct_source_catalog_does_not_restore_provider_ultra_default() {
    let (root, home, _backups) = profile_dirs("managed-direct-source-reasoning-default");
    let manifest = json!({
        "models": [{
            "slug": "provider/reasoning",
            "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [
                {"effort": "low", "description": "Low"},
                {"effort": "high", "description": "High"},
                {"effort": "ultra", "description": "Ultra"}
            ]
        }]
    });
    let direct = direct_source_model_catalog_with_manifest(
        &home,
        &["provider/reasoning".into()],
        Some(&manifest),
    )
    .unwrap()
    .expect("direct catalog");

    let managed = catalog::build_managed_model_catalog(&home, None, None, &direct).unwrap();
    let model = &serde_json::from_str::<Value>(&managed).unwrap()["models"][0];

    assert!(model.get("default_reasoning_level").is_none());
    assert_eq!(
        model["supported_reasoning_levels"],
        json!([
            {"effort": "low", "description": "Low"},
            {"effort": "high", "description": "High"},
            {"effort": "ultra", "description": "Ultra"}
        ])
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn repeated_attach_applies_changed_reasoning_catalog_to_an_active_profile() {
    let (root, home, backups) = profile_dirs("repeated-attach-reasoning-policy");
    let secrets = MemorySecrets::default();
    let first_catalog = r#"{"models":[{"slug":"vendor/claude-opus-4-8","default_reasoning_level":"high","supported_reasoning_levels":[{"effort":"low","description":"Low"},{"effort":"high","description":"High"}]}]}"#;
    let updated_catalog = r#"{"models":[{"slug":"vendor/claude-opus-4-8","default_reasoning_level":"low","supported_reasoning_levels":[{"effort":"low","description":"Low"}]}]}"#;

    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        first_catalog,
        &secrets,
    )
    .unwrap();
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        updated_catalog,
        &secrets,
    )
    .unwrap();

    let catalog_path = managed_model_catalog_path(&backups).unwrap();
    let catalog: Value = serde_json::from_str(&fs::read_to_string(catalog_path).unwrap()).unwrap();
    assert_eq!(catalog["models"][0]["default_reasoning_level"], "low");
    assert_eq!(
        catalog["models"][0]["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(config.contains("model_provider = \"zenith_relay_local\""));
    fs::remove_dir_all(root).unwrap();
}
