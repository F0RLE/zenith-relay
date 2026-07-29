use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    sync::{Mutex, MutexGuard, OnceLock},
    thread,
    time::Duration,
};

#[cfg(test)]
use std::collections::HashMap;

#[cfg(test)]
type TestKeyring = HashMap<(String, String), String>;

const KEYRING_SERVICE: &str = "Zenith Relay";
const LEGACY_KEYRING_SERVICE: &str = "Zenith Codex";
const KEYRING_USER: &str = "api-key";
const PREVIOUS_AUTH_USER: &str = "previous-codex-auth-json";
const CHUNK_UTF16_UNITS: usize = 1024;
const MAX_CHUNKS: usize = 4096;
const MANIFEST_PREFIX: &str = "__zenith_relay_secret_manifest__:";
const MANIFEST_VERSION: u8 = 1;
const READBACK_ATTEMPTS: usize = 3;
const READBACK_DELAY: Duration = Duration::from_millis(20);

#[derive(Debug, Deserialize, Serialize)]
struct SecretManifest {
    version: u8,
    generation: String,
    count: usize,
    sha256: String,
}

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
    save_to_service(KEYRING_SERVICE, user, value)
}

pub fn load_named_secret(user: &str) -> Option<String> {
    load_named_secret_result(user).ok().flatten()
}

pub fn load_named_secret_result(user: &str) -> Result<Option<String>, String> {
    let _guard = keyring_guard()?;
    if let Some(value) = load_secret_from_service(KEYRING_SERVICE, user)? {
        return Ok(Some(value));
    }

    let Some(value) = load_secret_from_service(LEGACY_KEYRING_SERVICE, user)? else {
        return Ok(None);
    };
    if save_to_service(KEYRING_SERVICE, user, &value).is_ok() {
        let _ = delete_secret_from_service(LEGACY_KEYRING_SERVICE, user);
    }
    Ok(Some(value))
}

pub fn delete_named_secret_result(user: &str) -> Result<(), String> {
    let _guard = keyring_guard()?;
    let current = delete_secret_from_service(KEYRING_SERVICE, user);
    let legacy = delete_secret_from_service(LEGACY_KEYRING_SERVICE, user);
    current.and(legacy)
}

fn keyring_guard() -> Result<MutexGuard<'static, ()>, String> {
    static KEYRING_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    KEYRING_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "Хранилище секретов заблокировано после внутренней ошибки".to_string())
}

#[cfg(not(test))]
fn keyring_entry_for_service(service: &str, user: &str) -> keyring::Entry {
    keyring::Entry::new(service, user).expect("valid keyring service and user")
}

fn save_to_service(service: &str, user: &str, value: &str) -> Result<(), String> {
    let previous_raw = load_raw_from_service(service, user)?;
    let previous_manifest = previous_raw
        .as_deref()
        .and_then(|stored| decode_manifest(stored).ok());

    if value.encode_utf16().count() <= CHUNK_UTF16_UNITS {
        set_password(service, user, value)?;
        if let Err(error) = verify_saved_secret(service, user, value) {
            restore_raw_secret(service, user, previous_raw.as_deref())?;
            return Err(error);
        }
        if let Some(manifest) = previous_manifest {
            delete_manifest_chunks(service, user, &manifest);
        }
        return Ok(());
    }

    let chunks = split_secret(value);
    if chunks.len() > MAX_CHUNKS {
        return Err("Секрет слишком велик для защищённого хранилища ОС".to_string());
    }

    let generation = uuid::Uuid::new_v4().simple().to_string();
    let mut written_users: Vec<String> = Vec::with_capacity(chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        let chunk_user = chunk_user(user, &generation, index);
        if let Err(error) = set_password(service, &chunk_user, chunk) {
            for written_user in written_users {
                let _ = delete_from_service(service, &written_user);
            }
            return Err(error);
        }
        written_users.push(chunk_user);
    }

    let manifest = SecretManifest {
        version: MANIFEST_VERSION,
        generation,
        count: chunks.len(),
        sha256: sha256_hex(value.as_bytes()),
    };
    let encoded = encode_manifest(&manifest)?;
    if let Err(error) = set_password(service, user, &encoded) {
        for written_user in written_users {
            let _ = delete_from_service(service, &written_user);
        }
        return Err(error);
    }
    if let Err(error) = verify_saved_secret(service, user, value) {
        let restore = restore_raw_secret(service, user, previous_raw.as_deref());
        delete_manifest_chunks(service, user, &manifest);
        restore?;
        return Err(error);
    }

    if let Some(previous) = previous_manifest {
        delete_manifest_chunks(service, user, &previous);
    }
    Ok(())
}

fn verify_saved_secret(service: &str, user: &str, expected: &str) -> Result<(), String> {
    let mut last_error = None;
    for attempt in 0..READBACK_ATTEMPTS {
        if attempt > 0 {
            thread::sleep(READBACK_DELAY);
        }
        match load_secret_from_service(service, user) {
            Ok(Some(value)) if value == expected => return Ok(()),
            Ok(_) => last_error = None,
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| "Не удалось проверить сохранённый секрет в хранилище ОС".to_string()))
}

fn restore_raw_secret(service: &str, user: &str, previous: Option<&str>) -> Result<(), String> {
    match previous {
        Some(value) => set_password(service, user, value),
        None => delete_from_service(service, user),
    }
}

fn load_secret_from_service(service: &str, user: &str) -> Result<Option<String>, String> {
    let Some(stored) = load_raw_from_service(service, user)? else {
        return Ok(None);
    };
    if !stored.starts_with(MANIFEST_PREFIX) {
        let value = stored.trim().to_string();
        return Ok((!value.is_empty()).then_some(value));
    }

    let manifest = decode_manifest(&stored)?;
    let mut value = String::new();
    for index in 0..manifest.count {
        let chunk_user = chunk_user(user, &manifest.generation, index);
        let Some(chunk) = load_raw_from_service(service, &chunk_user)? else {
            return Err("Защищённый секрет повреждён: отсутствует фрагмент".to_string());
        };
        value.push_str(&chunk);
    }
    if sha256_hex(value.as_bytes()) != manifest.sha256 {
        return Err("Защищённый секрет повреждён: контрольная сумма не совпадает".to_string());
    }
    Ok(Some(value))
}

fn delete_secret_from_service(service: &str, user: &str) -> Result<(), String> {
    let manifest = load_raw_from_service(service, user)?
        .as_deref()
        .and_then(|stored| decode_manifest(stored).ok());
    delete_from_service(service, user)?;
    if let Some(manifest) = manifest {
        delete_manifest_chunks_result(service, user, &manifest)?;
    }
    Ok(())
}

#[cfg(not(test))]
fn set_password(service: &str, user: &str, value: &str) -> Result<(), String> {
    keyring_entry_for_service(service, user)
        .set_password(value)
        .map_err(|err| format!("Не удалось сохранить секрет в хранилище ОС: {err}"))
}

#[cfg(test)]
fn set_password(service: &str, user: &str, value: &str) -> Result<(), String> {
    test_keyring()?.insert((service.to_string(), user.to_string()), value.to_string());
    Ok(())
}

#[cfg(not(test))]
fn load_raw_from_service(service: &str, user: &str) -> Result<Option<String>, String> {
    match keyring_entry_for_service(service, user).get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!(
            "Не удалось прочитать секрет из хранилища ОС: {error}"
        )),
    }
}

#[cfg(test)]
fn load_raw_from_service(service: &str, user: &str) -> Result<Option<String>, String> {
    Ok(test_keyring()?
        .get(&(service.to_string(), user.to_string()))
        .cloned())
}

#[cfg(not(test))]
fn delete_from_service(service: &str, user: &str) -> Result<(), String> {
    match keyring_entry_for_service(service, user).delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!(
            "Не удалось удалить секрет из хранилища ОС: {error}"
        )),
    }
}

#[cfg(test)]
fn delete_from_service(service: &str, user: &str) -> Result<(), String> {
    test_keyring()?.remove(&(service.to_string(), user.to_string()));
    Ok(())
}

#[cfg(test)]
fn test_keyring() -> Result<MutexGuard<'static, TestKeyring>, String> {
    static TEST_KEYRING: OnceLock<Mutex<TestKeyring>> = OnceLock::new();
    TEST_KEYRING
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "test keyring lock is unavailable".to_string())
}

fn split_secret(value: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut units = 0;
    for character in value.chars() {
        let character_units = character.len_utf16();
        if units + character_units > CHUNK_UTF16_UNITS && !chunk.is_empty() {
            chunks.push(std::mem::take(&mut chunk));
            units = 0;
        }
        chunk.push(character);
        units += character_units;
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}

fn encode_manifest(manifest: &SecretManifest) -> Result<String, String> {
    let json = serde_json::to_string(manifest)
        .map_err(|_| "Не удалось подготовить манифест защищённого секрета".to_string())?;
    Ok(format!("{MANIFEST_PREFIX}{json}"))
}

fn decode_manifest(value: &str) -> Result<SecretManifest, String> {
    let json = value
        .strip_prefix(MANIFEST_PREFIX)
        .ok_or_else(|| "Некорректный манифест защищённого секрета".to_string())?;
    let manifest: SecretManifest = serde_json::from_str(json)
        .map_err(|_| "Некорректный манифест защищённого секрета".to_string())?;
    let valid_generation = manifest.generation.len() == 32
        && manifest
            .generation
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit());
    let valid_hash =
        manifest.sha256.len() == 64 && manifest.sha256.bytes().all(|byte| byte.is_ascii_hexdigit());
    if manifest.version != MANIFEST_VERSION
        || manifest.count == 0
        || manifest.count > MAX_CHUNKS
        || !valid_generation
        || !valid_hash
    {
        return Err("Некорректный манифест защищённого секрета".to_string());
    }
    Ok(manifest)
}

fn chunk_user(user: &str, generation: &str, index: usize) -> String {
    format!("{user}:chunk:{generation}:{index}")
}

fn delete_manifest_chunks(service: &str, user: &str, manifest: &SecretManifest) {
    let _ = delete_manifest_chunks_result(service, user, manifest);
}

fn delete_manifest_chunks_result(
    service: &str,
    user: &str,
    manifest: &SecretManifest,
) -> Result<(), String> {
    let mut first_error = None;
    for index in 0..manifest.count {
        if let Err(error) =
            delete_from_service(service, &chunk_user(user, &manifest.generation, index))
        {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn sha256_hex(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_respect_utf16_limit_and_preserve_unicode() {
        let value = format!("{}{}{}", "a".repeat(1023), "😀", "b".repeat(2048));
        let chunks = split_secret(&value);

        assert_eq!(chunks.concat(), value);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.encode_utf16().count() <= CHUNK_UTF16_UNITS));
    }

    #[test]
    fn manifest_round_trips_and_rejects_invalid_metadata() {
        let manifest = SecretManifest {
            version: MANIFEST_VERSION,
            generation: "0123456789abcdef0123456789abcdef".to_string(),
            count: 5,
            sha256: sha256_hex(b"secret"),
        };
        let encoded = encode_manifest(&manifest).unwrap();
        let decoded = decode_manifest(&encoded).unwrap();

        assert_eq!(decoded.generation, manifest.generation);
        assert_eq!(decoded.count, manifest.count);
        assert_eq!(decoded.sha256, manifest.sha256);

        let invalid = encoded.replace("\"count\":5", "\"count\":0");
        assert!(decode_manifest(&invalid).is_err());
    }
}
