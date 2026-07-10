use std::sync::{Mutex, MutexGuard, OnceLock};

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

pub fn delete_saved_app_key() -> Result<(), String> {
    delete_named_secret_result(KEYRING_USER)
}

pub fn save_previous_codex_auth(content: &str) -> Result<(), String> {
    save_named_secret(PREVIOUS_AUTH_USER, content)
}

pub fn load_previous_codex_auth() -> Option<String> {
    load_named_secret(PREVIOUS_AUTH_USER)
}

pub fn delete_previous_codex_auth() -> Result<(), String> {
    delete_named_secret_result(PREVIOUS_AUTH_USER)
}

pub fn save_named_secret(user: &str, value: &str) -> Result<(), String> {
    let _guard = keyring_guard()?;
    keyring_entry_for(user)
        .set_password(value)
        .map_err(|err| format!("Не удалось сохранить секрет в хранилище ОС: {err}"))
}

pub fn load_named_secret(user: &str) -> Option<String> {
    load_named_secret_result(user).ok().flatten()
}

pub fn load_named_secret_result(user: &str) -> Result<Option<String>, String> {
    let _guard = keyring_guard()?;
    if let Some(value) = load_from_service(KEYRING_SERVICE, user)? {
        return Ok(Some(value));
    }

    let Some(value) = load_from_service(LEGACY_KEYRING_SERVICE, user)? else {
        return Ok(None);
    };
    if keyring_entry_for_service(KEYRING_SERVICE, user)
        .set_password(&value)
        .is_ok()
    {
        let _ = keyring_entry_for_service(LEGACY_KEYRING_SERVICE, user).delete_credential();
    }
    Ok(Some(value))
}

pub fn delete_named_secret_result(user: &str) -> Result<(), String> {
    let _guard = keyring_guard()?;
    let current = delete_from_service(KEYRING_SERVICE, user);
    let legacy = delete_from_service(LEGACY_KEYRING_SERVICE, user);
    current.and(legacy)
}

fn keyring_guard() -> Result<MutexGuard<'static, ()>, String> {
    static KEYRING_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    KEYRING_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "Хранилище секретов заблокировано после внутренней ошибки".to_string())
}

fn keyring_entry_for(user: &str) -> keyring::Entry {
    keyring_entry_for_service(KEYRING_SERVICE, user)
}

fn keyring_entry_for_service(service: &str, user: &str) -> keyring::Entry {
    keyring::Entry::new(service, user).expect("valid keyring service and user")
}

fn load_from_service(service: &str, user: &str) -> Result<Option<String>, String> {
    match keyring_entry_for_service(service, user).get_password() {
        Ok(value) => {
            let value = value.trim().to_string();
            Ok((!value.is_empty()).then_some(value))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!(
            "Не удалось прочитать секрет из хранилища ОС: {error}"
        )),
    }
}

fn delete_from_service(service: &str, user: &str) -> Result<(), String> {
    match keyring_entry_for_service(service, user).delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!(
            "Не удалось удалить секрет из хранилища ОС: {error}"
        )),
    }
}
