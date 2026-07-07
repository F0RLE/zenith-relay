const KEYRING_SERVICE: &str = "Zenith Codex";
const KEYRING_USER: &str = "api-key";
const PREVIOUS_AUTH_USER: &str = "previous-codex-auth-json";

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

pub fn save_previous_codex_auth(content: &str) -> Result<(), String> {
    keyring_entry_for(PREVIOUS_AUTH_USER)
        .set_password(content)
        .map_err(|err| format!("Не удалось сохранить прежнюю авторизацию Codex: {err}"))?;
    Ok(())
}

pub fn load_previous_codex_auth() -> Option<String> {
    keyring_entry_for(PREVIOUS_AUTH_USER)
        .get_password()
        .ok()
        .map(|content| content.trim().to_string())
        .filter(|content| !content.is_empty())
}

pub fn delete_previous_codex_auth() {
    let _ = keyring_entry_for(PREVIOUS_AUTH_USER).delete_credential();
}

fn keyring_entry() -> keyring::Entry {
    keyring_entry_for(KEYRING_USER)
}

fn keyring_entry_for(user: &str) -> keyring::Entry {
    keyring::Entry::new(KEYRING_SERVICE, user).expect("valid keyring service and user")
}
