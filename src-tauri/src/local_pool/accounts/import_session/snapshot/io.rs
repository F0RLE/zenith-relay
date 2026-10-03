use super::{ImportSessionError, ImportSessionErrorCode, SessionSnapshot, MAX_SNAPSHOT_BYTES};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

pub(in crate::local_pool::accounts::import_session) fn write_snapshot_new(
    path: &Path,
    snapshot: &SessionSnapshot,
) -> Result<(), ImportSessionError> {
    let parent = path.parent().ok_or_else(|| {
        ImportSessionError::new(
            ImportSessionErrorCode::SnapshotUnsafe,
            "import session snapshot path is invalid",
        )
    })?;
    ensure_import_dir(parent)?;
    if path.exists() {
        return Err(ImportSessionError::new(
            ImportSessionErrorCode::SessionCollision,
            "import session already exists",
        ));
    }
    let mut bytes = serde_json::to_vec_pretty(snapshot).map_err(|_| {
        ImportSessionError::new(
            ImportSessionErrorCode::SnapshotInvalid,
            "failed to serialize import session snapshot",
        )
    })?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err(ImportSessionError::new(
            ImportSessionErrorCode::SnapshotUnsafe,
            "import session snapshot exceeds the size limit",
        ));
    }
    let temp = snapshot_temp_path(path);
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| {
                ImportSessionError::new(
                    ImportSessionErrorCode::SnapshotIo,
                    "failed to create temporary import session snapshot",
                )
            })?;
        file.write_all(&bytes).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::SnapshotIo,
                "failed to write import session snapshot",
            )
        })?;
        file.sync_all().map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::SnapshotIo,
                "failed to flush import session snapshot",
            )
        })?;
        drop(file);
        fs::rename(&temp, path).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::SnapshotIo,
                "failed to publish import session snapshot",
            )
        })
    })();
    if result.is_err() {
        let _ = remove_snapshot_file(&temp);
    }
    result
}

pub(in crate::local_pool::accounts::import_session) fn ensure_import_dir(
    path: &Path,
) -> Result<(), ImportSessionError> {
    fs::create_dir_all(path).map_err(|_| {
        ImportSessionError::new(
            ImportSessionErrorCode::SnapshotIo,
            "failed to create import session directory",
        )
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        ImportSessionError::new(
            ImportSessionErrorCode::SnapshotIo,
            "failed to inspect import session directory",
        )
    })?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(ImportSessionError::new(
            ImportSessionErrorCode::SnapshotUnsafe,
            "import session directory is unsafe",
        ));
    }
    Ok(())
}

pub(in crate::local_pool::accounts::import_session) fn remove_snapshot_file(
    path: &Path,
) -> Result<(), ()> {
    crate::files::remove_regular_file(path)
}

pub(in crate::local_pool::accounts::import_session) fn snapshot_path(
    root: &Path,
    session_id: &str,
) -> Result<PathBuf, ImportSessionError> {
    let session_id = validate_session_id(session_id)?;
    Ok(root.join("imports").join(format!("{session_id}.json")))
}

pub(in crate::local_pool::accounts::import_session) fn prepared_snapshot_path(
    root: &Path,
    session_id: &str,
) -> Result<PathBuf, ImportSessionError> {
    let session_id = validate_session_id(session_id)?;
    Ok(root
        .join("imports")
        .join(format!("{session_id}.prepared.json")))
}

pub(in crate::local_pool::accounts::import_session) fn snapshot_temp_path(path: &Path) -> PathBuf {
    path.with_extension("tmp")
}

pub(in crate::local_pool::accounts::import_session) fn secret_ref(session_id: &str) -> String {
    format!("import-session:{session_id}")
}

pub(in crate::local_pool::accounts::import_session) fn prepared_secret_ref(
    session_id: &str,
) -> String {
    format!("import-session-prepared:{session_id}")
}

pub(in crate::local_pool::accounts::import_session) fn validate_session_id(
    session_id: &str,
) -> Result<String, ImportSessionError> {
    let session_id = session_id.trim();
    let uuid = Uuid::parse_str(session_id).map_err(|_| {
        ImportSessionError::new(
            ImportSessionErrorCode::InvalidSessionId,
            "import session id is invalid",
        )
    })?;
    let canonical = uuid.hyphenated().to_string();
    if !session_id.eq_ignore_ascii_case(&canonical)
        || !session_id.is_ascii()
        || session_id.len() != canonical.len()
    {
        return Err(ImportSessionError::new(
            ImportSessionErrorCode::InvalidSessionId,
            "import session id is invalid",
        ));
    }
    Ok(canonical)
}
