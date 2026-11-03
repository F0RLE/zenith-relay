use super::*;

#[derive(Default)]
struct FailBindingInstall {
    memory: MemorySecrets,
    fail: std::cell::Cell<bool>,
}

impl SecretBackend for FailBindingInstall {
    fn save(&self, reference: &str, value: &str) -> Result<()> {
        if self.fail.get() && reference.starts_with("profile:codex:snapshot-undo:") {
            return Err(LocalPoolError::invalid_state(
                "synthetic binding installation failure",
            ));
        }
        self.memory.save(reference, value)
    }
    fn load(&self, reference: &str) -> Result<Option<String>> {
        self.memory.load(reference)
    }
    fn delete(&self, reference: &str) -> Result<()> {
        self.memory.delete(reference)
    }
}

#[test]
fn restored_account_binding_refreshes_tokens_and_retains_reusable_undo() {
    let (root, home, backups) = profile_dirs("snapshot-account-binding");
    let secrets = MemorySecrets::default();
    let original = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-original"}"#;
    fs::write(home.join(AUTH_FILE), original).unwrap();
    let tokens = TokenSet::new(
        "synthetic-access",
        Some("synthetic-refresh".into()),
        Some("synthetic-id".into()),
        Some(60000),
        1,
        1,
    )
    .unwrap();
    attach_account_with(
        &home,
        &backups,
        "synthetic-account",
        &tokens,
        "synthetic-provider",
        &secrets,
    )
    .unwrap();
    let saved = account_ops::capture_user_profile_snapshot(&home, &backups, &secrets).unwrap();
    assert!(saved.binding.is_some());
    restore_account_with(&home, &backups, &secrets).unwrap();
    for _ in 0..2 {
        fs::write(home.join(CONFIG_FILE), "model_reasoning_effort = 'high'\nmodel_context_window = 200\n[profiles.other]\nmodel_provider = 'other-api'\n").unwrap();
        account_ops::restore_user_profile_snapshot_full_with(
            &home,
            &backups,
            &saved.profile,
            saved.catalog.as_deref(),
            saved.binding.as_ref(),
            &secrets,
        )
        .unwrap();
        let bindings = profile_bindings(&home, &backups).unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].credential_id, "synthetic-account");
        assert!(bindings[0].active);
        let refreshed = TokenSet::new(
            "synthetic-refreshed",
            Some("synthetic-next-refresh".into()),
            Some("synthetic-next-id".into()),
            Some(120000),
            2,
            2,
        )
        .unwrap();
        assert_eq!(
            sync_account_bindings(
                &backups,
                "synthetic-account",
                &refreshed,
                "synthetic-provider"
            )
            .unwrap(),
            1
        );
        let auth: Value = serde_json::from_slice(&fs::read(home.join(AUTH_FILE)).unwrap()).unwrap();
        assert_eq!(auth["tokens"]["access_token"], "synthetic-refreshed");
        restore_account_with(&home, &backups, &secrets).unwrap();
        assert_eq!(fs::read_to_string(home.join(AUTH_FILE)).unwrap(), original);
        let config = fs::read_to_string(home.join(CONFIG_FILE)).unwrap();
        assert!(config.contains("model_reasoning_effort = 'high'"));
        assert!(config.contains("other-api"));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restored_gateway_binding_refreshes_catalog_and_disconnects() {
    let (root, home, backups) = profile_dirs("snapshot-gateway-binding");
    let secrets = MemorySecrets::default();
    let catalog =
        json!({"models": [routed_codex_catalog_entry(None, "synthetic-routed", 1, None)]});
    attach_with_catalog_for_test(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "synthetic-key",
        &catalog.to_string(),
        &secrets,
    )
    .unwrap();
    let saved = account_ops::capture_user_profile_snapshot(&home, &backups, &secrets).unwrap();
    assert!(saved.binding.is_some());
    restore_with(&home, &backups, &secrets).unwrap();
    account_ops::restore_user_profile_snapshot_full_with(
        &home,
        &backups,
        &saved.profile,
        saved.catalog.as_deref(),
        saved.binding.as_ref(),
        &secrets,
    )
    .unwrap();
    assert!(profile_bindings(&home, &backups).unwrap()[0].active);
    let catalog =
        json!({"models": [routed_codex_catalog_entry(None, "synthetic-updated", 1, None)]});
    assert!(refresh_managed_model_catalog(&home, &backups, &catalog.to_string(), None).unwrap());
    assert!(profile_bindings(&home, &backups).unwrap()[0].active);
    restore_with(&home, &backups, &secrets).unwrap();
    assert!(profile_bindings(&home, &backups).unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_binding_install_restores_current_files_and_protected_undo() {
    let (root, home, backups) = profile_dirs("snapshot-binding-rollback");
    fs::write(
        home.join(AUTH_FILE),
        r#"{"OPENAI_API_KEY":"synthetic-baseline"}"#,
    )
    .unwrap();
    let secrets = FailBindingInstall::default();
    attach_with(
        &home,
        &backups,
        "http://127.0.0.1:14998/v1",
        "synthetic-one",
        &secrets,
    )
    .unwrap();
    let saved = account_ops::capture_user_profile_snapshot(&home, &backups, &secrets).unwrap();
    let config = fs::read(home.join(CONFIG_FILE)).unwrap();
    let auth = fs::read(home.join(AUTH_FILE)).unwrap();
    let backup = fs::read(backup_path(&backups)).unwrap();
    let protected = secrets.memory.0.lock().unwrap().clone();
    secrets.fail.set(true);
    assert!(
        account_ops::restore_user_profile_snapshot_full_with(
            &home,
            &backups,
            &saved.profile,
            saved.catalog.as_deref(),
            saved.binding.as_ref(),
            &secrets
        )
        .is_err()
    );
    assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), config);
    assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), auth);
    assert_eq!(fs::read(backup_path(&backups)).unwrap(), backup);
    assert_eq!(*secrets.memory.0.lock().unwrap(), protected);
    assert!(profile_bindings(&home, &backups).unwrap()[0].active);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_keyring_snapshot_refreshes_and_detaches_on_the_native_backend() {
    use snapshot_credentials::{AuthKeyring, NativeAuthKeyring};
    let (root, home, backups) = profile_dirs("snapshot-managed-keyring");
    let secrets = MemorySecrets::default();
    let original = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-original"}"#;
    fs::write(
        home.join(CONFIG_FILE),
        "cli_auth_credentials_store = 'keyring'\n",
    )
    .unwrap();
    NativeAuthKeyring
        .replace(&home, cfg!(windows), Some(original))
        .unwrap();
    let tokens = TokenSet::new(
        "synthetic-access",
        Some("synthetic-refresh".into()),
        Some("synthetic-id".into()),
        Some(60000),
        1,
        1,
    )
    .unwrap();
    attach_account_with(
        &home,
        &backups,
        "synthetic-account",
        &tokens,
        "synthetic-provider",
        &secrets,
    )
    .unwrap();
    assert!(!home.join(AUTH_FILE).exists());
    let saved = account_ops::capture_user_profile_snapshot(&home, &backups, &secrets).unwrap();
    assert!(saved.binding.is_some());
    account_ops::restore_user_profile_snapshot_full_with(
        &home,
        &backups,
        &saved.profile,
        saved.catalog.as_deref(),
        saved.binding.as_ref(),
        &secrets,
    )
    .unwrap();
    let refreshed = TokenSet::new(
        "synthetic-new-access",
        Some("synthetic-new-refresh".into()),
        Some("synthetic-new-id".into()),
        Some(120000),
        2,
        2,
    )
    .unwrap();
    assert_eq!(
        sync_account_bindings(
            &backups,
            "synthetic-account",
            &refreshed,
            "synthetic-provider"
        )
        .unwrap(),
        1
    );
    let auth: Value = serde_json::from_str(
        &NativeAuthKeyring
            .load(&home, cfg!(windows))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(auth["tokens"]["access_token"], "synthetic-new-access");
    assert!(!home.join(AUTH_FILE).exists());
    assert!(profile_bindings(&home, &backups).unwrap()[0].active);
    restore_account_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        NativeAuthKeyring
            .load(&home, cfg!(windows))
            .unwrap()
            .as_deref(),
        Some(original)
    );
    assert!(!home.join(AUTH_FILE).exists());
    assert!(profile_bindings(&home, &backups).unwrap().is_empty());
    NativeAuthKeyring
        .replace(&home, cfg!(windows), None)
        .unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn gateway_keyring_snapshot_preserves_the_original_native_login() {
    use snapshot_credentials::{AuthKeyring, NativeAuthKeyring};
    let (root, home, backups) = profile_dirs("snapshot-gateway-keyring");
    let secrets = MemorySecrets::default();
    let original = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-original"}"#;
    fs::write(
        home.join(CONFIG_FILE),
        "cli_auth_credentials_store = 'keyring'\n",
    )
    .unwrap();
    NativeAuthKeyring
        .replace(&home, cfg!(windows), Some(original))
        .unwrap();
    let catalog =
        json!({"models": [routed_codex_catalog_entry(None, "synthetic-routed", 1, None)]})
            .to_string();
    switch_to_local_with(
        &home,
        &backups,
        "synthetic-key-id",
        "http://127.0.0.1:14998/v1",
        "synthetic-key",
        LocalAttachOptions {
            catalog_json: Some(&catalog),
            ..LocalAttachOptions::default()
        },
        &secrets,
    )
    .unwrap();
    assert!(!home.join(AUTH_FILE).exists());
    let saved = account_ops::capture_user_profile_snapshot(&home, &backups, &secrets).unwrap();
    restore_with(&home, &backups, &secrets).unwrap();
    account_ops::restore_user_profile_snapshot_full_with(
        &home,
        &backups,
        &saved.profile,
        saved.catalog.as_deref(),
        saved.binding.as_ref(),
        &secrets,
    )
    .unwrap();
    assert!(profile_bindings(&home, &backups).unwrap()[0].active);
    restore_with(&home, &backups, &secrets).unwrap();
    assert_eq!(
        NativeAuthKeyring
            .load(&home, cfg!(windows))
            .unwrap()
            .as_deref(),
        Some(original)
    );
    assert!(!home.join(AUTH_FILE).exists());
    assert!(profile_bindings(&home, &backups).unwrap().is_empty());
    NativeAuthKeyring
        .replace(&home, cfg!(windows), None)
        .unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_native_switch_rolls_back_credentials_and_auto_fallback() {
    use snapshot_credentials::{AuthKeyring, NativeAuthKeyring};
    let (root, home, _) = profile_dirs("native-switch-rollback");
    let secrets = MemorySecrets::default();
    let before = r#"{"OPENAI_API_KEY":"synthetic-original"}"#;
    let after = r#"{"OPENAI_API_KEY":"synthetic-updated"}"#;
    fs::write(
        home.join(CONFIG_FILE),
        "cli_auth_credentials_store = 'auto'\n",
    )
    .unwrap();
    fs::write(home.join(AUTH_FILE), before).unwrap();
    let result: Result<()> = switch_transaction::run(&home, &secrets, |_| {
        snapshot_credentials::update_auth(&home, &Some(before.as_bytes().to_vec()), Some(after))?;
        assert!(!home.join(AUTH_FILE).exists());
        Err(LocalPoolError::invalid_state(
            "synthetic failure after native update",
        ))
    });
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(home.join(AUTH_FILE)).unwrap(), before);
    assert!(
        NativeAuthKeyring
            .load(&home, cfg!(windows))
            .unwrap()
            .is_none()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_switch_rollback_preserves_a_newer_external_login() {
    use snapshot_credentials::{AuthKeyring, NativeAuthKeyring};
    let (root, home, _) = profile_dirs("native-switch-external-writer");
    let secrets = MemorySecrets::default();
    let before = r#"{"OPENAI_API_KEY":"synthetic-original"}"#;
    let after = r#"{"OPENAI_API_KEY":"synthetic-updated"}"#;
    let external = r#"{"OPENAI_API_KEY":"synthetic-external"}"#;
    fs::write(
        home.join(CONFIG_FILE),
        "cli_auth_credentials_store = 'keyring'\n",
    )
    .unwrap();
    NativeAuthKeyring
        .replace(&home, cfg!(windows), Some(before))
        .unwrap();
    let result: Result<()> = switch_transaction::run(&home, &secrets, |_| {
        snapshot_credentials::update_auth(&home, &Some(before.as_bytes().to_vec()), Some(after))?;
        NativeAuthKeyring.replace(&home, cfg!(windows), Some(external))?;
        Err(LocalPoolError::invalid_state(
            "synthetic failure after external login",
        ))
    });
    assert!(result.is_err());
    assert_eq!(
        NativeAuthKeyring
            .load(&home, cfg!(windows))
            .unwrap()
            .as_deref(),
        Some(external)
    );
    NativeAuthKeyring
        .replace(&home, cfg!(windows), None)
        .unwrap();
    fs::remove_dir_all(root).unwrap();
}
