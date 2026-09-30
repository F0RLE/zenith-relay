use super::io::{prepared_secret_ref, prepared_snapshot_path, secret_ref, snapshot_path};
use super::{
    ImportSession, ImportSessionError, ImportSessionErrorCode, SessionSnapshot, MAX_SNAPSHOT_BYTES,
    MAX_SNAPSHOT_DEPTH, MAX_SNAPSHOT_NODES, MAX_SNAPSHOT_STRING_BYTES, SNAPSHOT_VERSION,
};
use serde_json::Value;
use std::{fs, path::Path};
use zenith_relay_core::accounts::{parse_import, ImportPreview, ParsedImport, MAX_IMPORT_ITEMS};

pub(in crate::local_pool::accounts::import_session) fn parse_stable(
    content: &str,
    source_file: Option<&str>,
    existing_identity_keys: &[String],
) -> Result<(ParsedImport, Option<String>), ImportSessionError> {
    let first = parse_import(content, source_file, existing_identity_keys)
        .map_err(ImportSessionError::from_import)?;
    let stable_source_file = first
        .preview
        .rows
        .iter()
        .find_map(|row| row.source_file.clone());
    if stable_source_file.as_deref() == source_file || source_file.is_none() {
        Ok((first, stable_source_file))
    } else {
        parse_import(
            content,
            stable_source_file.as_deref(),
            existing_identity_keys,
        )
        .map(|parsed| (parsed, stable_source_file))
        .map_err(ImportSessionError::from_import)
    }
}

pub(in crate::local_pool::accounts::import_session) fn session_from_parsed(
    session_id: String,
    created_at_ms: u64,
    parsed: ParsedImport,
    prepared: bool,
) -> ImportSession {
    ImportSession {
        session_id,
        created_at_ms,
        prepared,
        preview: parsed.preview,
        items: parsed.items,
    }
}

pub(in crate::local_pool::accounts::import_session) fn preview_value(
    preview: &ImportPreview,
) -> Result<Value, ImportSessionError> {
    serde_json::to_value(preview).map_err(|_| {
        ImportSessionError::new(
            ImportSessionErrorCode::SnapshotInvalid,
            "failed to serialize import preview",
        )
    })
}

pub(in crate::local_pool::accounts::import_session) fn selectable_row_count(
    preview: &ImportPreview,
) -> usize {
    preview.rows.iter().filter(|row| row.selectable).count()
}

pub(in crate::local_pool::accounts::import_session) fn read_snapshot(
    root: &Path,
    session_id: &str,
    prepared: bool,
) -> Result<SessionSnapshot, ImportSessionError> {
    let path = if prepared {
        prepared_snapshot_path(root, session_id)?
    } else {
        snapshot_path(root, session_id)?
    };
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ImportSessionError::new(
                ImportSessionErrorCode::SessionNotFound,
                "import session was not found",
            )
            .for_session(session_id));
        }
        Err(_) => {
            return Err(ImportSessionError::new(
                ImportSessionErrorCode::SnapshotIo,
                "failed to inspect import session snapshot",
            )
            .for_session(session_id));
        }
    };
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_SNAPSHOT_BYTES
    {
        return Err(ImportSessionError::new(
            ImportSessionErrorCode::SnapshotUnsafe,
            "import session snapshot is unsafe",
        )
        .for_session(session_id));
    }
    let bytes = fs::read(path).map_err(|_| {
        ImportSessionError::new(
            ImportSessionErrorCode::SnapshotIo,
            "failed to read import session snapshot",
        )
        .for_session(session_id)
    })?;
    let snapshot: SessionSnapshot = serde_json::from_slice(&bytes).map_err(|_| {
        ImportSessionError::new(
            ImportSessionErrorCode::SnapshotInvalid,
            "import session snapshot is invalid",
        )
        .for_session(session_id)
    })?;
    validate_snapshot(&snapshot, session_id, prepared)?;
    Ok(snapshot)
}

pub(in crate::local_pool::accounts::import_session) fn validate_snapshot(
    snapshot: &SessionSnapshot,
    expected_session_id: &str,
    prepared: bool,
) -> Result<(), ImportSessionError> {
    if snapshot.version != SNAPSHOT_VERSION {
        return Err(ImportSessionError::new(
            ImportSessionErrorCode::UnsupportedSnapshotVersion,
            "import session snapshot version is unsupported",
        )
        .for_session(expected_session_id));
    }
    let original_secret_ref = secret_ref(expected_session_id);
    let valid_secret_ref = if prepared {
        snapshot.secret_ref == prepared_secret_ref(expected_session_id)
            || snapshot.secret_ref == original_secret_ref
    } else {
        snapshot.secret_ref == original_secret_ref
    };
    if snapshot.session_id != expected_session_id
        || !valid_secret_ref
        || snapshot.created_at_ms == 0
        || prepared != snapshot.final_preview.is_some()
    {
        return Err(ImportSessionError::new(
            ImportSessionErrorCode::SnapshotInvalid,
            "import session snapshot metadata is invalid",
        )
        .for_session(expected_session_id));
    }
    if snapshot.source_file.as_deref().is_some_and(|source_file| {
        source_file.is_empty()
            || source_file.len() > 128
            || source_file.contains(['/', '\\'])
            || source_file.chars().any(char::is_control)
    }) {
        return Err(ImportSessionError::new(
            ImportSessionErrorCode::SnapshotUnsafe,
            "import session source metadata is unsafe",
        )
        .for_session(expected_session_id));
    }
    validate_preview(&snapshot.preview).map_err(|error| error.for_session(expected_session_id))?;
    if let Some(final_preview) = snapshot.final_preview.as_ref() {
        validate_preview(final_preview).map_err(|error| error.for_session(expected_session_id))?;
    }
    Ok(())
}

pub(in crate::local_pool::accounts::import_session) fn validate_preview(
    preview: &Value,
) -> Result<(), ImportSessionError> {
    let object = preview.as_object().ok_or_else(|| {
        ImportSessionError::new(
            ImportSessionErrorCode::SnapshotInvalid,
            "import preview snapshot must be an object",
        )
    })?;
    if !object.contains_key("format") || !object.contains_key("rows") {
        return Err(ImportSessionError::new(
            ImportSessionErrorCode::SnapshotInvalid,
            "import preview snapshot is incomplete",
        ));
    }
    let rows = object
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ImportSessionError::new(
                ImportSessionErrorCode::SnapshotInvalid,
                "import preview rows are invalid",
            )
        })?;
    if rows.len() > MAX_IMPORT_ITEMS {
        return Err(ImportSessionError::new(
            ImportSessionErrorCode::SnapshotUnsafe,
            "import preview has too many rows",
        ));
    }

    let mut stack = vec![(preview, 1usize)];
    let mut nodes = 0usize;
    while let Some((value, depth)) = stack.pop() {
        nodes = nodes.saturating_add(1);
        if depth > MAX_SNAPSHOT_DEPTH || nodes > MAX_SNAPSHOT_NODES {
            return Err(ImportSessionError::new(
                ImportSessionErrorCode::SnapshotUnsafe,
                "import preview snapshot exceeds safety limits",
            ));
        }
        match value {
            Value::Object(values) => {
                for (key, value) in values {
                    if sensitive_snapshot_key(key) {
                        return Err(ImportSessionError::new(
                            ImportSessionErrorCode::SnapshotUnsafe,
                            "import preview snapshot contains credential fields",
                        ));
                    }
                    stack.push((value, depth + 1));
                }
            }
            Value::Array(values) => {
                stack.extend(values.iter().map(|value| (value, depth + 1)));
            }
            Value::String(value) if value.len() > MAX_SNAPSHOT_STRING_BYTES => {
                return Err(ImportSessionError::new(
                    ImportSessionErrorCode::SnapshotUnsafe,
                    "import preview snapshot contains an oversized value",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

pub(in crate::local_pool::accounts::import_session) fn sensitive_snapshot_key(key: &str) -> bool {
    let normalized = key
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    matches!(
        normalized.as_slice(),
        b"accesstoken"
            | b"refreshtoken"
            | b"idtoken"
            | b"apikey"
            | b"openaiapikey"
            | b"credentials"
            | b"secret"
            | b"secrets"
            | b"tokens"
    )
}
