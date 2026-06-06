use std::{fs, path::PathBuf};

use crate::platform::app_data_dir;

const SAVED_KEY_FILE: &str = "zenith.key";
const KEYRING_SERVICE: &str = "Zenith Codex";
const KEYRING_USER: &str = "api-key";

pub fn save_app_key(api_key: &str) -> Result<(), String> {
    keyring_entry()
        .set_password(api_key)
        .map_err(|err| format!("Не удалось сохранить ключ приложения в хранилище ОС: {err}"))?;
    let _ = fs::remove_file(legacy_saved_key_path());
    Ok(())
}

pub fn load_saved_app_key() -> Option<String> {
    if let Ok(key) = keyring_entry().get_password() {
        let key = key.trim().to_string();
        return (!key.is_empty()).then_some(key);
    }

    let legacy_path = legacy_saved_key_path();
    let key = fs::read_to_string(&legacy_path).ok()?;
    let key = key.trim().to_string();
    if !key.is_empty() && save_app_key(&key).is_ok() {
        let _ = fs::remove_file(legacy_path);
    }
    (!key.is_empty()).then_some(key)
}

pub fn delete_saved_app_key() {
    let _ = keyring_entry().delete_credential();
    let _ = fs::remove_file(legacy_saved_key_path());
}

fn keyring_entry() -> keyring::Entry {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).expect("valid keyring service and user")
}

fn legacy_saved_key_path() -> PathBuf {
    app_data_dir().join(SAVED_KEY_FILE)
}
