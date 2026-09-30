use super::*;
use serde_json::json;

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
fn invalid_backup_metadata_reports_which_invariant_failed_without_writing() {
    for (field, replacement, reason) in [
        ("version", json!(2), "unsupported backup version"),
        ("managedKeyHash", json!("bad"), "managed key fingerprint"),
        ("managedBaseUrl", json!(""), "managed gateway address"),
    ] {
        let (root, home, backups) = profile_dirs("invalid-backup-reason");
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "local-key",
            &secrets,
        )
        .unwrap();
        let config = fs::read(home.join(CONFIG_FILE)).unwrap();
        let auth = fs::read(home.join(AUTH_FILE)).unwrap();
        let path = backup_path(&backups);
        let mut backup: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        backup[field] = replacement;
        let corrupted = serde_json::to_vec(&backup).unwrap();
        fs::write(&path, &corrupted).unwrap();

        let error = local_backup(&home, &backups).unwrap_err();
        assert_eq!(error.code, ErrorCode::RecoveryRequired);
        assert!(error.message.contains(reason), "{field}: {}", error.message);
        assert_eq!(fs::read(&path).unwrap(), corrupted);
        assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), config);
        assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), auth);
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn restore_preserves_fresh_login_and_removes_only_relay_settings() {
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
    let auth_before = fs::read(home.join(AUTH_FILE)).unwrap();

    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        "model_provider = \"openai\"\n"
    );
    assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), auth_before);
    assert!(!backup_path(&backups).exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn restore_preserves_an_unrecognized_auth_document() {
    for (case, external_auth) in [
        ("non-json", b"not a JSON login".as_slice()),
        ("non-utf8", &[0xff, 0xfe][..]),
    ] {
        let (root, home, backups) = profile_dirs(&format!("external-auth-format-{case}"));
        let secrets = MemorySecrets::default();
        attach_with(
            &home,
            &backups,
            "http://127.0.0.1:14998/v1",
            "zlr_key",
            &secrets,
        )
        .unwrap();
        fs::write(home.join(AUTH_FILE), external_auth).unwrap();

        restore_with(&home, &backups, &secrets).unwrap();
        assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), external_auth);
        assert!(!backup_path(&backups).exists());
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn restore_preserves_changed_provider_origin() {
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
    restore_with(&home, &backups, &secrets).unwrap();
    let restored = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(restored.contains("model_provider = \"openai\""));
    assert!(restored.contains("14999"));
    assert!(!restored.contains("zlr_key"));
    assert!(!backup_path(&backups).exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn restore_preserves_changed_gateway_bearer() {
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

    restore_with(&home, &backups, &secrets).unwrap();
    let restored = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
    assert!(restored.contains("experimental_bearer_token = \"zlr_other\""));
    assert!(!restored.contains("zlr_key"));
    assert!(!backup_path(&backups).exists());
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
fn switching_to_an_account_does_not_replace_a_fresh_login() {
    let (root, home, backups) = profile_dirs("switch-fresh-login");
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "zlr_key",
        &secrets,
    )
    .unwrap();
    let fresh_auth = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"fresh"}}"#;
    fs::write(home.join(AUTH_FILE), fresh_auth).unwrap();
    let config_before = fs::read(home.join(CONFIG_FILE)).unwrap();
    let backup_before = fs::read(backup_path(&backups)).unwrap();
    let tokens = TokenSet::new("managed", None, None, Some(60_000), 1, 1).unwrap();

    let error = switch_to_account_with(
        &home,
        &backups,
        "account-local",
        &tokens,
        "provider-account",
        &secrets,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProfileRestoreBlocked);
    assert_eq!(
        fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
        fresh_auth
    );
    assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), config_before);
    assert_eq!(fs::read(backup_path(&backups)).unwrap(), backup_before);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn explicit_local_reconnect_uses_the_new_login_as_its_restore_baseline() {
    let (root, home, backups) = profile_dirs("explicit-reconnect-fresh-login");
    fs::write(home.join(CONFIG_FILE), "model_provider = 'openai'\n").unwrap();
    fs::write(
        home.join(AUTH_FILE),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"old"}}"#,
    )
    .unwrap();
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "first-key",
        &secrets,
    )
    .unwrap();

    let fresh_auth = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"fresh"}}"#;
    fs::write(home.join(AUTH_FILE), fresh_auth).unwrap();
    let mut config = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    config["user_setting"] = value("keep");
    fs::write(home.join(CONFIG_FILE), config.to_string()).unwrap();

    switch_to_local_with(
        &home,
        &backups,
        "local_gateway",
        "http://127.0.0.1:14998/v1",
        "next-key",
        LocalAttachOptions {
            rebase_newer_login: true,
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();
    assert_eq!(
        local_backup(&home, &backups)
            .unwrap()
            .unwrap()
            .previous_auth_hash
            .as_deref(),
        Some(bytes_hash(fresh_auth.as_bytes()).as_str())
    );

    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
        fresh_auth
    );
    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(root_model_provider(&restored).as_deref(), Some("openai"));
    assert_eq!(restored["user_setting"].as_str(), Some("keep"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn explicit_account_activation_rebases_new_login_from_either_managed_profile() {
    for from_account in [false, true] {
        let (root, home, backups) = profile_dirs("explicit-account-fresh-login");
        let secrets = MemorySecrets::default();
        let tokens = TokenSet::new("managed", None, None, Some(60_000), 1, 1).unwrap();
        if from_account {
            attach_account_with(&home, &backups, "old", &tokens, "provider", &secrets).unwrap();
        } else {
            attach_with(
                &home,
                &backups,
                "http://127.0.0.1:14998/v1",
                "local-key",
                &secrets,
            )
            .unwrap();
        }
        let fresh_auth = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"fresh"}}"#;
        fs::write(home.join(AUTH_FILE), fresh_auth).unwrap();

        switch_to_account_with_intent(&home, &backups, "next", &tokens, "provider", true, &secrets)
            .unwrap();
        restore_account_with(&home, &backups, &secrets).unwrap();
        assert_eq!(
            fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
            fresh_auth
        );
        assert_eq!(profile_backup_count(&backups), 0);
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn explicit_account_activation_keeps_an_external_config_change() {
    let (root, home, backups) = profile_dirs("explicit-account-external-config");
    fs::write(home.join(CONFIG_FILE), "model_provider = 'original'\n").unwrap();
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new("managed", None, None, Some(60_000), 1, 1).unwrap();
    attach_account_with(&home, &backups, "old", &tokens, "provider", &secrets).unwrap();
    fs::write(
        home.join(CONFIG_FILE),
        "model_provider = 'external'\nuser_setting = 'keep'\n",
    )
    .unwrap();

    switch_to_account_with_intent(&home, &backups, "next", &tokens, "provider", true, &secrets)
        .unwrap();
    restore_account_with(&home, &backups, &secrets).unwrap();
    let restored = parse_config(&fs::read_to_string(home.join(CONFIG_FILE)).unwrap()).unwrap();
    assert_eq!(root_model_provider(&restored).as_deref(), Some("external"));
    assert_eq!(restored["user_setting"].as_str(), Some("keep"));
    assert_eq!(profile_backup_count(&backups), 0);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn explicit_local_activation_rebases_new_login_from_managed_account() {
    let (root, home, backups) = profile_dirs("explicit-local-from-account-fresh-login");
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new("managed", None, None, Some(60_000), 1, 1).unwrap();
    attach_account_with(&home, &backups, "old", &tokens, "provider", &secrets).unwrap();
    let fresh_auth = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"fresh"}}"#;
    fs::write(home.join(AUTH_FILE), fresh_auth).unwrap();

    switch_to_local_with(
        &home,
        &backups,
        "local_gateway",
        "http://127.0.0.1:14998/v1",
        "next-key",
        LocalAttachOptions {
            rebase_newer_login: true,
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();
    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
        fresh_auth
    );
    assert_eq!(profile_backup_count(&backups), 0);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn failed_explicit_reconnect_rolls_back_without_replacing_new_login() {
    for from_account in [false, true] {
        let (root, home, backups) = profile_dirs("failed-explicit-reconnect");
        let secrets = SwitchFaultSecrets::default();
        let tokens = TokenSet::new("managed", None, None, Some(60_000), 1, 1).unwrap();
        if from_account {
            attach_account_with(&home, &backups, "old", &tokens, "provider", &secrets).unwrap();
        } else {
            attach_with(
                &home,
                &backups,
                "http://127.0.0.1:14998/v1",
                "local-key",
                &secrets,
            )
            .unwrap();
        }
        let fresh_auth = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"fresh"}}"#;
        fs::write(home.join(AUTH_FILE), fresh_auth).unwrap();
        let original_config = fs::read(home.join(CONFIG_FILE)).unwrap();
        let backup_file = if from_account {
            account_backup_for_profile(&home, &backups)
                .unwrap()
                .unwrap()
        } else {
            backup_path(&backups)
        };
        let original_backup = fs::read(&backup_file).unwrap();
        let original_secrets = secrets.memory.0.lock().unwrap().clone();
        *secrets.fail_projection_save.lock().unwrap() = true;

        let error = switch_to_local_with(
            &home,
            &backups,
            "next",
            "http://127.0.0.1:14998/v1",
            "next-key",
            LocalAttachOptions {
                rebase_newer_login: true,
                ..LocalAttachOptions::default()
            },
            &secrets,
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::SecretStoreUnavailable);
        assert_eq!(
            fs::read_to_string(home.join(AUTH_FILE)).unwrap(),
            fresh_auth
        );
        assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), original_config);
        assert_eq!(fs::read(&backup_file).unwrap(), original_backup);
        assert_eq!(*secrets.memory.0.lock().unwrap(), original_secrets);
        fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn explicit_reconnect_does_not_overwrite_a_login_written_during_activation() {
    let (root, home, backups) = profile_dirs("explicit-reconnect-concurrent-login");
    let original_config = "model_provider = \"openai\"\n";
    fs::write(home.join(CONFIG_FILE), original_config).unwrap();
    let secrets = MemorySecrets::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "first-key",
        &secrets,
    )
    .unwrap();
    let auth_path = home.join(AUTH_FILE);
    fs::write(
        &auth_path,
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"before-reconnect"}}"#,
    )
    .unwrap();
    let concurrent_auth =
        br#"{"auth_mode":"chatgpt","tokens":{"access_token":"concurrent-login"}}"#;
    let mutating = MutatingSecrets::new(auth_path.clone(), concurrent_auth.to_vec());
    *mutating.values.lock().unwrap() = secrets.0.lock().unwrap().clone();

    let error = switch_to_local_with(
        &home,
        &backups,
        "local_gateway",
        "http://127.0.0.1:14998/v1",
        "next-key",
        LocalAttachOptions {
            rebase_newer_login: true,
            ..LocalAttachOptions::default()
        },
        &mutating,
    )
    .unwrap_err();
    assert!(matches!(
        error.code,
        ErrorCode::ProfileRestoreBlocked | ErrorCode::RecoveryRequired
    ));
    assert_eq!(fs::read(auth_path).unwrap(), concurrent_auth);
    assert_eq!(
        fs::read_to_string(home.join(CONFIG_FILE)).unwrap(),
        original_config
    );
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
