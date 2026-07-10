use crate::{
    key_storage::{delete_named_secret, load_named_secret, save_named_secret},
    local_pool::error::{ErrorCode, LocalPoolError, Result},
};

const SECRET_PREFIX: &str = "local-pool:";

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
    Ok(load_named_secret(&keyring_user(secret_ref)?))
}

pub fn delete(secret_ref: &str) -> Result<()> {
    delete_named_secret(&keyring_user(secret_ref)?);
    Ok(())
}

fn keyring_user(secret_ref: &str) -> Result<String> {
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
    Ok(format!("{SECRET_PREFIX}{secret_ref}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_refs_cannot_escape_the_keyring_namespace() {
        assert!(keyring_user("source:abc-123").is_ok());
        assert!(keyring_user("../source").is_err());
        assert!(keyring_user("").is_err());
    }
}
