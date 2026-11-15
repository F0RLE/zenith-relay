use super::*;
use snapshot_credentials::AuthKeyring;

const NATIVE: &str =
    r#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic-keyring-login"}}"#;

struct EditingKeyring {
    target: &'static str,
    updated: Option<&'static str>,
    auth: Option<&'static str>,
}

impl AuthKeyring for EditingKeyring {
    fn load(&self, home: &Path, _: bool) -> Result<Option<String>> {
        let path = home.join(self.target);
        match self.updated {
            Some(content) => fs::write(path, content).unwrap(),
            None => fs::remove_file(path).unwrap(),
        }
        Ok(self.auth.map(str::to_owned))
    }

    fn replace(&self, _: &Path, _: bool, _: Option<&str>) -> Result<()> {
        panic!("capturing a snapshot must not replace native credentials")
    }
}

#[test]
fn snapshot_capture_rejects_files_changed_during_native_credential_read() {
    for target in [CONFIG_FILE, AUTH_FILE] {
        for removed in [false, true] {
            let (root, home, backups) = profile_dirs("snapshot-capture-consistency");
            let mode = if target == CONFIG_FILE {
                "keyring"
            } else {
                "auto"
            };
            fs::write(
                home.join(CONFIG_FILE),
                format!("cli_auth_credentials_store = '{mode}'\nmodel_provider = 'openai'\n"),
            )
            .unwrap();
            fs::write(
                home.join(AUTH_FILE),
                r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-original-file"}"#,
            )
            .unwrap();
            let updated = if removed {
                None
            } else if target == CONFIG_FILE {
                Some(
                    "cli_auth_credentials_store = 'file'\nmodel_provider = 'synthetic-new-provider'\n",
                )
            } else {
                Some(r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-new-file"}"#)
            };
            let result = account_ops::snapshot_user_profile_locked(
                &home,
                &EditingKeyring {
                    target,
                    updated,
                    auth: (target == CONFIG_FILE).then_some(NATIVE),
                },
            );
            let error = match result {
                Err(error) => error,
                Ok(_) => {
                    fs::remove_dir_all(root).unwrap();
                    panic!("captured a mixed snapshot: {target}, removed={removed}");
                }
            };
            assert_eq!(error.code, ErrorCode::Conflict);
            assert!(!error.message.contains("synthetic-keyring-login"));
            assert_eq!(
                fs::read_to_string(home.join(target)).ok().as_deref(),
                updated
            );
            assert!(!backup_path(&backups).exists());
            // A later, stable capture remains available after an ordinary edit.
            fs::write(
                home.join(CONFIG_FILE),
                "cli_auth_credentials_store = 'file'\n",
            )
            .unwrap();
            let stable = account_ops::snapshot_user_profile_locked(
                &home,
                &snapshot_credentials::NativeAuthKeyring,
            )
            .unwrap();
            assert_eq!(stable.auth, fs::read_to_string(home.join(AUTH_FILE)).ok());
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn native_capture_ignores_edits_to_an_unselected_fallback_file() {
    for mode in ["keyring", "auto"] {
        for removed in [false, true] {
            let (root, home, _) = profile_dirs("snapshot-unselected-fallback");
            fs::write(
                home.join(CONFIG_FILE),
                format!("cli_auth_credentials_store = '{mode}'\n"),
            )
            .unwrap();
            // Equal initial documents must not make the file look authoritative.
            fs::write(home.join(AUTH_FILE), NATIVE).unwrap();
            let updated = (!removed)
                .then_some(r#"{"auth_mode":"apikey","OPENAI_API_KEY":"synthetic-newer-fallback"}"#);
            let captured = account_ops::snapshot_user_profile_locked(
                &home,
                &EditingKeyring {
                    target: AUTH_FILE,
                    updated,
                    auth: Some(NATIVE),
                },
            )
            .unwrap();
            assert_eq!(captured.auth.as_deref(), Some(NATIVE));
            assert_eq!(
                fs::read_to_string(home.join(AUTH_FILE)).ok().as_deref(),
                updated
            );
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn native_capture_does_not_decode_an_unselected_non_utf8_fallback_file() {
    for mode in ["keyring", "auto"] {
        let (root, home, _) = profile_dirs("snapshot-non-utf8-fallback");
        fs::write(
            home.join(CONFIG_FILE),
            format!("cli_auth_credentials_store = '{mode}'\n"),
        )
        .unwrap();
        let bytes = [0xff, 0xfe];
        fs::write(home.join(AUTH_FILE), bytes).unwrap();
        snapshot_credentials::NativeAuthKeyring
            .replace(&home, cfg!(windows), Some(NATIVE))
            .unwrap();
        let captured = account_ops::snapshot_user_profile_locked(
            &home,
            &snapshot_credentials::NativeAuthKeyring,
        )
        .unwrap();
        assert_eq!(captured.auth.as_deref(), Some(NATIVE));
        assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), bytes);
        snapshot_credentials::NativeAuthKeyring
            .replace(&home, cfg!(windows), None)
            .unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
