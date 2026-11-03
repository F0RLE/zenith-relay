use super::*;
use snapshot_credentials::AuthKeyring;
use std::cell::{Cell, RefCell};

#[derive(Default)]
struct MemoryKeyring {
    auth: RefCell<Option<String>>,
    unavailable: Cell<bool>,
}

impl AuthKeyring for MemoryKeyring {
    fn load(&self, _: &Path, _: bool) -> Result<Option<String>> {
        if self.unavailable.get() {
            return Err(LocalPoolError::invalid_state("synthetic locked store"));
        }
        Ok(self.auth.borrow().clone())
    }
    fn replace(&self, _: &Path, _: bool, auth: Option<&str>) -> Result<()> {
        if self.unavailable.get() {
            return Err(LocalPoolError::invalid_state("synthetic locked store"));
        }
        *self.auth.borrow_mut() = auth.map(str::to_owned);
        Ok(())
    }
}

const SAVED: &str = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic-saved"}}"#;
const NEWER: &str = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-newer"}"#;

#[test]
fn keyring_snapshot_captures_selected_login_and_restores_without_fallback_file() {
    for mode in ["keyring", "auto"] {
        let (root, home, backups) = profile_dirs("snapshot-native-auth");
        fs::write(
            home.join(CONFIG_FILE),
            format!("cli_auth_credentials_store = '{mode}'\n"),
        )
        .unwrap();
        fs::write(home.join(AUTH_FILE), NEWER).unwrap();
        let keyring = MemoryKeyring::default();
        keyring.replace(&home, false, Some(SAVED)).unwrap();
        let saved = account_ops::snapshot_user_profile_locked(&home, &keyring).unwrap();
        assert_eq!(saved.auth.as_deref(), Some(SAVED));
        keyring.replace(&home, false, Some(NEWER)).unwrap();
        fs::write(
            home.join(CONFIG_FILE),
            "cli_auth_credentials_store = 'file'\nmodel_reasoning_effort = 'high'\n",
        )
        .unwrap();
        account_ops::restore_snapshot_with_keyring(
            &home,
            &backups,
            &saved,
            None,
            None,
            &MemorySecrets::default(),
            &keyring,
        )
        .unwrap();
        assert_eq!(keyring.load(&home, false).unwrap().as_deref(), Some(SAVED));
        assert!(!home.join(AUTH_FILE).exists());
        assert!(
            fs::read_to_string(home.join(CONFIG_FILE))
                .unwrap()
                .contains("model_reasoning_effort = 'high'")
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn auto_capture_uses_file_only_when_keyring_entry_is_absent() {
    let (root, home, _) = profile_dirs("snapshot-auto-fallback");
    fs::write(
        home.join(CONFIG_FILE),
        "cli_auth_credentials_store = 'auto'\n",
    )
    .unwrap();
    fs::write(home.join(AUTH_FILE), SAVED).unwrap();
    let keyring = MemoryKeyring::default();
    assert_eq!(
        account_ops::snapshot_user_profile_locked(&home, &keyring)
            .unwrap()
            .auth
            .as_deref(),
        Some(SAVED)
    );
    keyring.unavailable.set(true);
    assert!(account_ops::snapshot_user_profile_locked(&home, &keyring).is_err());
    assert_eq!(fs::read_to_string(home.join(AUTH_FILE)).unwrap(), SAVED);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_credentials_roll_back_when_later_binding_cleanup_fails() {
    let (root, home, backups) = profile_dirs("snapshot-keyring-rollback");
    let config = "cli_auth_credentials_store = 'file'\n";
    fs::write(home.join(CONFIG_FILE), config).unwrap();
    fs::write(home.join(AUTH_FILE), NEWER).unwrap();
    fs::create_dir_all(&backups).unwrap();
    fs::write(backup_path(&backups), "invalid-binding").unwrap();
    let keyring = MemoryKeyring::default();
    keyring.replace(&home, false, Some(NEWER)).unwrap();
    let saved = UserProfileSnapshot {
        config: Some("cli_auth_credentials_store = 'keyring'\n".into()),
        auth: Some(SAVED.into()),
    };
    assert!(
        account_ops::restore_snapshot_with_keyring(
            &home,
            &backups,
            &saved,
            None,
            None,
            &MemorySecrets::default(),
            &keyring
        )
        .is_err()
    );
    assert_eq!(keyring.load(&home, false).unwrap().as_deref(), Some(NEWER));
    assert_eq!(fs::read_to_string(home.join(CONFIG_FILE)).unwrap(), config);
    assert_eq!(fs::read_to_string(home.join(AUTH_FILE)).unwrap(), NEWER);
    assert_eq!(
        fs::read_to_string(backup_path(&backups)).unwrap(),
        "invalid-binding"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_restore_does_not_overwrite_other_native_credentials() {
    let (root, home, backups) = profile_dirs("snapshot-return-to-file");
    fs::write(
        home.join(CONFIG_FILE),
        "cli_auth_credentials_store = 'keyring'\n",
    )
    .unwrap();
    let keyring = MemoryKeyring::default();
    keyring.replace(&home, false, Some(NEWER)).unwrap();
    let saved = UserProfileSnapshot {
        config: None,
        auth: Some(SAVED.into()),
    };
    account_ops::restore_snapshot_with_keyring(
        &home,
        &backups,
        &saved,
        None,
        None,
        &MemorySecrets::default(),
        &keyring,
    )
    .unwrap();
    assert_eq!(keyring.load(&home, false).unwrap().as_deref(), Some(NEWER));
    assert_eq!(fs::read_to_string(home.join(AUTH_FILE)).unwrap(), SAVED);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unsupported_and_legacy_uncaptured_backends_never_become_empty_logins() {
    for config in [
        "cli_auth_credentials_store = 'ephemeral'\n",
        "cli_auth_credentials_store = 123\n",
    ] {
        assert!(snapshot_credentials::store(Some(config)).is_err());
    }
    let saved = UserProfileSnapshot {
        config: Some("cli_auth_credentials_store = 'keyring'\n".into()),
        auth: None,
    };
    assert!(snapshot_credentials::validate_legacy_snapshot_auth(&saved).is_err());
}

#[test]
fn backend_selection_matches_codex_and_honors_explicit_overrides() {
    assert_eq!(
        snapshot_credentials::encrypted_backend(None).unwrap(),
        cfg!(windows)
    );
    for enabled in [true, false] {
        let config = format!("[features]\nsecret_auth_storage = {enabled}\n");
        assert_eq!(
            snapshot_credentials::encrypted_backend(Some(&config)).unwrap(),
            enabled
        );
    }
    assert!(
        snapshot_credentials::encrypted_backend(Some(
            "[features]\nsecret_auth_storage = 'invalid'\n"
        ))
        .is_err()
    );
}
