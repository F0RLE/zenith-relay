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

#[derive(Default)]
struct SwitchFaultSecrets {
    memory: MemorySecrets,
    fail_projection_save: Mutex<bool>,
    fail_delete_at: Mutex<Option<usize>>,
    external_config: Mutex<Option<PathBuf>>,
}

impl SecretBackend for SwitchFaultSecrets {
    fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
        if secret_ref.starts_with("profile:codex:projection:")
            && std::mem::take(&mut *self.fail_projection_save.lock().unwrap())
        {
            if let Some(path) = self.external_config.lock().unwrap().take() {
                fs::write(path, "model_provider = 'external'\n").map_err(io_error)?;
            }
            return Err(LocalPoolError::new(
                ErrorCode::SecretStoreUnavailable,
                "injected save failure",
            ));
        }
        self.memory.save(secret_ref, value)
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        self.memory.load(secret_ref)
    }

    fn delete(&self, secret_ref: &str) -> Result<()> {
        let mut countdown = self.fail_delete_at.lock().unwrap();
        if let Some(remaining) = countdown.as_mut() {
            *remaining -= 1;
            if *remaining == 0 {
                *countdown = None;
                return Err(LocalPoolError::new(
                    ErrorCode::SecretStoreUnavailable,
                    "injected delete failure",
                ));
            }
        }
        self.memory.delete(secret_ref)
    }
}

#[test]
fn failed_switch_restores_files_credentials_and_backup_in_both_directions() {
    for from_account in [false, true] {
        let (root, home, backups) = profile_dirs("switch-failure-rollback");
        fs::write(home.join(CONFIG_FILE), "model_provider = 'original'\n").unwrap();
        fs::write(
            home.join(AUTH_FILE),
            r#"{"OPENAI_API_KEY":"original-fixture"}"#,
        )
        .unwrap();
        let secrets = SwitchFaultSecrets::default();
        let tokens = TokenSet::new(
            "fixture-access",
            Some("fixture-refresh".into()),
            None,
            None,
            1,
            1,
        )
        .unwrap();
        if from_account {
            attach_account_with(
                &home,
                &backups,
                "account",
                &tokens,
                "provider-account",
                &secrets,
            )
            .unwrap();
        } else {
            attach_with(
                &home,
                &backups,
                "http://127.0.0.1:14998/v1",
                "fixture-key",
                &secrets,
            )
            .unwrap();
        }
        let config = fs::read(home.join(CONFIG_FILE)).unwrap();
        let auth = fs::read(home.join(AUTH_FILE)).unwrap();
        let path = if from_account {
            account_backup_for_profile(&home, &backups)
                .unwrap()
                .unwrap()
        } else {
            backup_path(&backups)
        };
        let backup = fs::read(&path).unwrap();
        let stored = secrets.memory.0.lock().unwrap().clone();
        *secrets.fail_projection_save.lock().unwrap() = true;
        let error = if from_account {
            switch_to_local_with(
                &home,
                &backups,
                "key",
                "http://127.0.0.1:14998/v1",
                "fixture-next",
                LocalAttachOptions::default(),
                &secrets,
            )
            .unwrap_err()
        } else {
            switch_to_account_with(&home, &backups, "next", &tokens, "provider-next", &secrets)
                .unwrap_err()
        };
        assert_eq!(error.code, ErrorCode::SecretStoreUnavailable);
        assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), config);
        assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), auth);
        assert_eq!(fs::read(&path).unwrap(), backup);
        assert_eq!(*secrets.memory.0.lock().unwrap(), stored);
        assert_eq!(profile_backup_count(&backups), 1);
        if from_account {
            restore_account_with(&home, &backups, &secrets).unwrap();
        } else {
            restore_with(&home, &backups, &secrets).unwrap();
        }
        assert!(secrets.memory.0.lock().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn failed_switch_does_not_overwrite_a_new_external_profile() {
    let (root, home, backups) = profile_dirs("switch-external-writer");
    let secrets = SwitchFaultSecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "fixture-key",
        &secrets,
    )
    .unwrap();
    let tokens = TokenSet::new("fixture-access", None, None, None, 1, 1).unwrap();
    *secrets.fail_projection_save.lock().unwrap() = true;
    *secrets.external_config.lock().unwrap() = Some(home.join(CONFIG_FILE));
    let error = switch_to_account_with(&home, &backups, "next", &tokens, "provider-next", &secrets)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        "model_provider = 'external'\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn partial_secret_cleanup_keeps_restore_retryable() {
    for account in [false, true] {
        let (root, home, backups) = profile_dirs("partial-secret-cleanup");
        let secrets = SwitchFaultSecrets::default();
        fs::write(
            home.join(AUTH_FILE),
            r#"{"OPENAI_API_KEY":"original-fixture"}"#,
        )
        .unwrap();
        if account {
            let tokens = TokenSet::new("fixture-access", None, None, None, 1, 1).unwrap();
            attach_account_with(
                &home,
                &backups,
                "account",
                &tokens,
                "provider-account",
                &secrets,
            )
            .unwrap();
        } else {
            attach_with(
                &home,
                &backups,
                "http://127.0.0.1:14998/v1",
                "fixture-key",
                &secrets,
            )
            .unwrap();
        }
        let stored = secrets.memory.0.lock().unwrap().clone();
        *secrets.fail_delete_at.lock().unwrap() = Some(2);
        let restore = || {
            if account {
                restore_account_with(&home, &backups, &secrets).map(|_| ())
            } else {
                restore_with(&home, &backups, &secrets)
            }
        };
        assert!(restore().is_err());
        assert_eq!(*secrets.memory.0.lock().unwrap(), stored);
        assert_eq!(profile_backup_count(&backups), 1);
        restore().unwrap();
        assert!(secrets.memory.0.lock().unwrap().is_empty());
        assert_eq!(profile_backup_count(&backups), 0);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn websocket_change_preserves_user_settings_on_restore_and_failed_save() {
    let (root, home, backups) = profile_dirs("websocket-undo-projection");
    let secrets = SwitchFaultSecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "fixture-key",
        &secrets,
    )
    .unwrap();
    let path = home.join(CONFIG_FILE);
    let current = format!(
        "user_setting = 'keep'\n{}",
        fs::read_to_string(&path).unwrap()
    );
    fs::write(&path, &current).unwrap();
    *secrets.fail_projection_save.lock().unwrap() = true;
    assert!(set_local_gateway_websockets_with_backend(&home, &backups, false, &secrets).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), current);
    set_local_gateway_websockets_with_backend(&home, &backups, false, &secrets).unwrap();
    restore_with(&home, &backups, &secrets).unwrap();
    let restored = parse_config(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(restored["user_setting"].as_str(), Some("keep"));
    assert!(restored.get("model_providers").is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn websocket_change_does_not_create_an_absent_codex_home() {
    let (root, home, backups) = profile_dirs("websocket-missing-home");
    fs::remove_dir_all(&home).unwrap();

    let previous = set_local_gateway_websockets_with_backend(
        &home,
        &backups,
        false,
        &MemorySecrets::default(),
    )
    .unwrap();

    assert_eq!(previous, None);
    assert!(!home.exists());
    assert!(!backups.exists());
    fs::remove_dir_all(root).unwrap();
}

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
fn local_gateway_websocket_setting_updates_managed_config_and_backup() {
    let (root, home, backups) = profile_dirs("websocket-toggle");
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

    set_local_gateway_websockets_with_backend(&home, &backups, false, &secrets).unwrap();
    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("supports_websockets = false"));
    let backup_file = backup_path(&backups);
    let backup: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&backup_file).unwrap()).unwrap();
    assert_eq!(backup["managedSupportsWebsockets"], false);

    set_local_gateway_websockets_with_backend(&home, &backups, true, &secrets).unwrap();
    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("supports_websockets = true"));
    let backup: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&backup_file).unwrap()).unwrap();
    assert_eq!(backup["managedSupportsWebsockets"], true);

    restore_with(&home, &backups, &secrets).unwrap();
    let restored = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(restored.contains("model_provider = \"openai\""));
    assert!(!restored.contains("[model_providers.zenith_relay_local]"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ready_api_websocket_setting_and_restore_use_its_managed_provider_id() {
    let (root, home, backups) = profile_dirs("ready-api-websocket-toggle");
    fs::write(
        home.join(CONFIG_FILE),
        "model_provider = \"custom\"\n\n[model_providers.custom]\nname = \"Custom\"\n",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    switch_to_local_with(
        &home,
        &backups,
        "ready-api",
        "https://api.zenithmarket.dev/v1",
        "fixture-api-key",
        LocalAttachOptions {
            provider_id: READY_API_PROVIDER_ID,
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();

    set_local_gateway_websockets_with_backend(&home, &backups, false, &secrets).unwrap();
    let managed = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(
        managed["model_providers"][READY_API_PROVIDER_ID]["name"].as_str(),
        Some("OpenAI")
    );
    assert_eq!(
        managed["model_providers"][READY_API_PROVIDER_ID]["supports_websockets"].as_bool(),
        Some(false)
    );

    restore_with(&home, &backups, &secrets).unwrap();
    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(restored["model_provider"].as_str(), Some("custom"));
    assert!(restored["model_providers"]
        .get(READY_API_PROVIDER_ID)
        .is_none());
    assert_eq!(
        restored["model_providers"]["custom"]["name"].as_str(),
        Some("Custom")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ready_api_legacy_provider_name_is_migrated_to_openai() {
    let (root, home, backups) = profile_dirs("ready-api-provider-name-migration");
    fs::write(
        home.join(CONFIG_FILE),
        "model_provider = \"custom\"\n\n[model_providers.custom]\nname = \"Custom\"\n",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let options = LocalAttachOptions {
        provider_id: READY_API_PROVIDER_ID,
        ..LocalAttachOptions::default()
    };
    switch_to_local_with(
        &home,
        &backups,
        "ready-api",
        "https://api.zenithmarket.dev/v1",
        "fixture-api-key",
        options,
        &secrets,
    )
    .unwrap();

    let legacy = fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .replace("name = \"OpenAI\"", "name = \"Zenith\"");
    fs::write(home.join(CONFIG_FILE), legacy).unwrap();

    switch_to_local_with(
        &home,
        &backups,
        "ready-api",
        "https://api.zenithmarket.dev/v1",
        "fixture-api-key",
        LocalAttachOptions {
            provider_id: READY_API_PROVIDER_ID,
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();

    let migrated = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(
        migrated["model_providers"][READY_API_PROVIDER_ID]["name"].as_str(),
        Some("OpenAI")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_backup_without_websocket_field_does_not_block_restore() {
    let (root, home, backups) = profile_dirs("legacy-websocket-backup");
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

    let config_path = home.join(CONFIG_FILE);
    let config = fs::read_to_string(&config_path).unwrap();
    fs::write(
        &config_path,
        config.replace("supports_websockets = true", "supports_websockets = false"),
    )
    .unwrap();
    let backup_file = backup_path(&backups);
    let mut backup: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&backup_file).unwrap()).unwrap();
    backup
        .as_object_mut()
        .unwrap()
        .remove("managedSupportsWebsockets");
    // This fixture represents a pre-projection backup, not a new-format
    // undo record whose expected provider settings were changed externally.
    backup
        .as_object_mut()
        .unwrap()
        .remove("projectionSecretRef");
    fs::write(&backup_file, serde_json::to_string_pretty(&backup).unwrap()).unwrap();

    restore_with(&home, &backups, &secrets).unwrap();
    let restored = fs::read_to_string(config_path).unwrap();
    assert!(restored.contains("model_provider = \"openai\""));
    assert!(!restored.contains("[model_providers.zenith_relay_local]"));
    fs::remove_dir_all(root).unwrap();
}

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
fn profile_bindings_reports_orphaned_managed_provider_without_blocking_inventory() {
    let (root, home, backups) = profile_dirs("missing-reset-backup");
    fs::write(
            home.join(CONFIG_FILE),
            "model_provider = \"zenith_relay_local\"\n\n[model_providers.zenith_relay_local]\nname = \"Zenith Relay\"\n",
        )
        .unwrap();

    let bindings = profile_bindings(&home, &backups).unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(
        bindings[0].credential_kind,
        ProfileCredentialKind::LocalGateway
    );
    assert!(!bindings[0].active);
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
        .replace("supports_websockets = false", "supports_websockets = true");
    let backup_path = backup_path(&backups);
    let mut legacy_backup: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
    legacy_backup["managedSupportsWebsockets"] = serde_json::Value::Bool(true);
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
        .contains("supports_websockets = true"));
    let upgraded_backup: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
    assert_eq!(upgraded_backup["managedSupportsWebsockets"], true);

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
fn profile_file_replace_refuses_an_outdated_snapshot() {
    let (root, home, _backups) = profile_dirs("outdated-file-snapshot");
    let path = home.join(CONFIG_FILE);
    let expected = Some(b"original".to_vec());
    let current = b"external".to_vec();
    fs::write(&path, &current).unwrap();

    let error = replace_if_unchanged(&path, &expected, "managed").unwrap_err();

    assert_eq!(error.code, ErrorCode::ProfileRestoreBlocked);
    assert_eq!(fs::read(&path).unwrap(), current);
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
fn oauth_account_attach_uses_native_catalog_instead_of_foreign_managed_catalog() {
    let (root, home, backups) = profile_dirs("oauth-account-native-catalog");
    fs::write(
        home.join(CONFIG_FILE),
        format!(
            "model_provider = \"{PROVIDER_ID}\"\nmodel_catalog_json = \"foreign-catalog.json\"\n\n[model_providers.{PROVIDER_ID}]\nname = \"Relay\"\n"
        ),
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new("access", Some("refresh".into()), None, None, 1, 1).unwrap();

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
    assert!(root_model_catalog_json(&attached).is_none());
    assert!(root_model_provider(&attached).is_none());
    assert!(!document_has_provider(&attached));
    restore_account_with(&home, &backups, &secrets).unwrap();
    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(root_model_provider(&restored).as_deref(), Some(PROVIDER_ID));
    assert_eq!(
        root_model_catalog_json(&restored).as_deref(),
        Some("foreign-catalog.json")
    );
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
        &original,
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
        &rotated,
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
        &rotated,
        "provider-account",
    )
    .unwrap()
    .is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_profile_refresh_only_rotation_updates_other_bound_profiles() {
    let (root, home, backups) = profile_dirs("managed-refresh-only-rotation");
    let peer = root.join("peer-profile");
    fs::create_dir_all(&peer).unwrap();
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    fs::write(peer.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    let secrets = MemorySecrets::default();
    let original = TokenSet::new(
        "access-stable",
        Some("refresh-original".into()),
        Some("id-original".into()),
        Some(60_000),
        1,
        1,
    )
    .unwrap();
    for profile in [&home, &peer] {
        attach_account_with(
            profile,
            &backups,
            "account-local",
            &original,
            "provider-account",
            &secrets,
        )
        .unwrap();
    }

    let rotated = TokenSet::new(
        "access-stable",
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
        &original,
        "provider-account",
    )
    .unwrap()
    .expect("refresh-only rotation must be adopted");
    assert_eq!(update.access_token, "access-stable");
    assert_eq!(update.refresh_token, "refresh-rotated");
    assert_eq!(update.id_token.as_deref(), Some("id-rotated"));

    assert_eq!(
        sync_account_bindings(&backups, "account-local", &rotated, "provider-account").unwrap(),
        1
    );
    let peer_auth = fs::read_to_string(peer.join(AUTH_FILE)).unwrap();
    assert!(peer_auth.contains("refresh-rotated"));
    assert!(peer_auth.contains("id-rotated"));
    assert!(managed_account_token_update(
        &home,
        &backups,
        "account-local",
        &rotated,
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
fn local_gateway_restore_adopts_oauth_rotation_before_switching_to_chatgpt() {
    let (root, home, backups) = profile_dirs("local-gateway-restore-after-oauth-rotation");
    fs::write(home.join(CONFIG_FILE), "model_provider = \"custom\"\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"access_token\":\"original\"}}",
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let original = TokenSet::new(
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
                tokens: &original,
                provider_account_id: "provider-account",
            }),
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();

    let rotated = TokenSet::new(
        "bound-access-rotated",
        Some("bound-refresh-rotated".into()),
        Some("bound-id-rotated".into()),
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
        &original,
        "provider-account",
    )
    .unwrap()
    .expect("rotated OAuth token");
    assert_eq!(update.access_token, rotated.access_token());
    sync_local_gateway_binding(
        &home,
        &backups,
        "account-local",
        &rotated,
        "provider-account",
    )
    .unwrap();

    restore_with(&home, &backups, &secrets).unwrap();
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

#[test]
fn sync_default_service_tier_preserves_codex_profile_state() {
    let (root, home, _) = profile_dirs("service-tier");
    fs::write(
        home.join(CONFIG_FILE),
        "model_provider = \"custom\"\n\n[desktop]\nappearanceTheme = \"dark\"\n",
    )
    .unwrap();
    fs::write(
        home.join(GLOBAL_STATE_FILE),
        r#"{"other":1,"electron-persisted-atom-state":{"theme":"dark"}}"#,
    )
    .unwrap();

    sync_default_service_tier(&home, DefaultServiceTier::Fast).unwrap();
    let config = fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    assert_eq!(
        config["desktop"][DESKTOP_DEFAULT_SERVICE_TIER_KEY].as_str(),
        Some("priority")
    );
    assert_eq!(
        config[TOP_LEVEL_SERVICE_TIER_KEY].as_str(),
        Some("priority")
    );
    assert_eq!(config["desktop"]["appearanceTheme"].as_str(), Some("dark"));
    let state: Value =
        serde_json::from_str(&fs::read_to_string(home.join(GLOBAL_STATE_FILE)).unwrap()).unwrap();
    assert_eq!(state["other"], 1);
    assert_eq!(state[PERSISTED_ATOM_STATE_KEY]["theme"], "dark");
    assert_eq!(
        state[PERSISTED_ATOM_STATE_KEY][DESKTOP_DEFAULT_SERVICE_TIER_KEY],
        "priority"
    );
    assert_eq!(
        state[PERSISTED_ATOM_STATE_KEY][SERVICE_TIER_CHANGED_KEY],
        true
    );

    sync_default_service_tier(&home, DefaultServiceTier::Ultrafast).unwrap();
    let config = fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    assert_eq!(
        config["desktop"][DESKTOP_DEFAULT_SERVICE_TIER_KEY].as_str(),
        Some("ultrafast")
    );
    assert_eq!(
        config[TOP_LEVEL_SERVICE_TIER_KEY].as_str(),
        Some("ultrafast")
    );
    let state: Value =
        serde_json::from_str(&fs::read_to_string(home.join(GLOBAL_STATE_FILE)).unwrap()).unwrap();
    assert_eq!(
        state[PERSISTED_ATOM_STATE_KEY][DESKTOP_DEFAULT_SERVICE_TIER_KEY],
        "ultrafast"
    );

    sync_default_service_tier(&home, DefaultServiceTier::Standard).unwrap();
    let config = fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    assert!(config["desktop"]
        .as_table()
        .unwrap()
        .get(DESKTOP_DEFAULT_SERVICE_TIER_KEY)
        .is_none());
    assert_eq!(config[TOP_LEVEL_SERVICE_TIER_KEY].as_str(), Some("default"));
    assert_eq!(config["desktop"]["appearanceTheme"].as_str(), Some("dark"));
    let state: Value =
        serde_json::from_str(&fs::read_to_string(home.join(GLOBAL_STATE_FILE)).unwrap()).unwrap();
    assert_eq!(state["other"], 1);
    assert!(state[PERSISTED_ATOM_STATE_KEY][DESKTOP_DEFAULT_SERVICE_TIER_KEY].is_null());
    assert_eq!(
        state[PERSISTED_ATOM_STATE_KEY][SERVICE_TIER_CHANGED_KEY],
        true
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

#[test]
fn direct_source_catalog_uses_models_dev_capabilities_without_overriding_context() {
    let (root, home, _backups) = profile_dirs("direct-source-models-dev");
    ensure_test_native_catalog(&home);
    let metadata = ModelMetadataCatalog::from_models_dev_json(r#"{
        "test/text-only":{"modalities":{"input":["text"],"output":["text"]},
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
    assert_eq!(value["models"][0]["input_modalities"], json!(["text"]));
    assert!(value["models"][0].get("context_window").is_none());
    assert_eq!(
        value["models"][1]["input_modalities"],
        json!(["text", "image"])
    );
    assert_eq!(value["models"][1]["supported_reasoning_levels"], json!([]));
    assert!(value["models"][1].get("context_window").is_none());
    let managed = catalog::build_managed_model_catalog(&home, None, None, &catalog).unwrap();
    let managed: Value = serde_json::from_str(&managed).unwrap();
    assert_eq!(managed["models"], value["models"]);
    fs::remove_dir_all(root).unwrap();
}
