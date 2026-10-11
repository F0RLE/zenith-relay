use super::*;

#[test]
fn context_and_compaction_overrides_are_managed_and_restored_independently() {
    use zenith_relay_core::catalog::context_policy::{
        ApplicationContextPolicy, CompactionPolicy, ContextWindowPolicy,
    };
    let (root, home, backups) = profile_dirs("independent-context-policy");
    fs::write(
        home.join(CONFIG_FILE),
        "model_context_window = 200000\nmodel_auto_compact_token_limit = 190000\n",
    )
    .unwrap();
    let compact_only = ApplicationContextPolicy::Configured {
        context: ContextWindowPolicy::Client,
        compaction: CompactionPolicy::Custom { tokens: 150000 },
    };
    context::sync_context_policy(&home, &backups, compact_only).unwrap();
    let current = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(current["model_context_window"].as_integer(), Some(200000));
    assert!(current.get("model_auto_compact_token_limit").is_none());
    let window_only = ApplicationContextPolicy::Configured {
        context: ContextWindowPolicy::Extended,
        compaction: CompactionPolicy::Client,
    };
    let change = context::sync_context_policy(&home, &backups, window_only).unwrap();
    let current = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert!(current.get("model_context_window").is_none());
    assert_eq!(
        current["model_auto_compact_token_limit"].as_integer(),
        Some(190000)
    );
    change.rollback().unwrap();
    let current = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(current["model_context_window"].as_integer(), Some(200000));
    assert!(current.get("model_auto_compact_token_limit").is_none());
    context::sync_context_policy(&home, &backups, window_only).unwrap();
    context::sync_context_policy(&home, &backups, ApplicationContextPolicy::Client).unwrap();
    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(restored["model_context_window"].as_integer(), Some(200000));
    assert_eq!(
        restored["model_auto_compact_token_limit"].as_integer(),
        Some(190000)
    );
    assert!(!backups.join("codex-context-overrides.json").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn context_policy_removes_only_active_overrides_and_restores_previous_values() {
    let (root, home, backups) = profile_dirs("context-policy-restore");
    let original = concat!(
        "profile = 'active'\nmodel_context_window = 200000\nmodel_auto_compact_token_limit = 190000\n",
        "[profiles.active]\nmodel_context_window = 128000\nmodel_auto_compact_token_limit = 110000\n",
        "[profiles.other]\nmodel_context_window = 64000\n"
    );
    fs::write(home.join(CONFIG_FILE), original).unwrap();
    sync_context_overrides(&home, &backups, true).unwrap();
    let mut active = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert!(active.get("model_context_window").is_none());
    assert!(active["profiles"]["active"]
        .get("model_context_window")
        .is_none());
    assert_eq!(
        active["profiles"]["other"]["model_context_window"].as_integer(),
        Some(64000)
    );
    // Newer user values must survive a return to client-owned settings.
    active["model_context_window"] = value(180000);
    fs::write(home.join(CONFIG_FILE), active.to_string()).unwrap();
    assert!(sync_context_overrides(&home, &backups, true).is_err());
    sync_context_overrides(&home, &backups, false).unwrap();
    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(restored["model_context_window"].as_integer(), Some(180000));
    assert_eq!(
        restored["model_auto_compact_token_limit"].as_integer(),
        Some(190000)
    );
    assert_eq!(
        restored["profiles"]["active"]["model_context_window"].as_integer(),
        Some(128000)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn context_change_rollback_restores_exact_config_and_undo_state() {
    let (root, home, backups) = profile_dirs("context-policy-rollback");
    let original = "# user comment\nmodel_context_window = 200000\n";
    fs::write(home.join(CONFIG_FILE), original).unwrap();
    let change = sync_context_overrides(&home, &backups, true).unwrap();
    change.rollback().unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        original
    );
    assert!(!backups.join("codex-context-overrides.json").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_profile_attach_can_restore_the_preceding_context_change() {
    let (root, home, backups) = profile_dirs("context-policy-attach-failure");
    let original = "# user comment\nmodel_context_window = 200000\n";
    fs::write(home.join(CONFIG_FILE), original).unwrap();
    let change = sync_context_overrides(&home, &backups, true).unwrap();
    assert!(attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        "{\"models\":[]}",
        &MemorySecrets::default(),
    )
    .is_err());
    change.rollback().unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        original
    );
    assert!(!backups.join("codex-context-overrides.json").exists());
    assert!(credential_kind(&home, &backups).unwrap().is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn client_context_without_undo_does_not_create_or_modify_config() {
    let (root, home, backups) = profile_dirs("context-policy-client");
    sync_context_overrides(&home, &backups, false).unwrap();
    assert!(!home.join(CONFIG_FILE).exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn client_mode_recovers_when_context_removal_was_interrupted_after_saving_undo() {
    let (root, home, backups) = profile_dirs("context-policy-interrupted");
    let original = "model_context_window = 200000\nmodel_auto_compact_token_limit = 190000\n";
    fs::write(home.join(CONFIG_FILE), original).unwrap();
    sync_context_overrides(&home, &backups, true).unwrap();
    // Recreate an interruption after durable undo, before the config replacement.
    fs::write(home.join(CONFIG_FILE), original).unwrap();
    sync_context_overrides(&home, &backups, false).unwrap();
    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(restored["model_context_window"].as_integer(), Some(200000));
    assert_eq!(
        restored["model_auto_compact_token_limit"].as_integer(),
        Some(190000)
    );
    assert!(!backups.join("codex-context-overrides.json").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_catalog_policy_applies_after_native_normalization_and_can_return_to_client() {
    let (root, home, backups) = profile_dirs("context-catalog-policy");
    let mut native = routed_codex_catalog_entry(None, "gpt-native", 1, Some(128000));
    native["slug"] = json!("gpt-native");
    native["context_window"] = json!(64000);
    native["max_context_window"] = json!(128000);
    let document = json!({
        "models": [native],
        "relay_context_policy": {"mode": "custom", "tokens": 1000000},
        "relay_context_limits": { "gpt-native": {"context": 128000, "input": 100000, "output": 32000} }
    });
    let managed =
        catalog::build_managed_model_catalog(&home, None, None, &document.to_string()).unwrap();
    let managed: Value = serde_json::from_str(&managed).unwrap();
    assert_eq!(managed["models"][0]["context_window"], 128000);
    assert_eq!(managed["models"][0]["auto_compact_token_limit"], 91200);
    assert!(managed.get("relay_context_policy").is_none());
    let original = "model_context_window = 200000\nmodel_auto_compact_token_limit = 190000\n";
    fs::write(home.join(CONFIG_FILE), original).unwrap();
    let secrets = MemorySecrets::default();
    // The host applies the context change before committing a new profile.
    sync_context_overrides(&home, &backups, true).unwrap();
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &document.to_string(),
        &secrets,
    )
    .unwrap();
    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        original
    );
    assert!(!backups.join("codex-context-overrides.json").exists());
    fs::remove_dir_all(root).unwrap();
}
