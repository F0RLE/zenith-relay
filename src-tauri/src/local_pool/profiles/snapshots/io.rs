use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    store::secret_store,
};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const SNAPSHOT_DIR: &str = "snapshots";

pub(super) fn snapshot_root(backup_root: &Path) -> PathBuf {
    backup_root.join(SNAPSHOT_DIR)
}

pub(super) fn metadata_path(backup_root: &Path, id: &str) -> Result<PathBuf> {
    let id = parse_id(id)?;
    Ok(snapshot_root(backup_root).join(format!("{id}.json")))
}

pub(super) fn parse_id(id: &str) -> Result<String> {
    let parsed = Uuid::parse_str(id.trim()).map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "ChatGPT snapshot ID is invalid")
    })?;
    let normalized = parsed.to_string();
    if normalized != id {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT snapshot ID is invalid",
        ));
    }
    Ok(normalized)
}

pub(super) fn payload_secret_ref(id: &str) -> String {
    format!("profile:snapshot:{id}:payload")
}

pub(super) fn read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > max_bytes {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT snapshot metadata is invalid",
        ));
    }
    fs::read(path).map_err(io_error)
}

pub(super) fn invalid_data(error: impl std::fmt::Display) -> LocalPoolError {
    let _ = error;
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "ChatGPT snapshot data is invalid",
    )
}

pub(super) fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::Io,
        format!("ChatGPT snapshot I/O failed: {error}"),
    )
}

pub(super) fn io_error_message(error: String) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, error)
}

pub(super) fn snapshot_changed() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::ProfileRestoreBlocked,
        "ChatGPT snapshot changed while Relay was updating it",
    )
}

pub(super) fn with_cleanup(error: LocalPoolError, cleanup: Result<()>) -> LocalPoolError {
    match cleanup {
        Ok(()) => error,
        Err(cleanup) => LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; snapshot cleanup failed: {}",
                error.message, cleanup.message
            ),
        ),
    }
}

pub(super) trait SnapshotSecrets {
    fn save(&self, secret_ref: &str, value: &str) -> Result<()>;
    fn load(&self, secret_ref: &str) -> Result<Option<String>>;
    fn delete(&self, secret_ref: &str) -> Result<()>;
}

pub(super) struct OsSnapshotSecrets;

impl SnapshotSecrets for OsSnapshotSecrets {
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
