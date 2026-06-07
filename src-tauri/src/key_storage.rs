const KEYRING_SERVICE: &str = "Zenith Codex";
const KEYRING_USER: &str = "api-key";

pub fn save_app_key(api_key: &str) -> Result<(), String> {
    keyring_entry()
        .set_password(api_key)
        .map_err(|err| format!("Не удалось сохранить ключ приложения в хранилище ОС: {err}"))?;
    Ok(())
}

pub fn load_saved_app_key() -> Option<String> {
    if let Ok(key) = keyring_entry().get_password() {
        let key = key.trim().to_string();
        return (!key.is_empty()).then_some(key);
    }

    None
}

pub fn delete_saved_app_key() {
    let _ = keyring_entry().delete_credential();
}

fn keyring_entry() -> keyring::Entry {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).expect("valid keyring service and user")
}
