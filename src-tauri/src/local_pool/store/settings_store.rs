use crate::{
    files::atomic_write,
    local_pool::error::{ErrorCode, LocalPoolError, Result},
};
use serde::{de::DeserializeOwned, Serialize};
use std::{fs, path::Path};

pub fn load_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path).map_err(|err| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to read {}: {err}", path.display()),
        )
    })?;
    serde_json::from_str(&content).map(Some).map_err(|err| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("failed to parse {}: {err}", path.display()),
        )
    })
}

pub fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let content = serde_json::to_string_pretty(value).map_err(|err| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("failed to serialize {}: {err}", path.display()),
        )
    })?;
    atomic_write(path, &format!("{content}\n"))
        .map_err(|err| LocalPoolError::new(ErrorCode::Io, err))
}
