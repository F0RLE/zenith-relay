use crate::{
    files::atomic_write,
    local_pool::{
        error::{ErrorCode, LocalPoolError, Result},
        store::secret_store,
    },
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{Mutex, OnceLock},
};

const PROVIDER_ID: &str = "zenith_relay_local";
const BACKUP_FILE: &str = "opencode-default.json";
const CONFIG_BACKUP_REF: &str = "profile:opencode:default:previous_config";
const AUTH_BACKUP_REF: &str = "profile:opencode:default:previous_auth";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileBackup {
    version: u32,
    config_secret_ref: Option<String>,
    auth_secret_ref: Option<String>,
    managed_config_hash: String,
    managed_auth_hash: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileState {
    pub attached: bool,
    pub backup_available: bool,
    pub changed: bool,
    pub config_path: String,
}

trait SecretBackend {
    fn save(&self, secret_ref: &str, value: &str) -> Result<()>;
    fn load(&self, secret_ref: &str) -> Result<Option<String>>;
    fn delete(&self, secret_ref: &str) -> Result<()>;
}

struct OsSecretBackend;

impl SecretBackend for OsSecretBackend {
    fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
        secret_store::save(secret_ref, value)
    }
    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        secret_store::load(secret_ref)
    }
    fn delete(&self, secret_ref: &str) -> Result<()> {
        secret_store::delete(secret_ref)
    }
}

pub fn attach(
    config_path: &Path,
    auth_path: &Path,
    backup_root: &Path,
    base_url: &str,
    local_key: &str,
    models: &[String],
) -> Result<()> {
    attach_with(
        config_path,
        auth_path,
        backup_root,
        base_url,
        local_key,
        models,
        &OsSecretBackend,
    )
}

pub fn restore(config_path: &Path, auth_path: &Path, backup_root: &Path) -> Result<()> {
    restore_with(config_path, auth_path, backup_root, &OsSecretBackend)
}

pub fn state(config_path: &Path, auth_path: &Path, backup_root: &Path) -> Result<ProfileState> {
    let backup_path = backup_root.join(BACKUP_FILE);
    let backup = parse_backup(&read_optional(&backup_path)?, &backup_path)?;
    let Some(backup) = backup else {
        return Ok(ProfileState {
            attached: false,
            backup_available: false,
            changed: false,
            config_path: config_path.to_string_lossy().into_owned(),
        });
    };
    let changed = hash_snapshot(&read_optional(config_path)?) != backup.managed_config_hash
        || hash_snapshot(&read_optional(auth_path)?) != backup.managed_auth_hash;
    Ok(ProfileState {
        attached: !changed,
        backup_available: true,
        changed,
        config_path: config_path.to_string_lossy().into_owned(),
    })
}

fn attach_with(
    config_path: &Path,
    auth_path: &Path,
    backup_root: &Path,
    base_url: &str,
    local_key: &str,
    models: &[String],
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _guard = profile_lock()
        .lock()
        .map_err(|_| io_error("OpenCode profile lock poisoned"))?;
    let base_url = base_url.trim_end_matches('/');
    if local_key.trim().is_empty()
        || url::Url::parse(base_url)
            .ok()
            .is_none_or(|url| !matches!(url.scheme(), "http" | "https"))
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "OpenCode endpoint or key is invalid",
        ));
    }
    let model_map = models
        .iter()
        .filter(|model| valid_model(model))
        .take(2_048)
        .map(|model| (model.clone(), json!({ "name": model })))
        .collect::<BTreeMap<_, _>>();
    if model_map.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "OpenCode profile requires at least one visible model",
        ));
    }
    fs::create_dir_all(backup_root).map_err(|error| io_error(error.to_string()))?;
    let backup_path = backup_root.join(BACKUP_FILE);
    let original_config = read_optional(config_path)?;
    let original_auth = read_optional(auth_path)?;
    let original_backup = read_optional(&backup_path)?;
    let existing = parse_backup(&original_backup, &backup_path)?;
    if let Some(backup) = &existing {
        if hash_snapshot(&original_config) != backup.managed_config_hash
            || hash_snapshot(&original_auth) != backup.managed_auth_hash
        {
            return Err(profile_changed());
        }
    }
    let mut config = parse_object(&original_config, config_path)?;
    let mut auth = parse_object(&original_auth, auth_path)?;
    if existing.is_none()
        && (object_contains(&config, "provider", PROVIDER_ID) || auth.contains_key(PROVIDER_ID))
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "managed OpenCode provider exists without a backup",
        ));
    }
    let providers = object_entry(&mut config, "provider")?;
    providers.insert(
        PROVIDER_ID.into(),
        json!({
            "npm": "@ai-sdk/openai-compatible",
            "name": "Zenith Relay Local",
            "options": { "baseURL": base_url },
            "models": model_map,
        }),
    );
    auth.insert(
        PROVIDER_ID.into(),
        json!({ "type": "api", "key": local_key.trim() }),
    );
    let next_config = json_bytes(config)?;
    let next_auth = json_bytes(auth)?;
    let created = existing.is_none();
    let mut backup = existing.unwrap_or(ProfileBackup {
        version: 1,
        config_secret_ref: None,
        auth_secret_ref: None,
        managed_config_hash: String::new(),
        managed_auth_hash: String::new(),
    });
    if created {
        backup.config_secret_ref = save_snapshot(secrets, CONFIG_BACKUP_REF, &original_config)?;
        if let Err(error) = save_snapshot(secrets, AUTH_BACKUP_REF, &original_auth)
            .map(|value| backup.auth_secret_ref = value)
        {
            cleanup_secret(secrets, backup.config_secret_ref.as_deref());
            return Err(error);
        }
    }
    backup.managed_config_hash = hash_bytes(&next_config);
    backup.managed_auth_hash = hash_bytes(&next_auth);
    if let Err(error) = write_snapshot(config_path, Some(&next_config)) {
        cleanup_created(secrets, created, &backup);
        return Err(error);
    }
    if let Err(error) = write_snapshot(auth_path, Some(&next_auth)) {
        let _ = write_snapshot(config_path, original_config.as_deref());
        cleanup_created(secrets, created, &backup);
        return Err(error);
    }
    let backup_bytes = json_bytes(
        serde_json::to_value(&backup)
            .map_err(|error| invalid(error.to_string()))?
            .as_object()
            .cloned()
            .ok_or_else(|| invalid("invalid OpenCode backup"))?,
    )?;
    if let Err(error) = write_snapshot(&backup_path, Some(&backup_bytes)) {
        let _ = write_snapshot(config_path, original_config.as_deref());
        let _ = write_snapshot(auth_path, original_auth.as_deref());
        cleanup_created(secrets, created, &backup);
        return Err(error);
    }
    Ok(())
}

fn restore_with(
    config_path: &Path,
    auth_path: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _guard = profile_lock()
        .lock()
        .map_err(|_| io_error("OpenCode profile lock poisoned"))?;
    let backup_path = backup_root.join(BACKUP_FILE);
    let backup_bytes = read_optional(&backup_path)?.ok_or_else(|| {
        LocalPoolError::new(ErrorCode::NotFound, "OpenCode profile backup was not found")
    })?;
    let backup = parse_backup(&Some(backup_bytes.clone()), &backup_path)?
        .ok_or_else(|| invalid("OpenCode backup is invalid"))?;
    let managed_config = read_optional(config_path)?;
    let managed_auth = read_optional(auth_path)?;
    if hash_snapshot(&managed_config) != backup.managed_config_hash
        || hash_snapshot(&managed_auth) != backup.managed_auth_hash
    {
        return Err(profile_changed());
    }
    let previous_config = load_snapshot(secrets, backup.config_secret_ref.as_deref())?;
    let previous_auth = load_snapshot(secrets, backup.auth_secret_ref.as_deref())?;
    write_snapshot(config_path, previous_config.as_deref())?;
    if let Err(error) = write_snapshot(auth_path, previous_auth.as_deref()) {
        let _ = write_snapshot(config_path, managed_config.as_deref());
        return Err(error);
    }
    if let Err(error) = fs::remove_file(&backup_path) {
        let _ = write_snapshot(config_path, managed_config.as_deref());
        let _ = write_snapshot(auth_path, managed_auth.as_deref());
        return Err(io_error(error.to_string()));
    }
    if let Some(secret_ref) = backup.config_secret_ref.as_deref() {
        secrets.delete(secret_ref)?;
    }
    if let Some(secret_ref) = backup.auth_secret_ref.as_deref() {
        secrets.delete(secret_ref)?;
    }
    Ok(())
}

fn parse_object(snapshot: &Option<Vec<u8>>, path: &Path) -> Result<Map<String, Value>> {
    let Some(bytes) = snapshot else {
        return Ok(Map::new());
    };
    let value: Value = serde_json::from_slice(bytes).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("{} is not valid JSON", path.display()),
        )
    })?;
    value.as_object().cloned().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("{} must contain a JSON object", path.display()),
        )
    })
}

fn object_entry<'a>(
    root: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>> {
    let value = root.entry(key).or_insert_with(|| Value::Object(Map::new()));
    value.as_object_mut().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("OpenCode {key} must be an object"),
        )
    })
}

fn object_contains(root: &Map<String, Value>, key: &str, child: &str) -> bool {
    root.get(key)
        .and_then(Value::as_object)
        .is_some_and(|value| value.contains_key(child))
}
fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 256
        && !model.chars().any(char::is_whitespace)
        && !model.chars().any(char::is_control)
}
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error.to_string())),
    }
}
fn write_snapshot(path: &Path, snapshot: Option<&[u8]>) -> Result<()> {
    match snapshot {
        Some(bytes) => {
            let text =
                std::str::from_utf8(bytes).map_err(|_| invalid("OpenCode profile is not UTF-8"))?;
            atomic_write(path, text).map_err(io_error)
        }
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error.to_string())),
        },
    }
}
fn json_bytes(value: Map<String, Value>) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(value))
        .map_err(|error| invalid(error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn parse_backup(snapshot: &Option<Vec<u8>>, path: &Path) -> Result<Option<ProfileBackup>> {
    snapshot
        .as_ref()
        .map(|bytes| {
            serde_json::from_slice(bytes).map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("{} is invalid", path.display()),
                )
            })
        })
        .transpose()
}
fn save_snapshot(
    secrets: &impl SecretBackend,
    secret_ref: &str,
    snapshot: &Option<Vec<u8>>,
) -> Result<Option<String>> {
    let Some(bytes) = snapshot else {
        return Ok(None);
    };
    secrets.save(secret_ref, &format!("snapshot:{}", STANDARD.encode(bytes)))?;
    Ok(Some(secret_ref.into()))
}
fn load_snapshot(
    secrets: &impl SecretBackend,
    secret_ref: Option<&str>,
) -> Result<Option<Vec<u8>>> {
    let Some(secret_ref) = secret_ref else {
        return Ok(None);
    };
    let value = secrets.load(secret_ref)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "OpenCode backup secret is missing",
        )
    })?;
    let encoded = value
        .strip_prefix("snapshot:")
        .ok_or_else(|| invalid("OpenCode backup secret is invalid"))?;
    STANDARD
        .decode(encoded)
        .map(Some)
        .map_err(|_| invalid("OpenCode backup secret is invalid"))
}
fn cleanup_secret(secrets: &impl SecretBackend, secret_ref: Option<&str>) {
    if let Some(secret_ref) = secret_ref {
        let _ = secrets.delete(secret_ref);
    }
}
fn cleanup_created(secrets: &impl SecretBackend, created: bool, backup: &ProfileBackup) {
    if created {
        cleanup_secret(secrets, backup.config_secret_ref.as_deref());
        cleanup_secret(secrets, backup.auth_secret_ref.as_deref());
    }
}
fn hash_snapshot(snapshot: &Option<Vec<u8>>) -> String {
    snapshot
        .as_deref()
        .map(hash_bytes)
        .unwrap_or_else(|| "absent".into())
}
fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn profile_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
fn profile_changed() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::ProfileRestoreBlocked,
        "OpenCode profile changed after backup",
    )
}
fn invalid(message: impl Into<String>) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, message)
}
fn io_error(message: impl Into<String>) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, String>>);
    impl SecretBackend for MemorySecrets {
        fn save(&self, key: &str, value: &str) -> Result<()> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        fn load(&self, key: &str) -> Result<Option<String>> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        fn delete(&self, key: &str) -> Result<()> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[test]
    fn attach_and_restore_preserve_unrelated_opencode_state() {
        let root = std::env::temp_dir().join(format!("zenith-opencode-{}", uuid::Uuid::new_v4()));
        let config = root.join("config/opencode.json");
        let auth = root.join("data/auth.json");
        let backups = root.join("backups");
        write_snapshot(
            &config,
            Some(br#"{"theme":"system","provider":{"other":{"name":"Other"}}}"#),
        )
        .unwrap();
        write_snapshot(
            &auth,
            Some(br#"{"other":{"type":"api","key":"other-secret"}}"#),
        )
        .unwrap();
        let secrets = MemorySecrets::default();
        attach_with(
            &config,
            &auth,
            &backups,
            "http://127.0.0.1:14998/v1",
            "local-secret",
            &["gpt-test".into()],
            &secrets,
        )
        .unwrap();
        let managed = fs::read_to_string(&config).unwrap();
        assert!(managed.contains("zenith_relay_local"));
        assert!(managed.contains("gpt-test"));
        assert!(!fs::read_to_string(backups.join(BACKUP_FILE))
            .unwrap()
            .contains("other-secret"));
        assert!(state(&config, &auth, &backups).unwrap().attached);
        restore_with(&config, &auth, &backups, &secrets).unwrap();
        assert!(fs::read_to_string(&config)
            .unwrap()
            .contains("\"theme\":\"system\""));
        assert!(fs::read_to_string(&auth).unwrap().contains("other-secret"));
        assert!(!state(&config, &auth, &backups).unwrap().backup_available);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_blocks_external_change() {
        let root = std::env::temp_dir().join(format!("zenith-opencode-{}", uuid::Uuid::new_v4()));
        let config = root.join("config.json");
        let auth = root.join("auth.json");
        let backups = root.join("backups");
        let secrets = MemorySecrets::default();
        attach_with(
            &config,
            &auth,
            &backups,
            "http://127.0.0.1:14998/v1",
            "local-secret",
            &["gpt-test".into()],
            &secrets,
        )
        .unwrap();
        fs::write(&config, "{}\n").unwrap();
        assert!(matches!(
            restore_with(&config, &auth, &backups, &secrets)
                .unwrap_err()
                .code,
            ErrorCode::ProfileRestoreBlocked
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
