const KEYRING_SERVICE: &str = "Zenith Relay";
const LEGACY_KEYRING_SERVICE: &str = "Zenith Codex";
const KEYRING_USER: &str = "api-key";
const PREVIOUS_AUTH_USER: &str = "previous-codex-auth-json";

pub fn save_app_key(api_key: &str) -> Result<(), String> {
    save_named_secret(KEYRING_USER, api_key)
}

pub fn load_saved_app_key() -> Option<String> {
    load_named_secret(KEYRING_USER)
}

pub fn delete_saved_app_key() {
    delete_named_secret(KEYRING_USER);
}

pub fn save_previous_codex_auth(content: &str) -> Result<(), String> {
    save_named_secret(PREVIOUS_AUTH_USER, content)
}

pub fn load_previous_codex_auth() -> Option<String> {
    load_named_secret(PREVIOUS_AUTH_USER)
}

pub fn delete_previous_codex_auth() {
    delete_named_secret(PREVIOUS_AUTH_USER);
}

pub fn save_named_secret(user: &str, value: &str) -> Result<(), String> {
    keyring_entry_for(user)
        .set_password(value)
        .map_err(|err| format!("Не удалось сохранить секрет в хранилище ОС: {err}"))
}

pub fn load_named_secret(user: &str) -> Option<String> {
    if let Some(value) = load_from_service(KEYRING_SERVICE, user) {
        return Some(value);
    }

    let value = load_from_service(LEGACY_KEYRING_SERVICE, user)?;
    if keyring_entry_for_service(KEYRING_SERVICE, user)
        .set_password(&value)
        .is_ok()
    {
        let _ = keyring_entry_for_service(LEGACY_KEYRING_SERVICE, user).delete_credential();
    }
    Some(value)
}

pub fn delete_named_secret(user: &str) {
    let _ = keyring_entry_for(user).delete_credential();
    let _ = keyring_entry_for_service(LEGACY_KEYRING_SERVICE, user).delete_credential();
}

fn keyring_entry_for(user: &str) -> keyring::Entry {
    keyring_entry_for_service(KEYRING_SERVICE, user)
}

fn keyring_entry_for_service(service: &str, user: &str) -> keyring::Entry {
    keyring::Entry::new(service, user).expect("valid keyring service and user")
}

fn load_from_service(service: &str, user: &str) -> Option<String> {
    keyring_entry_for_service(service, user)
        .get_password()
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
