use super::*;

mod credentials;
mod managed;

#[test]
fn snapshot_catalog_resolves_directory_aliases_before_and_after_removal() {
    let (root, _home, backups) = profile_dirs("snapshot-catalog-alias");
    let nested = backups.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let path = managed_model_catalog_path(&backups).unwrap();
    let alias = nested.join("..").join(MODEL_CATALOG_FILE);
    let catalog = r#"{"models":[{"slug":"synthetic-model"}]}"#;
    fs::write(&path, catalog).unwrap();
    let saved = UserProfileSnapshot {
        config: Some(format!("model_catalog_json = '{}'\n", alias.display())),
        auth: None,
    };
    assert_eq!(
        snapshot::capture_managed_catalog(&backups, &saved)
            .unwrap()
            .as_deref(),
        Some(catalog)
    );
    fs::remove_file(&path).unwrap();
    assert!(snapshot::references_catalog(saved.config.as_deref(), &path).unwrap());

    let foreign = root.join(MODEL_CATALOG_FILE);
    fs::write(&foreign, catalog).unwrap();
    let external = UserProfileSnapshot {
        config: Some(format!("model_catalog_json = '{}'\n", foreign.display())),
        auth: None,
    };
    assert!(
        snapshot::capture_managed_catalog(&backups, &external)
            .unwrap()
            .is_none()
    );
    assert!(!snapshot::references_catalog(external.config.as_deref(), &path).unwrap());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn snapshot_captures_active_connection_instead_of_attachment_baseline() {
    let (root, home, backups) = profile_dirs("snapshot-current");
    let secrets = MemorySecrets::default();
    fs::write(home.join(CONFIG_FILE), "model_provider = 'openai'\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic-original"}}"#,
    )
    .unwrap();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "synthetic-secret",
        &secrets,
    )
    .unwrap();
    let snapshot = account_ops::snapshot_user_profile(&home).unwrap();
    assert_eq!(
        snapshot.config.as_deref(),
        Some(fs::read_to_string(home.join(CONFIG_FILE)).unwrap().as_str())
    );
    assert_eq!(
        snapshot.auth.as_deref(),
        Some(fs::read_to_string(home.join(AUTH_FILE)).unwrap().as_str())
    );
    assert_eq!(snapshot::history_provider(&snapshot).unwrap(), PROVIDER_ID);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_restore_returns_account_and_preserves_newer_preferences() {
    let (root, home, backups) = profile_dirs("snapshot-account");
    let saved = UserProfileSnapshot {
        config: Some("model_provider = 'openai'\napproval_policy = 'on-request'\n[features]\nold_flag = true\n".into()),
        auth: Some(r#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic-saved"}}"#.into()),
    };
    fs::write(home.join(CONFIG_FILE), "model_provider = 'zenith_relay_local'\nmodel = 'relay-only-model'\napproval_policy = 'untrusted'\n[features]\nnew_flag = true\n[mcp_servers.synthetic]\ncommand = 'synthetic-command'\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-new","extension":{"new":true}}"#,
    )
    .unwrap();
    let chat = home.join("conversation-sentinel.jsonl");
    fs::write(&chat, "synthetic-new-conversation").unwrap();
    account_ops::restore_user_profile_snapshot_full_with(
        &home,
        &backups,
        &saved,
        None,
        None,
        &MemorySecrets::default(),
    )
    .unwrap();
    let config = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(root_model_provider(&config).as_deref(), Some("openai"));
    assert!(root_model(&config).is_none());
    assert_eq!(config["approval_policy"].as_str(), Some("untrusted"));
    assert_eq!(config["features"]["new_flag"].as_bool(), Some(true));
    assert!(config["features"].get("old_flag").is_none());
    assert_eq!(
        config["mcp_servers"]["synthetic"]["command"].as_str(),
        Some("synthetic-command")
    );
    let auth: Value = serde_json::from_slice(&fs::read(home.join(AUTH_FILE)).unwrap()).unwrap();
    assert_eq!(auth["auth_mode"], "chatgpt");
    assert_eq!(auth["tokens"]["access_token"], "synthetic-saved");
    assert!(auth.get("OPENAI_API_KEY").is_none());
    assert_eq!(auth["extension"]["new"], true);
    assert_eq!(
        fs::read_to_string(chat).unwrap(),
        "synthetic-new-conversation"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn api_snapshot_restores_selected_route_without_overwriting_other_profiles() {
    let saved = UserProfileSnapshot {
        config: Some("profile = 'work'\n[profiles.work]\nmodel_provider = 'saved-api'\napproval_policy = 'on-request'\n[model_providers.saved-api]\nbase_url = 'https://saved.invalid/v1'\nwire_api = 'responses'\n".into()),
        auth: Some(r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-api"}"#.into()),
    };
    let current = "profile = 'other'\n[profiles.work]\nmodel_provider = 'changed-api'\napproval_policy = 'untrusted'\n[profiles.other]\nmodel_provider = 'other-api'\n[model_providers.saved-api]\nbase_url = 'https://changed.invalid/v1'\n[model_providers.other-api]\nbase_url = 'https://other.invalid/v1'\n";
    let restored = snapshot::restore_connection(
        &saved,
        Some(current),
        Some(r#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic-new"}}"#),
    )
    .unwrap();
    let config = parse_config(restored.config.as_deref().unwrap()).unwrap();
    assert_eq!(snapshot::history_provider(&restored).unwrap(), "saved-api");
    assert_eq!(
        config["profiles"]["work"]["approval_policy"].as_str(),
        Some("untrusted")
    );
    assert_eq!(
        config["profiles"]["other"]["model_provider"].as_str(),
        Some("other-api")
    );
    assert_eq!(
        config["model_providers"]["saved-api"]["base_url"].as_str(),
        Some("https://saved.invalid/v1")
    );
    assert_eq!(
        config["model_providers"]["other-api"]["base_url"].as_str(),
        Some("https://other.invalid/v1")
    );
    let auth: Value = serde_json::from_str(restored.auth.as_deref().unwrap()).unwrap();
    assert_eq!(auth["auth_mode"], "apikey");
    assert!(auth.get("tokens").is_none());
}

#[test]
fn history_uses_saved_route_even_with_oauth_and_absence_clears_credentials() {
    let saved = UserProfileSnapshot {
        config: Some("model_provider = 'zenith_relay_local'\n".into()),
        auth: Some(r#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic"}}"#.into()),
    };
    assert_eq!(
        snapshot::history_provider(&saved).unwrap(),
        "zenith_relay_local"
    );
    let empty = UserProfileSnapshot {
        config: None,
        auth: None,
    };
    let restored = snapshot::restore_connection(
        &empty,
        Some("model_provider = 'external'\napproval_policy = 'untrusted'\n"),
        saved.auth.as_deref(),
    )
    .unwrap();
    assert!(restored.auth.is_none());
    assert_eq!(snapshot::history_provider(&restored).unwrap(), "openai");
    assert!(restored.config.unwrap().contains("approval_policy"));
}

#[test]
fn restore_preserves_preferences_and_existing_inactive_profiles() {
    let saved = UserProfileSnapshot {
        config: Some("profile = 'work'\nmodel_reasoning_effort = 'low'\nmodel_context_window = 100\n[profiles.work]\nmodel_provider = 'saved-api'\nmodel = 'saved-model'\n[profiles.personal]\nmodel_provider = 'obsolete-api'\nmodel = 'obsolete-model'\n[model_providers.saved-api]\nbase_url = 'https://saved.invalid/v1'\n[model_providers.obsolete-api]\nbase_url = 'https://obsolete.invalid/v1'\n".into()),
        auth: None,
    };
    let current = "profile = 'work'\nmodel_reasoning_effort = 'high'\nmodel_context_window = 200\nmodel_auto_compact_token_limit = 150\n[profiles.work]\nmodel_provider = 'saved-api'\nmodel = 'new-preference'\nmodel_reasoning_effort = 'high'\n[profiles.personal]\nmodel_provider = 'current-api'\nmodel = 'personal-model'\n[model_providers.saved-api]\nbase_url = 'https://saved.invalid/v1'\n[model_providers.current-api]\nbase_url = 'https://current.invalid/v1'\n";
    let restored = snapshot::restore_connection(&saved, Some(current), None).unwrap();
    let config = parse_config(restored.config.as_deref().unwrap()).unwrap();
    assert_eq!(config["model_reasoning_effort"].as_str(), Some("high"));
    assert_eq!(config["model_context_window"].as_integer(), Some(200));
    assert_eq!(
        config["model_auto_compact_token_limit"].as_integer(),
        Some(150)
    );
    assert_eq!(
        config["profiles"]["work"]["model"].as_str(),
        Some("new-preference")
    );
    assert_eq!(
        config["profiles"]["work"]["model_reasoning_effort"].as_str(),
        Some("high")
    );
    assert_eq!(
        config["profiles"]["personal"]["model_provider"].as_str(),
        Some("current-api")
    );
    assert_eq!(
        config["profiles"]["personal"]["model"].as_str(),
        Some("personal-model")
    );
    assert!(config["model_providers"].get("obsolete-api").is_none());
}

#[test]
fn restore_does_not_recreate_deleted_inactive_profiles() {
    let saved = UserProfileSnapshot {
        config: Some("[profiles.deleted]\nmodel_provider = 'obsolete'\n".into()),
        auth: None,
    };
    let restored =
        snapshot::restore_connection(&saved, Some("model_reasoning_effort = 'high'\n"), None)
            .unwrap();
    let config = parse_config(restored.config.as_deref().unwrap()).unwrap();
    assert!(config.get("profiles").is_none());
    assert_eq!(config["model_reasoning_effort"].as_str(), Some("high"));
}
