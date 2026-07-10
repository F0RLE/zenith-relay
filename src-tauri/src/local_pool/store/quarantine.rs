use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use chrono::Utc;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn move_file(root: &Path, path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("invalid.json");
    let quarantine_dir = root.join("quarantine");
    fs::create_dir_all(&quarantine_dir).map_err(|err| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to create {}: {err}", quarantine_dir.display()),
        )
    })?;
    let target = quarantine_dir.join(format!("{}-{}", Utc::now().format("%Y%m%d%H%M%S%3f"), name));
    fs::rename(path, &target).map_err(|err| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to quarantine {}: {err}", path.display()),
        )
    })?;
    Ok(target)
}
