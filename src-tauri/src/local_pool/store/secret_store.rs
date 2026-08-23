use super::vault::Vault;
use crate::{
    key_storage::{delete_named_secret_result, load_named_secret_result, save_named_secret},
    local_pool::error::{ErrorCode, LocalPoolError, Result},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::Rng;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const SECRET_PREFIX: &str = "local-pool:";
const HASHED_SECRET_PREFIX: &str = "lp:";
const VAULT_KEY_USER: &str = "local-vault-master-key-v1";
const LEGACY_MIGRATION_MARKER: &str = ".legacy-keyring-migrated-v1";

struct ConfiguredVault {
    root: PathBuf,
    vault: Vault,
}

static VAULT: OnceLock<ConfiguredVault> = OnceLock::new();
static INITIALIZE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub fn initialize(root: &Path) -> Result<()> {
    let _guard = INITIALIZE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| LocalPoolError::new(ErrorCode::Io, "secret vault lock is unavailable"))?;
    let root = canonical_root(root)?;
    if let Some(configured) = VAULT.get() {
        return if configured.root == root {
            Ok(())
        } else {
            Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "secret vault is already initialized for another data directory",
            ))
        };
    }

    let key = load_or_create_vault_key(&root)?;
    let vault = Vault::open(&root, key).map_err(|message| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("failed to open encrypted secret vault: {message}"),
        )
    })?;
    migrate_legacy_keyring_once(&root, &vault);
    VAULT
        .set(ConfiguredVault { root, vault })
        .map_err(|_| LocalPoolError::new(ErrorCode::Io, "failed to initialize secret vault"))
}

pub fn save(secret_ref: &str, value: &str) -> Result<()> {
    validate_secret_ref(secret_ref)?;
    if value.trim().is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "secret value must not be empty",
        ));
    }
    if let Some(configured) = VAULT.get() {
        configured
            .vault
            .save(secret_ref, value)
            .map_err(vault_error)?;
        let _ = delete_keyring_secret(secret_ref);
        return Ok(());
    }
    save_keyring_secret(secret_ref, value)
}

pub fn load(secret_ref: &str) -> Result<Option<String>> {
    validate_secret_ref(secret_ref)?;
    let Some(configured) = VAULT.get() else {
        return load_keyring_secret(secret_ref);
    };
    if let Some(value) = configured.vault.load(secret_ref).map_err(vault_error)? {
        return Ok(Some(value));
    }
    let Some(value) = load_keyring_secret(secret_ref)? else {
        return Ok(None);
    };
    configured
        .vault
        .save(secret_ref, &value)
        .map_err(vault_error)?;
    let _ = delete_keyring_secret(secret_ref);
    Ok(Some(value))
}

fn migrate_legacy_keyring_once(root: &Path, vault: &Vault) {
    let marker = root.join(LEGACY_MIGRATION_MARKER);
    if marker.exists() {
        return;
    }
    let Ok(secret_refs) = vault.secret_refs() else {
        return;
    };
    let mut complete = true;
    for secret_ref in secret_refs {
        complete &= delete_keyring_secret(&secret_ref).is_ok();
    }
    if !complete {
        return;
    }
    let _ = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker);
}

pub fn delete(secret_ref: &str) -> Result<()> {
    validate_secret_ref(secret_ref)?;
    if let Some(configured) = VAULT.get() {
        configured.vault.delete(secret_ref).map_err(vault_error)?;
    }
    delete_keyring_secret(secret_ref)
}

fn canonical_root(root: &Path) -> Result<PathBuf> {
    fs::create_dir_all(root).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to create local data directory: {error}"),
        )
    })?;
    let metadata = fs::symlink_metadata(root).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to inspect local data directory: {error}"),
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "local data directory is unsafe",
        ));
    }
    fs::canonicalize(root).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to resolve local data directory: {error}"),
        )
    })
}

fn load_or_create_vault_key(vault_root: &Path) -> Result<[u8; 32]> {
    if let Some(encoded) = load_named_secret_result(VAULT_KEY_USER)
        .map_err(|error| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error))?
    {
        return decode_vault_key(&encoded);
    }
    if Vault::has_persisted_data(vault_root) {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "encrypted secret vault exists but its system master key is missing",
        ));
    }
    let mut key = [0_u8; 32];
    rand::rng().fill_bytes(&mut key);
    save_named_secret(VAULT_KEY_USER, &URL_SAFE_NO_PAD.encode(key))
        .map_err(|error| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error))?;
    Ok(key)
}

fn decode_vault_key(value: &str) -> Result<[u8; 32]> {
    let bytes = URL_SAFE_NO_PAD.decode(value.trim()).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "encrypted secret vault master key is invalid",
        )
    })?;
    bytes.try_into().map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "encrypted secret vault master key has an invalid length",
        )
    })
}

fn save_keyring_secret(secret_ref: &str, value: &str) -> Result<()> {
    save_named_secret(&keyring_user(secret_ref)?, value)
        .map_err(|error| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error))
}

fn load_keyring_secret(secret_ref: &str) -> Result<Option<String>> {
    if let Some(value) = load_named_secret_result(&keyring_user(secret_ref)?)
        .map_err(|error| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error))?
    {
        return Ok(Some(value));
    }
    let legacy_user = legacy_keyring_user(secret_ref)?;
    let Some(value) = load_named_secret_result(&legacy_user)
        .map_err(|error| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error))?
    else {
        return Ok(None);
    };
    save_keyring_secret(secret_ref, &value)?;
    delete_named_secret_result(&legacy_user)
        .map_err(|error| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error))?;
    Ok(Some(value))
}

fn delete_keyring_secret(secret_ref: &str) -> Result<()> {
    let current = delete_named_secret_result(&keyring_user(secret_ref)?);
    let legacy = delete_named_secret_result(&legacy_keyring_user(secret_ref)?);
    current
        .and(legacy)
        .map_err(|error| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error))
}

fn vault_error(message: String) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::SecretStoreUnavailable, message)
}

fn keyring_user(secret_ref: &str) -> Result<String> {
    validate_secret_ref(secret_ref)?;
    Ok(format!(
        "{HASHED_SECRET_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(Sha256::digest(secret_ref.as_bytes()))
    ))
}

fn legacy_keyring_user(secret_ref: &str) -> Result<String> {
    validate_secret_ref(secret_ref)?;
    Ok(format!("{SECRET_PREFIX}{secret_ref}"))
}

fn validate_secret_ref(secret_ref: &str) -> Result<()> {
    let valid = !secret_ref.is_empty()
        && secret_ref.len() <= 128
        && secret_ref
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'));
    if !valid {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "secret reference contains unsupported characters",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_refs_cannot_escape_the_storage_namespace() {
        assert!(keyring_user("source:abc-123").is_ok());
        assert!(keyring_user("source:abc-123").unwrap().len() < 64);
        assert!(keyring_user("../source").is_err());
        assert!(keyring_user("").is_err());
    }

    #[test]
    fn production_length_keyring_secret_round_trips_before_vault_initialization() {
        let secret_ref = format!("account:codex:account_{}", uuid::Uuid::new_v4().simple());
        let secret = format!("{{\"tokens\":\"{}\"}}", "synthetic-secret".repeat(384));
        assert!(secret.len() >= 5 * 1024);

        save_keyring_secret(&secret_ref, &secret).unwrap();
        assert_eq!(
            load_keyring_secret(&secret_ref).unwrap().as_deref(),
            Some(secret.as_str())
        );
        delete_keyring_secret(&secret_ref).unwrap();
    }

    #[test]
    fn legacy_readable_key_is_migrated_to_the_short_key() {
        let secret_ref = format!("source:{}", uuid::Uuid::new_v4().simple());
        let legacy_user = legacy_keyring_user(&secret_ref).unwrap();
        save_named_secret(&legacy_user, "legacy-secret").unwrap();

        assert_eq!(
            load_keyring_secret(&secret_ref).unwrap().as_deref(),
            Some("legacy-secret")
        );
        assert!(load_named_secret_result(&legacy_user).unwrap().is_none());
        delete_keyring_secret(&secret_ref).unwrap();
    }
}
