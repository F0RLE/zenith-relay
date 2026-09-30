use super::*;

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
    assert!(
        set_local_gateway_websockets_with_backend(&home, &backups, false, None, &secrets).is_err()
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), current);
    set_local_gateway_websockets_with_backend(&home, &backups, false, None, &secrets).unwrap();
    restore_with(&home, &backups, &secrets).unwrap();
    let restored = parse_config(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(restored["user_setting"].as_str(), Some("keep"));
    assert!(restored.get("model_providers").is_none());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn websocket_change_is_scoped_to_the_expected_connection() {
    let (root, home, backups) = profile_dirs("websocket-connection-scope");
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "fixture-key",
        &secrets,
    )
    .unwrap();
    let config = fs::read(home.join(CONFIG_FILE)).unwrap();
    let backup = fs::read(backup_path(&backups)).unwrap();
    assert_eq!(
        set_local_gateway_websockets_with_backend(
            &home,
            &backups,
            false,
            Some("another-pool"),
            &secrets,
        )
        .unwrap(),
        None,
    );
    assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), config);
    assert_eq!(fs::read(backup_path(&backups)).unwrap(), backup);
    assert_eq!(
        set_local_gateway_websockets_with_backend(
            &home,
            &backups,
            false,
            Some("local_gateway"),
            &secrets,
        )
        .unwrap(),
        Some(true),
    );
    let document = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(
        document["model_providers"][PROVIDER_ID]["supports_websockets"].as_bool(),
        Some(false)
    );
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
        None,
        &MemorySecrets::default(),
    )
    .unwrap();

    assert_eq!(previous, None);
    assert!(!home.exists());
    assert!(!backups.exists());
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

    set_local_gateway_websockets_with_backend(&home, &backups, false, None, &secrets).unwrap();
    assert!(fs::read_to_string(home.join(CONFIG_FILE))
        .unwrap()
        .contains("supports_websockets = false"));
    let backup_file = backup_path(&backups);
    let backup: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&backup_file).unwrap()).unwrap();
    assert_eq!(backup["managedSupportsWebsockets"], false);

    set_local_gateway_websockets_with_backend(&home, &backups, true, None, &secrets).unwrap();
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

    set_local_gateway_websockets_with_backend(&home, &backups, false, None, &secrets).unwrap();
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
