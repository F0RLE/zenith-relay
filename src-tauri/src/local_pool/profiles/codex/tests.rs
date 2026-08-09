use super::*;
use std::{collections::HashMap, path::PathBuf, sync::Mutex};

#[derive(Default)]
struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretBackend for MemorySecrets {
    fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(secret_ref.into(), value.into());
        Ok(())
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(secret_ref).cloned())
    }

    fn delete(&self, secret_ref: &str) -> Result<()> {
        self.0.lock().unwrap().remove(secret_ref);
        Ok(())
    }
}

#[derive(Default)]
struct FailingDeleteSecrets(MemorySecrets);

impl SecretBackend for FailingDeleteSecrets {
    fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
        self.0.save(secret_ref, value)
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        self.0.load(secret_ref)
    }

    fn delete(&self, _secret_ref: &str) -> Result<()> {
        Err(LocalPoolError::new(
            ErrorCode::SecretStoreUnavailable,
            "injected delete failure",
        ))
    }
}

struct MutatingSecrets {
    values: Mutex<HashMap<String, String>>,
    path: PathBuf,
    content: Vec<u8>,
}

impl MutatingSecrets {
    fn new(path: PathBuf, content: impl Into<Vec<u8>>) -> Self {
        Self {
            values: Mutex::new(HashMap::new()),
            path,
            content: content.into(),
        }
    }
}

impl SecretBackend for MutatingSecrets {
    fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
        self.values
            .lock()
            .unwrap()
            .insert(secret_ref.into(), value.into());
        fs::write(&self.path, &self.content).map_err(io_error)
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        Ok(self.values.lock().unwrap().get(secret_ref).cloned())
    }

    fn delete(&self, secret_ref: &str) -> Result<()> {
        self.values.lock().unwrap().remove(secret_ref);
        Ok(())
    }
}

struct MutatingLoadSecrets {
    values: Mutex<HashMap<String, String>>,
    path: PathBuf,
    content: Vec<u8>,
}

impl MutatingLoadSecrets {
    fn new(path: PathBuf, content: impl Into<Vec<u8>>) -> Self {
        Self {
            values: Mutex::new(HashMap::new()),
            path,
            content: content.into(),
        }
    }
}

impl SecretBackend for MutatingLoadSecrets {
    fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
        self.values
            .lock()
            .unwrap()
            .insert(secret_ref.into(), value.into());
        Ok(())
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        fs::write(&self.path, &self.content).map_err(io_error)?;
        Ok(self.values.lock().unwrap().get(secret_ref).cloned())
    }

    fn delete(&self, secret_ref: &str) -> Result<()> {
        self.values.lock().unwrap().remove(secret_ref);
        Ok(())
    }
}

#[test]
fn missing_backup_directory_has_no_local_binding() {
    let (root, home, backups) = profile_dirs("missing-backup-root");
    assert!(local_backup(&home, &backups).unwrap().is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn attach_and_restore_preserve_previous_profile_and_nested_provider() {
    let (root, home, backups) = profile_dirs("restore");
    fs::write(
        home.join(CONFIG_FILE),
        "model_provider = \"openai\"\n\n[profiles.default]\nmodel_provider = \"custom\"\n",
    )
    .unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"secret\"}}",
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
    restore_with(&home, &backups, &secrets).unwrap();

    let config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(config.contains("model_provider = \"openai\""));
    assert!(config.contains("model_provider = \"custom\""));
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("chatgpt"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn local_gateway_uses_catalog_reasoning_default_and_restores_global_override() {
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
    assert!(!managed_config.contains("model_reasoning_effort"));
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
        Some(catalog_path.to_string_lossy().as_ref())
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
    assert_eq!(models[0]["service_tiers"][0]["id"], "priority");
    assert_eq!(models[0]["additional_speed_tiers"], json!(["fast"]));
    assert_eq!(models[0]["default_service_tier"], "priority");
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
    assert_eq!(models[1]["slug"], "gpt-fake");
    assert_eq!(models[2]["slug"], "vendor/claude");
    assert_eq!(
        models
            .iter()
            .map(|model| model["priority"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1_000, 1_001, 1_002]
    );
    for model in &models {
        assert!(model.get("default_reasoning_level").is_none());
        assert_eq!(model["supported_reasoning_levels"], json!([]));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_source_catalog_allows_images_for_every_routed_model() {
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
    assert_eq!(models[1]["input_modalities"], json!(["text", "image"]));
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
fn active_managed_catalog_refreshes_without_replacing_the_profile() {
    let (root, home, backups) = profile_dirs("model-catalog-refresh");
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

    assert!(
        refresh_managed_model_catalog(&home, &backups, r#"{"models":[{"slug":"new-model"}]}"#)
            .unwrap()
    );
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
        r#"{"models":[{"slug":"new-model"}]}"#
    )
    .unwrap());
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
        !refresh_managed_model_catalog(&home, &backups, next_source_catalog).unwrap(),
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
        Some(catalog_path.to_string_lossy().as_ref())
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
fn user_snapshot_excludes_managed_projection_and_restore_detaches_it() {
    let (root, home, backups) = profile_dirs("user-snapshot");
    let original_config = "model_provider = \"custom\"\n";
    let original_auth = "{\"tokens\":{\"access_token\":\"original\"}}";
    fs::write(home.join(CONFIG_FILE), original_config).unwrap();
    fs::write(home.join(AUTH_FILE), original_auth).unwrap();
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();

    let snapshot = snapshot_user_profile_with(&home, &backups, &secrets).unwrap();
    assert_eq!(snapshot.config.as_deref(), Some(original_config));
    assert_eq!(snapshot.auth.as_deref(), Some(original_auth));
    assert!(!snapshot.config.as_deref().unwrap().contains(PROVIDER_ID));

    restore_user_profile_snapshot_full_with(&home, &backups, &snapshot, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        original_config
    );
    assert_eq!(
        fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
        original_auth
    );
    assert_eq!(profile_backup_count(&backups), 0);
    assert!(secrets.load(BACKUP_SECRET_REF).unwrap().is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_snapshot_restore_preserves_unmanaged_profile_data() {
    let (root, home, backups) = profile_dirs("managed-snapshot-merge");
    fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"custom\"\nmodel_catalog_json = \"before.json\"\nopenai_base_url = \"https://old.example/v1\"\n[mcp_servers.context7]\ncommand = \"old\"\n[plugins.example]\nenabled = true\n[features]\nexperimental = false\n[model_providers.external]\nbase_url = \"https://external.example/v1\"\n",
        )
        .unwrap();
    fs::write(
            home.join(AUTH_FILE),
            "{\"OPENAI_API_KEY\":\"original-key\",\"last_refresh\":\"old\",\"tokens\":{\"access_token\":\"original\"},\"custom\":{\"keep\":true}}",
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

    let snapshot = snapshot_user_profile_with(&home, &backups, &secrets).unwrap();
    let mut current = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    current["mcp_servers"]["context7"]["command"] = value("new");
    current["plugins"]["example"]["enabled"] = value(false);
    current["features"]["experimental"] = value(true);
    current["openai_base_url"] = value("https://changed-openai.example/v1");
    current["model_providers"]["external"]["base_url"] = value("https://changed.example/v1");
    fs::write(home.join(CONFIG_FILE), current.to_string()).unwrap();
    let mut current_auth: Value =
        serde_json::from_str(&fs::read_to_string(home.join(AUTH_FILE)).unwrap()).unwrap();
    current_auth["custom"] = json!({"keep": "current"});
    fs::write(
        home.join(AUTH_FILE),
        format!("{}\n", serde_json::to_string_pretty(&current_auth).unwrap()),
    )
    .unwrap();

    restore_user_profile_snapshot_managed_with(&home, &backups, &snapshot, &secrets).unwrap();

    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(root_model_provider(&restored).as_deref(), Some("custom"));
    assert_eq!(
        root_model_catalog_json(&restored).as_deref(),
        Some("before.json")
    );
    assert_eq!(
        restored["openai_base_url"].as_str(),
        Some("https://changed-openai.example/v1")
    );
    assert_eq!(
        restored["mcp_servers"]["context7"]["command"].as_str(),
        Some("new")
    );
    assert_eq!(
        restored["plugins"]["example"]["enabled"].as_bool(),
        Some(false)
    );
    assert_eq!(restored["features"]["experimental"].as_bool(), Some(true));
    assert_eq!(
        restored["model_providers"]["external"]["base_url"].as_str(),
        Some("https://changed.example/v1")
    );
    assert!(restored
        .get("model_providers")
        .and_then(Item::as_table)
        .is_none_or(|providers| !providers.contains_key(PROVIDER_ID)));

    let restored_auth: Value =
        serde_json::from_str(&fs::read_to_string(home.join(AUTH_FILE)).unwrap()).unwrap();
    assert_eq!(restored_auth["OPENAI_API_KEY"], "original-key");
    assert_eq!(restored_auth["last_refresh"], "old");
    assert_eq!(restored_auth["tokens"]["access_token"], "original");
    assert_eq!(restored_auth["custom"]["keep"], "current");
    assert_eq!(profile_backup_count(&backups), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_snapshot_restore_accepts_a_detached_profile() {
    let (root, home, backups) = profile_dirs("managed-snapshot-detached");
    fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"custom\"\nmodel_catalog_json = \"before.json\"\n[mcp_servers.context7]\ncommand = \"original\"\n",
        )
        .unwrap();
    fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"apikey\",\"OPENAI_API_KEY\":\"original-key\",\"custom\":{\"keep\":\"original\"}}",
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
    let snapshot = snapshot_user_profile_with(&home, &backups, &secrets).unwrap();
    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(profile_backup_count(&backups), 0);

    fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"changed\"\nmodel_catalog_json = \"current.json\"\n[mcp_servers.context7]\ncommand = \"current\"\n",
        )
        .unwrap();
    fs::write(
            home.join(AUTH_FILE),
            "{\"auth_mode\":\"apikey\",\"OPENAI_API_KEY\":\"current-key\",\"custom\":{\"keep\":\"current\"}}",
        )
        .unwrap();

    restore_user_profile_snapshot_managed_with(&home, &backups, &snapshot, &secrets).unwrap();

    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(root_model_provider(&restored).as_deref(), Some("custom"));
    assert_eq!(
        root_model_catalog_json(&restored).as_deref(),
        Some("current.json")
    );
    assert_eq!(
        restored["mcp_servers"]["context7"]["command"].as_str(),
        Some("current")
    );
    let restored_auth: Value =
        serde_json::from_str(&fs::read_to_string(home.join(AUTH_FILE)).unwrap()).unwrap();
    assert_eq!(restored_auth["OPENAI_API_KEY"], "original-key");
    assert_eq!(restored_auth["custom"]["keep"], "current");
    assert_eq!(profile_backup_count(&backups), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_snapshot_restore_blocks_a_fresh_login() {
    let (root, home, backups) = profile_dirs("managed-snapshot-fresh-login");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"tokens\":{\"access_token\":\"original\"}}",
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
    let snapshot = snapshot_user_profile_with(&home, &backups, &secrets).unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"fresh\"}}",
    )
    .unwrap();
    let config_before = fs::read(home.join(CONFIG_FILE)).unwrap();
    let auth_before = fs::read(home.join(AUTH_FILE)).unwrap();

    let error = restore_user_profile_snapshot_managed_with(&home, &backups, &snapshot, &secrets)
        .unwrap_err();

    assert_eq!(error.code, ErrorCode::ProfileRestoreBlocked);
    assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), config_before);
    assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), auth_before);
    assert!(backup_path(&backups).exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restore_blocks_fresh_login_without_touching_files() {
    let (root, home, backups) = profile_dirs("fresh-login");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    fs::write(home.join(AUTH_FILE), "{\"auth_mode\":\"chatgpt\"}").unwrap();
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"fresh\"}}",
    )
    .unwrap();
    let config_before = fs::read(home.join(CONFIG_FILE)).unwrap();
    let auth_before = fs::read(home.join(AUTH_FILE)).unwrap();

    let error = restore_with(&home, &backups, &secrets).unwrap_err();
    assert!(matches!(error.code, ErrorCode::ProfileRestoreBlocked));
    assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), config_before);
    assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), auth_before);
    assert!(backup_path(&backups).exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn profile_bindings_fail_closed_when_managed_provider_has_no_backup() {
    let (root, home, backups) = profile_dirs("missing-reset-backup");
    fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"zenith_relay_local\"\n\n[model_providers.zenith_relay_local]\nname = \"Zenith Relay\"\n",
        )
        .unwrap();

    let error = profile_bindings(&home, &backups).unwrap_err();
    assert!(matches!(error.code, ErrorCode::RecoveryRequired));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restore_blocks_changed_provider_origin() {
    let (root, home, backups) = profile_dirs("changed-origin");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();
    let changed = fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .replace("14998", "14999");
    fs::write(home.join(CONFIG_FILE), changed).unwrap();
    assert!(matches!(
        restore_with(&home, &backups, &secrets).unwrap_err().code,
        ErrorCode::ProfileRestoreBlocked
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restore_blocks_changed_gateway_bearer() {
    let (root, home, backups) = profile_dirs("changed-bearer");
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();
    let changed = fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .replace("zlr_key", "zlr_other");
    fs::write(home.join(CONFIG_FILE), changed).unwrap();

    assert!(matches!(
        restore_with(&home, &backups, &secrets).unwrap_err().code,
        ErrorCode::ProfileRestoreBlocked
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_attach_upgrades_a_profile_without_managed_bearer_metadata() {
    let (root, home, backups) = profile_dirs("legacy-missing-bearer");
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();
    let config = fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .lines()
        .filter(|line| !line.trim_start().starts_with("experimental_bearer_token ="))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(home.join(CONFIG_FILE), config).unwrap();
    let backup_path = backup_path(&backups);
    let mut backup: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
    backup
        .as_object_mut()
        .unwrap()
        .remove("managedBearerInConfig");
    fs::write(&backup_path, serde_json::to_string_pretty(&backup).unwrap()).unwrap();

    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();
    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("experimental_bearer_token = \"zlr_key\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_attach_blocks_after_fresh_login() {
    let (root, home, backups) = profile_dirs("repeat-fresh-login");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"fresh\"}}",
    )
    .unwrap();

    assert!(matches!(
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_new_key",
            &secrets
        )
        .unwrap_err()
        .code,
        ErrorCode::ProfileRestoreBlocked
    ));
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("fresh"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_attach_rebases_external_takeover_and_restores_latest_profile() {
    let (root, home, backups) = profile_dirs("repeat-external-takeover");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
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

    let legacy_config = fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .replace("supports_websockets = true", "supports_websockets = false");
    let backup_path = backup_path(&backups);
    let mut legacy_backup: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
    legacy_backup
        .as_object_mut()
        .unwrap()
        .remove("managedSupportsWebsockets");
    fs::write(
        &backup_path,
        serde_json::to_string_pretty(&legacy_backup).unwrap(),
    )
    .unwrap();

    let external_config = legacy_config
            .replacen(
                "model_provider = \"zenith_relay_local\"",
                "model_provider = \"codex_local_access\"",
                1,
            )
            + "\n[model_providers.codex_local_access]\nname = \"Codex API Service\"\nbase_url = \"http://127.0.0.1:49976/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n";
    fs::write(home.join(CONFIG_FILE), external_config).unwrap();
    let external_auth = "{\"OPENAI_API_KEY\":null,\"tokens\":{\"access_token\":\"fresh\"}}";
    fs::write(home.join(AUTH_FILE), external_auth).unwrap();

    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_next_key",
        &secrets,
    )
    .unwrap();
    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .starts_with("model_provider = \"zenith_relay_local\""));
    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("supports_websockets = false"));
    let upgraded_backup: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
    assert_eq!(upgraded_backup["managedSupportsWebsockets"], false);

    restore_with(&home, &backups, &secrets).unwrap();
    let restored_config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(restored_config.starts_with("model_provider = \"codex_local_access\""));
    assert!(!restored_config.contains("[model_providers.zenith_relay_local]"));
    assert_eq!(
        fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
        external_auth
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restore_keeps_root_model_provider_absent_when_it_started_absent() {
    let (root, home, backups) = profile_dirs("no-root-provider");
    fs::write(
        home.join(CONFIG_FILE),
        "[profiles.default]\nmodel_provider = \"custom\"\n",
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
    restore_with(&home, &backups, &secrets).unwrap();

    let document = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert!(document.get("model_provider").is_none());
    assert_eq!(
        document["profiles"]["default"]["model_provider"].as_str(),
        Some("custom")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn attach_rejects_non_utf8_config_without_rewriting_it() {
    let (root, home, backups) = profile_dirs("non-utf8");
    let config_path = home.join(CONFIG_FILE);
    let original = vec![0xff, 0xfe, 0xfd];
    fs::write(&config_path, &original).unwrap();
    let secrets = MemorySecrets::default();

    assert!(attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets
    )
    .is_err());
    assert_eq!(fs::read(config_path).unwrap(), original);
    assert!(!backup_path(&backups).exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn attach_rejects_non_utf8_auth_without_rewriting_it() {
    let (root, home, backups) = profile_dirs("non-utf8-auth");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    let auth_path = home.join(AUTH_FILE);
    let original = vec![0xff, 0xfe, 0xfd];
    fs::write(&auth_path, &original).unwrap();

    assert!(attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &MemorySecrets::default()
    )
    .is_err());
    assert_eq!(fs::read(auth_path).unwrap(), original);
    assert!(!backup_path(&backups).exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn external_login_during_attach_is_not_overwritten() {
    let (root, home, backups) = profile_dirs("external-login");
    let config_path = home.join(CONFIG_FILE);
    let auth_path = home.join(AUTH_FILE);
    let original_config = "model_provider = \"openai\"\n";
    let fresh_auth = b"{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"fresh\"}}";
    fs::write(&config_path, original_config).unwrap();
    fs::write(&auth_path, "{\"auth_mode\":\"chatgpt\"}").unwrap();
    let secrets = MutatingSecrets::new(auth_path.clone(), fresh_auth.to_vec());

    let error = attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap_err();

    assert!(matches!(error.code, ErrorCode::ProfileRestoreBlocked));
    assert_eq!(fs::read_to_string(config_path).unwrap(), original_config);
    assert_eq!(fs::read(auth_path).unwrap(), fresh_auth);
    assert!(!backup_path(&backups).exists());
    assert!(secrets.load(BACKUP_SECRET_REF).unwrap().is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn external_config_change_during_attach_is_not_overwritten() {
    let (root, home, backups) = profile_dirs("external-config");
    let config_path = home.join(CONFIG_FILE);
    let changed_config = b"model_provider = \"custom\"\n";
    fs::write(&config_path, "model_provider = \"openai\"\n").unwrap();
    fs::write(home.join(AUTH_FILE), "{\"auth_mode\":\"chatgpt\"}").unwrap();
    let secrets = MutatingSecrets::new(config_path.clone(), changed_config.to_vec());

    let error = attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap_err();

    assert!(matches!(error.code, ErrorCode::ProfileRestoreBlocked));
    assert_eq!(fs::read(config_path).unwrap(), changed_config);
    assert!(!backup_path(&backups).exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_reattach_restores_previous_backup_metadata() {
    let (root, home, backups) = profile_dirs("backup-rollback");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_old_key",
        &secrets,
    )
    .unwrap();
    let managed_config = fs::read(home.join(CONFIG_FILE)).unwrap();
    let managed_auth = fs::read(home.join(AUTH_FILE)).unwrap();
    fs::create_dir(home.join("config.tmp")).unwrap();

    assert!(attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14999/v1",
        "zlr_new_key",
        &secrets,
    )
    .is_err());
    assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), managed_config);
    assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), managed_auth);
    let pending: Value =
        serde_json::from_str(&fs::read_to_string(backup_path(&backups)).unwrap()).unwrap();
    assert_eq!(pending["restorePending"], true);
    fs::remove_dir_all(home.join("config.tmp")).unwrap();
    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        "model_provider = \"openai\"\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_backup_secret_cleanup_rolls_restore_back() {
    let (root, home, backups) = profile_dirs("restore-cleanup-rollback");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"old\"}}",
    )
    .unwrap();
    let secrets = FailingDeleteSecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();
    assert!(restore_with(&home, &backups, &secrets).is_err());
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        "model_provider = \"openai\"\n"
    );
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("old"));
    let pending: Value =
        serde_json::from_str(&fs::read_to_string(backup_path(&backups)).unwrap()).unwrap();
    assert_eq!(pending["restorePending"], true);
    assert!(!managed_model_catalog_path(&backups).unwrap().exists());
    assert!(secrets.load(BACKUP_SECRET_REF).unwrap().is_some());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_backup_during_restore_rolls_profile_back() {
    let (root, home, backups) = profile_dirs("restore-backup-race");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"old\"}}",
    )
    .unwrap();
    let external_backup = b"external backup change".to_vec();
    let secrets = MutatingLoadSecrets::new(backup_path(&backups), external_backup.clone());
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();
    let managed_config = fs::read(home.join(CONFIG_FILE)).unwrap();
    let managed_auth = fs::read(home.join(AUTH_FILE)).unwrap();

    let error = restore_with(&home, &backups, &secrets).unwrap_err();

    assert!(matches!(error.code, ErrorCode::ProfileRestoreBlocked));
    assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), managed_config);
    assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), managed_auth);
    assert_eq!(fs::read(backup_path(&backups)).unwrap(), external_backup);
    assert!(secrets.load(BACKUP_SECRET_REF).unwrap().is_some());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn oauth_account_attach_reuses_one_profile_binding_and_restores_previous_login() {
    let (root, home, backups) = profile_dirs("oauth-account");
    let previous_config = r#"model_provider = "custom"
openai_base_url = "https://stale.example.com/v1"

[model_providers.custom]
name = "Custom"
base_url = "https://custom.example.com/v1"
"#;
    fs::write(home.join(CONFIG_FILE), previous_config).unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"previous\"}}",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let first = TokenSet::new(
        "access-secret",
        Some("refresh-secret".into()),
        Some("id-secret".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();
    let binding = attach_account_with(
        &home,
        &backups,
        "account-local",
        &first,
        "provider-private-id",
        &secrets,
    )
    .unwrap();
    assert_eq!(binding.credential_id, "account-local");
    let stored_bindings = account_bindings(&backups).unwrap();
    assert_eq!(stored_bindings.len(), 1);
    assert_eq!(stored_bindings[0].credential_id, binding.credential_id);
    assert!(profile_bindings(&home, &backups).unwrap()[0].active);
    let account_config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(!account_config.contains("model_provider ="));
    assert!(!account_config.contains("openai_base_url"));
    assert!(!account_config.contains("[model_providers.zenith_relay_local]"));
    assert!(account_config.contains("[model_providers.custom]"));
    let account_auth: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(AUTH_FILE)).unwrap()).unwrap();
    assert_eq!(account_auth["OPENAI_API_KEY"], serde_json::Value::Null);
    assert_eq!(account_auth["tokens"]["refresh_token"], "refresh-secret");
    assert!(account_auth.get("auth_mode").is_none());

    let canonical_home = canonical_profile_dir(&home).unwrap();
    let backup_path = account_backup_path(&backups, &canonical_home);
    let backup = fs::read_to_string(&backup_path).unwrap();
    for secret in [
        "access-secret",
        "refresh-secret",
        "id-secret",
        "provider-private-id",
    ] {
        assert!(!backup.contains(secret));
    }

    attach_account_with(
        &home,
        &backups,
        "account-local",
        &first,
        "provider-private-id",
        &secrets,
    )
    .unwrap();
    assert_eq!(account_bindings(&backups).unwrap().len(), 1);

    let refreshed = TokenSet::new(
        "access-refreshed",
        Some("refresh-new".into()),
        Some("id-new".into()),
        Some(120_000),
        2,
        2,
    )
    .unwrap();
    assert_eq!(
        sync_account_bindings(&backups, "account-local", &refreshed, "provider-private-id",)
            .unwrap(),
        1
    );
    assert_eq!(
        sync_account_bindings(&backups, "account-local", &refreshed, "provider-private-id",)
            .unwrap(),
        0
    );
    assert_eq!(account_bindings(&backups).unwrap().len(), 1);
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("access-refreshed"));

    let restored = restore_account_with(&home, &backups, &secrets)
        .unwrap()
        .unwrap();
    assert_eq!(restored, binding);
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        previous_config
    );
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("previous"));
    assert!(account_bindings(&backups).unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_profile_rotation_is_adopted_only_for_the_same_account() {
    let (root, home, backups) = profile_dirs("managed-token-adoption");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    let secrets = MemorySecrets::default();
    let original = TokenSet::new(
        "access-original",
        Some("refresh-original".into()),
        Some("id-original".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();
    attach_account_with(
        &home,
        &backups,
        "account-local",
        &original,
        "provider-account",
        &secrets,
    )
    .unwrap();

    let rotated = TokenSet::new(
        "access-rotated",
        Some("refresh-rotated".into()),
        Some("id-rotated".into()),
        Some(120_000),
        2,
        2,
    )
    .unwrap();
    fs::write(
        home.join(AUTH_FILE),
        account_auth_content(&rotated, "provider-account").unwrap(),
    )
    .unwrap();
    let update = managed_account_token_update(
        &home,
        &backups,
        "account-local",
        original.access_token(),
        "provider-account",
    )
    .unwrap()
    .unwrap();
    assert_eq!(update.access_token, "access-rotated");
    assert_eq!(update.refresh_token, "refresh-rotated");
    assert_eq!(update.id_token.as_deref(), Some("id-rotated"));
    let debug = format!("{update:?}");
    assert!(!debug.contains("rotated"));

    assert_eq!(
        sync_account_bindings(&backups, "account-local", &rotated, "provider-account").unwrap(),
        1
    );
    assert!(managed_account_token_update(
        &home,
        &backups,
        "account-local",
        rotated.access_token(),
        "provider-account",
    )
    .unwrap()
    .is_none());

    let other = TokenSet::new(
        "other-access",
        Some("other-refresh".into()),
        Some("other-id".into()),
        Some(180_000),
        3,
        3,
    )
    .unwrap();
    fs::write(
        home.join(AUTH_FILE),
        account_auth_content(&other, "provider-other").unwrap(),
    )
    .unwrap();
    assert!(managed_account_token_update(
        &home,
        &backups,
        "account-local",
        rotated.access_token(),
        "provider-account",
    )
    .unwrap()
    .is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn switching_account_and_local_gateway_preserves_the_original_profile() {
    let (root, home, backups) = profile_dirs("credential-kind-switch");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new(
        "managed-account",
        Some("refresh".into()),
        Some("id-token".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();

    let local = switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions::default(),
        &secrets,
    )
    .unwrap();
    assert_eq!(local.credential_kind, ProfileCredentialKind::LocalGateway);
    assert_eq!(profile_bindings(&home, &backups).unwrap(), vec![local]);
    assert_eq!(profile_backup_count(&backups), 1);

    let account = switch_to_account_with(
        &home,
        &backups,
        "account-local",
        &tokens,
        "provider-account",
        &secrets,
    )
    .unwrap();
    assert_eq!(account.credential_kind, ProfileCredentialKind::OAuthAccount);
    assert_eq!(profile_bindings(&home, &backups).unwrap(), vec![account]);
    assert!(!backup_path(&backups).exists());
    assert_eq!(profile_backup_count(&backups), 1);

    switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions::default(),
        &secrets,
    )
    .unwrap();
    assert_eq!(profile_backup_count(&backups), 1);
    restore_with(&home, &backups, &secrets).unwrap();

    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("model_provider = \"custom\""));
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("original"));
    assert_eq!(profile_backup_count(&backups), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn profile_binding_detects_an_external_provider_takeover() {
    let (root, home, backups) = profile_dirs("external-provider-active-state");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    fs::write(home.join(AUTH_FILE), "{\"auth_mode\":\"apikey\"}").unwrap();
    let secrets = MemorySecrets::default();
    switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions::default(),
        &secrets,
    )
    .unwrap();
    assert!(profile_bindings(&home, &backups).unwrap()[0].active);

    let managed_auth = fs::read(home.join(AUTH_FILE)).unwrap();
    fs::write(home.join(AUTH_FILE), r#"{"auth_mode":"apikey"}"#).unwrap();
    assert!(!profile_bindings(&home, &backups).unwrap()[0].active);
    fs::write(home.join(AUTH_FILE), managed_auth).unwrap();

    fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"codex_local_access\"\n\n[model_providers.codex_local_access]\nbase_url = \"https://api.example.test/v1\"\n",
        )
        .unwrap();
    let bindings = profile_bindings(&home, &backups).unwrap();
    assert_eq!(bindings.len(), 1);
    assert!(!bindings[0].active);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn switching_external_account_takeover_to_local_rebases_the_latest_profile() {
    let (root, home, backups) = profile_dirs("external-account-takeover");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"openai\"\n").unwrap();
    fs::write(home.join(AUTH_FILE), "{\"auth_mode\":\"chatgpt\"}").unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new(
        "managed-access",
        Some("managed-refresh".into()),
        Some("managed-id".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();
    attach_account_with(
        &home,
        &backups,
        "account-local",
        &tokens,
        "provider-account",
        &secrets,
    )
    .unwrap();

    let external_config = "model_provider = \"codex_local_access\"\n\n[model_providers.codex_local_access]\nname = \"Codex API Service\"\nbase_url = \"http://127.0.0.1:49976/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n";
    let external_auth = "{\"tokens\":{\"access_token\":\"external\"}}";
    fs::write(home.join(CONFIG_FILE), external_config).unwrap();
    fs::write(home.join(AUTH_FILE), external_auth).unwrap();

    switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions {
            bound_oauth: Some(BoundOAuthProfile {
                account_id: "account-local",
                tokens: &tokens,
                provider_account_id: "provider-account",
            }),
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();
    assert_eq!(profile_backup_count(&backups), 1);
    assert!(backup_path(&backups).exists());

    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        external_config
    );
    assert_eq!(
        fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
        external_auth
    );
    assert_eq!(profile_backup_count(&backups), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn local_gateway_projects_and_syncs_a_bound_oauth_profile() {
    let (root, home, backups) = profile_dirs("local-gateway-bound-oauth");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new(
        "bound-access",
        Some("bound-refresh".into()),
        Some("bound-id".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();

    let binding = switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions {
            bound_oauth: Some(BoundOAuthProfile {
                account_id: "account-local",
                tokens: &tokens,
                provider_account_id: "provider-account",
            }),
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();
    assert_eq!(
        binding.bound_oauth_account_id.as_deref(),
        Some("account-local")
    );
    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("model_provider = \"zenith_relay_local\""));
    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("experimental_bearer_token = \"zlr_key\""));
    let projected = fs::read_to_string(home.join(AUTH_FILE)).unwrap();
    assert!(projected.contains("bound-access"));
    assert!(!projected.contains("zlr_key"));
    let projected_value = serde_json::from_str::<serde_json::Value>(&projected).unwrap();
    assert!(projected_value["OPENAI_API_KEY"].is_null());
    assert!(projected_value.get("auth_mode").is_none());
    assert_eq!(projected_value["tokens"]["account_id"], "provider-account");
    DateTime::parse_from_rfc3339(projected_value["last_refresh"].as_str().unwrap()).unwrap();

    let refreshed = TokenSet::new(
        "bound-access-refreshed",
        Some("bound-refresh-next".into()),
        Some("bound-id-next".into()),
        Some(120_000),
        2,
        2,
    )
    .unwrap();
    assert!(sync_local_gateway_binding(
        &home,
        &backups,
        "account-local",
        &refreshed,
        "provider-account",
    )
    .unwrap());
    assert!(!sync_local_gateway_binding(
        &home,
        &backups,
        "account-local",
        &refreshed,
        "provider-account",
    )
    .unwrap());
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("bound-access-refreshed"));

    restore_with(&home, &backups, &secrets).unwrap();
    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("model_provider = \"custom\""));
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("original"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn local_gateway_can_replace_bound_oauth_with_local_key() {
    let (root, home, backups) = profile_dirs("local-gateway-remove-oauth-binding");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new(
        "bound-access",
        Some("bound-refresh".into()),
        Some("bound-id".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();

    switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions {
            bound_oauth: Some(BoundOAuthProfile {
                account_id: "account-local",
                tokens: &tokens,
                provider_account_id: "provider-account",
            }),
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();
    let binding = switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions::default(),
        &secrets,
    )
    .unwrap();

    assert_eq!(binding.bound_oauth_account_id, None);
    let projected = fs::read_to_string(home.join(AUTH_FILE)).unwrap();
    assert!(projected.contains("zlr_key"));
    assert!(!projected.contains("bound-access"));
    restore_with(&home, &backups, &secrets).unwrap();
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("original"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn local_gateway_keeps_api_key_projection_when_bound_oauth_has_no_id_token() {
    let (root, home, backups) = profile_dirs("local-gateway-bound-access-only");
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new(
        "bound-access",
        Some("bound-refresh".into()),
        None,
        Some(60_000),
        1,
        1,
    )
    .unwrap();

    let binding = switch_to_local_with(
        &home,
        &backups,
        "key-local",
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        LocalAttachOptions {
            bound_oauth: Some(BoundOAuthProfile {
                account_id: "account-local",
                tokens: &tokens,
                provider_account_id: "provider-account",
            }),
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();
    assert_eq!(
        binding.bound_oauth_account_id.as_deref(),
        Some("account-local")
    );
    let projected = fs::read_to_string(home.join(AUTH_FILE)).unwrap();
    assert!(projected.contains("zlr_key"));
    assert!(!projected.contains("bound-access"));
    restore_with(&home, &backups, &secrets).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn oauth_account_restore_refuses_a_fresh_manual_login() {
    let (root, home, backups) = profile_dirs("oauth-fresh-login");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new("managed", None, None, Some(60_000), 1, 1).unwrap();
    attach_account_with(
        &home,
        &backups,
        "account-local",
        &tokens,
        "provider-private-id",
        &secrets,
    )
    .unwrap();
    let auth: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(AUTH_FILE)).unwrap()).unwrap();
    assert_eq!(auth["tokens"]["refresh_token"], "");
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"fresh\"}}",
    )
    .unwrap();

    assert!(matches!(
        restore_account_with(&home, &backups, &secrets)
            .unwrap_err()
            .code,
        ErrorCode::ProfileRestoreBlocked
    ));
    assert!(fs::read_to_string(home.join(AUTH_FILE))
        .unwrap()
        .contains("fresh"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn oauth_account_bindings_are_isolated_per_profile_path() {
    let (root, first, backups) = profile_dirs("oauth-multi-profile");
    let second = root.join("second-profile");
    fs::create_dir_all(&second).unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new("managed", None, None, Some(60_000), 1, 1).unwrap();
    attach_account_with(
        &first,
        &backups,
        "account-local",
        &tokens,
        "provider-private-id",
        &secrets,
    )
    .unwrap();
    attach_account_with(
        &second,
        &backups,
        "account-local",
        &tokens,
        "provider-private-id",
        &secrets,
    )
    .unwrap();
    assert_eq!(account_bindings(&backups).unwrap().len(), 2);

    restore_account_with(&first, &backups, &secrets).unwrap();
    let remaining = account_bindings(&backups).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].profile_dir,
        canonical_profile_dir(&second).unwrap().to_string_lossy()
    );
    fs::remove_dir_all(root).unwrap();
}

fn profile_dirs(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-profile-{name}-{}",
        uuid::Uuid::new_v4()
    ));
    let home = root.join("profile");
    let backups = root.join("backups");
    fs::create_dir_all(&home).unwrap();
    (root, home, backups)
}

fn profile_backup_count(backups: &Path) -> usize {
    fs::read_dir(backups)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("json"))
        .count()
}

fn write_test_catalog_file(path: &Path, slug: &str) {
    let mut entry = routed_codex_catalog_entry(None, slug, 2, None);
    entry["slug"] = Value::String(slug.into());
    entry["display_name"] = Value::String(slug.into());
    entry["description"] = Value::String("Native user model".into());
    entry["comp_hash"] = Value::String("official".into());
    entry["default_reasoning_level"] = Value::String("medium".into());
    entry["supported_reasoning_levels"] = json!([
        {"effort": "medium", "description": "Medium"}
    ]);
    fs::write(
        path,
        serde_json::to_string_pretty(&json!({"models": [entry]})).unwrap(),
    )
    .unwrap();
}
