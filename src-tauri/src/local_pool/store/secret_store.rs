use crate::{
    key_storage::{delete_named_secret_result, load_named_secret_result, save_named_secret},
    local_pool::error::{ErrorCode, LocalPoolError, Result},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};

const SECRET_PREFIX: &str = "local-pool:";
const HASHED_SECRET_PREFIX: &str = "lp:";

pub fn save(secret_ref: &str, value: &str) -> Result<()> {
    let user = keyring_user(secret_ref)?;
    if value.trim().is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "secret value must not be empty",
        ));
    }
    save_named_secret(&user, value)
        .map_err(|err| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, err))
}

pub fn load(secret_ref: &str) -> Result<Option<String>> {
    let user = keyring_user(secret_ref)?;
    if let Some(value) = load_named_secret_result(&user)
        .map_err(|err| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, err))?
    {
        return Ok(Some(value));
    }
    let legacy_user = legacy_keyring_user(secret_ref)?;
    let Some(value) = load_named_secret_result(&legacy_user)
        .map_err(|err| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, err))?
    else {
        return Ok(None);
    };
    save_named_secret(&user, &value)
        .map_err(|err| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, err))?;
    delete_named_secret_result(&legacy_user)
        .map_err(|err| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, err))?;
    Ok(Some(value))
}

pub fn delete(secret_ref: &str) -> Result<()> {
    let current = delete_named_secret_result(&keyring_user(secret_ref)?);
    let legacy = delete_named_secret_result(&legacy_keyring_user(secret_ref)?);
    current
        .and(legacy)
        .map_err(|err| LocalPoolError::new(ErrorCode::SecretStoreUnavailable, err))
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
    fn secret_refs_cannot_escape_the_keyring_namespace() {
        assert!(keyring_user("source:abc-123").is_ok());
        assert!(keyring_user("source:abc-123").unwrap().len() < 64);
        assert!(keyring_user("../source").is_err());
        assert!(keyring_user("").is_err());
    }

    #[test]
    fn production_length_account_secret_round_trips() {
        let secret_ref = format!("account:codex:account_{}", uuid::Uuid::new_v4().simple());
        save(&secret_ref, "synthetic-secret").unwrap();
        assert_eq!(
            load(&secret_ref).unwrap().as_deref(),
            Some("synthetic-secret")
        );
        delete(&secret_ref).unwrap();
    }

    #[test]
    fn legacy_readable_key_is_migrated_to_the_short_key() {
        let secret_ref = format!("source:{}", uuid::Uuid::new_v4().simple());
        let legacy_user = legacy_keyring_user(&secret_ref).unwrap();
        save_named_secret(&legacy_user, "legacy-secret").unwrap();

        assert_eq!(load(&secret_ref).unwrap().as_deref(), Some("legacy-secret"));
        assert!(load_named_secret_result(&legacy_user).unwrap().is_none());
        delete(&secret_ref).unwrap();
    }

    #[test]
    fn multiple_account_secrets_are_isolated() {
        let first = format!("account:codex:account_{}", uuid::Uuid::new_v4().simple());
        let second = format!("account:codex:account_{}", uuid::Uuid::new_v4().simple());
        save(&first, "first-secret").unwrap();
        save(&second, "second-secret").unwrap();
        assert_eq!(load(&first).unwrap().as_deref(), Some("first-secret"));
        assert_eq!(load(&second).unwrap().as_deref(), Some("second-secret"));
        delete(&first).unwrap();
        delete(&second).unwrap();
    }
}
