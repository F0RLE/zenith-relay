use super::*;

fn rollout(profile: &Path, name: &str, provider: &str) -> std::path::PathBuf {
    let sessions = profile.join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    let path = sessions.join(format!("rollout-{name}.jsonl"));
    fs::write(&path, format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{name}\",\"model_provider\":\"{provider}\"}}}}\n{{\"type\":\"event_msg\",\"payload\":{{\"text\":\"synthetic-private-message\"}}}}\n")).unwrap();
    path
}

#[test]
fn restoring_saved_api_or_account_repairs_old_and_new_chats() {
    for provider in ["openai", "saved-custom-api", "zenith_relay_local"] {
        let root = crate::files::test_temp_root("snapshot-history-mode");
        let profile = root.join("profile");
        let backups = root.join("backups");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            profile.join("config.toml"),
            format!("model_provider = '{provider}'\n"),
        )
        .unwrap();
        let auth = if provider == "openai" {
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic-saved"}}"#
        } else {
            r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-saved"}"#
        };
        fs::write(profile.join("auth.json"), auth).unwrap();
        let first = rollout(&profile, "before-snapshot", "changed-route");
        let secrets = MemorySecrets::default();
        let snapshot =
            create_with(&profile, &backups, "Saved connection", false, &secrets).unwrap();
        let newer = rollout(&profile, "after-snapshot", "changed-route");
        fs::write(
            profile.join("config.toml"),
            "model_provider = 'changed-route'\napproval_policy = 'untrusted'\n",
        )
        .unwrap();
        fs::write(
            profile.join("auth.json"),
            r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-newer"}"#,
        )
        .unwrap();

        restore_full_with(
            &profile,
            &backups,
            &snapshot.id,
            (&root.join("state"), &root.join("history")),
            &secrets,
        )
        .unwrap();

        assert_eq!(fs::read_to_string(profile.join("auth.json")).unwrap(), auth);
        assert!(
            fs::read_to_string(profile.join("config.toml"))
                .unwrap()
                .contains("approval_policy = 'untrusted'")
        );
        for path in [first, newer] {
            let content = fs::read_to_string(path).unwrap();
            let meta: serde_json::Value =
                serde_json::from_str(content.lines().next().unwrap()).unwrap();
            assert_eq!(meta["payload"]["model_provider"], provider);
            assert_eq!(
                content.lines().nth(1).unwrap(),
                r#"{"type":"event_msg","payload":{"text":"synthetic-private-message"}}"#
            );
        }
        assert_eq!(list_with(&backups, &secrets).unwrap().snapshots.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn failed_authorization_restore_rolls_back_history_without_losing_new_chats() {
    let root = crate::files::test_temp_root("snapshot-history-rollback");
    let profile = root.join("profile");
    let backups = root.join("backups");
    fs::create_dir_all(&profile).unwrap();
    fs::write(profile.join("config.toml"), "model_provider = 'openai'\n").unwrap();
    let secrets = MemorySecrets::default();
    let snapshot = create_with(&profile, &backups, "Saved account", false, &secrets).unwrap();
    let current_config = "model_provider = 'changed-route'\n";
    fs::write(profile.join("config.toml"), current_config).unwrap();
    fs::write(profile.join("auth.json"), "invalid-current-auth").unwrap();
    let newer = rollout(&profile, "newer-chat", "changed-route");
    let original = fs::read(&newer).unwrap();
    let result = restore_full_with(
        &profile,
        &backups,
        &snapshot.id,
        (&root.join("state"), &root.join("history")),
        &secrets,
    );
    assert!(result.is_err());
    assert_eq!(fs::read(&newer).unwrap(), original);
    assert_eq!(
        fs::read_to_string(profile.join("config.toml")).unwrap(),
        current_config
    );
    assert_eq!(
        fs::read_to_string(profile.join("auth.json")).unwrap(),
        "invalid-current-auth"
    );
    assert_eq!(list_with(&backups, &secrets).unwrap().snapshots.len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn saved_managed_catalog_survives_switching_away_and_back() {
    let root = crate::files::test_temp_root("snapshot-catalog");
    let profile = root.join("profile");
    let backups = root.join("backups");
    fs::create_dir_all(&profile).unwrap();
    fs::create_dir_all(&backups).unwrap();
    let catalog_path = backups.join("codex-model-catalog.json");
    let catalog = r#"{"models":[{"slug":"synthetic-model"}]}"#;
    fs::write(&catalog_path, catalog).unwrap();
    fs::write(
        profile.join("config.toml"),
        format!(
            "model_provider = 'zenith_relay_local'\nmodel_catalog_json = '{}'\n",
            catalog_path.display()
        ),
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    let snapshot = create_with(&profile, &backups, "Saved API", false, &secrets).unwrap();
    fs::remove_file(&catalog_path).unwrap();
    fs::write(profile.join("config.toml"), "model_provider = 'openai'\n").unwrap();
    restore_full_with(
        &profile,
        &backups,
        &snapshot.id,
        (&root.join("state"), &root.join("history")),
        &secrets,
    )
    .unwrap();
    assert_eq!(fs::read_to_string(catalog_path).unwrap(), catalog);
    assert!(
        fs::read_to_string(profile.join("config.toml"))
            .unwrap()
            .contains("zenith_relay_local")
    );
    fs::remove_dir_all(root).unwrap();
}
