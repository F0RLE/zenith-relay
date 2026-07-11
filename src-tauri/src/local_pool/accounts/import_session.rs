use super::imports::{
    parse_import, ImportError, ImportErrorCode, ImportPreview, ParsedImport, ParsedImportItem,
    MAX_IMPORT_ITEMS,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const SNAPSHOT_VERSION: u32 = 1;
const MAX_SNAPSHOT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SNAPSHOT_DEPTH: usize = 16;
const MAX_SNAPSHOT_NODES: usize = 16_384;
const MAX_SNAPSHOT_STRING_BYTES: usize = 4 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SecretBackendError;

pub trait SecretBackend {
    fn save(&self, secret_ref: &str, value: &str) -> Result<(), SecretBackendError>;
    fn load(&self, secret_ref: &str) -> Result<Option<String>, SecretBackendError>;
    fn delete(&self, secret_ref: &str) -> Result<(), SecretBackendError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportSessionErrorCode {
    CleanupIncomplete,
    ImportInvalid,
    InvalidSessionId,
    RecoveryRequired,
    SecretMissing,
    SecretStoreUnavailable,
    SessionCollision,
    SessionNotFound,
    SnapshotInvalid,
    SnapshotIo,
    SnapshotMismatch,
    SnapshotUnsafe,
    UnsupportedSnapshotVersion,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSessionError {
    pub code: ImportSessionErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import_code: Option<ImportErrorCode>,
}

impl ImportSessionError {
    fn new(code: ImportSessionErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
            session_id: None,
            import_code: None,
        }
    }

    fn for_session(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }

    fn from_import(error: ImportError) -> Self {
        Self {
            code: ImportSessionErrorCode::ImportInvalid,
            message: "import content is invalid".to_string(),
            session_id: None,
            import_code: Some(error.code),
        }
    }
}

impl fmt::Display for ImportSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ImportSessionError {}

pub struct ImportSession {
    pub session_id: String,
    pub created_at_ms: u64,
    pub prepared: bool,
    pub preview: ImportPreview,
    pub items: Vec<ParsedImportItem>,
}

impl fmt::Debug for ImportSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImportSession")
            .field("session_id", &self.session_id)
            .field("created_at_ms", &self.created_at_ms)
            .field("prepared", &self.prepared)
            .field("preview", &self.preview)
            .field("item_count", &self.items.len())
            .finish()
    }
}

pub struct ImportSessionStore<B> {
    root: PathBuf,
    secrets: B,
}

impl<B: SecretBackend> ImportSessionStore<B> {
    pub fn new(root: PathBuf, secrets: B) -> Self {
        Self { root, secrets }
    }

    pub fn start(
        &self,
        content: &str,
        source_file: Option<&str>,
        existing_identity_keys: &[String],
    ) -> Result<ImportSession, ImportSessionError> {
        let session_id = Uuid::new_v4().hyphenated().to_string();
        self.start_with_id(&session_id, content, source_file, existing_identity_keys)
    }

    pub fn resume(
        &self,
        session_id: &str,
        existing_identity_keys: &[String],
    ) -> Result<ImportSession, ImportSessionError> {
        let session_id = validate_session_id(session_id)?;
        let snapshot = match read_snapshot(&self.root, &session_id, true) {
            Ok(snapshot) => snapshot,
            Err(error) if error.code == ImportSessionErrorCode::SessionNotFound => {
                read_snapshot(&self.root, &session_id, false)?
            }
            Err(error) => return Err(error),
        };
        let content = self
            .secrets
            .load(&snapshot.secret_ref)
            .map_err(|_| {
                ImportSessionError::new(
                    ImportSessionErrorCode::SecretStoreUnavailable,
                    "import session secret is unavailable",
                )
                .for_session(&session_id)
            })?
            .ok_or_else(|| {
                ImportSessionError::new(
                    ImportSessionErrorCode::SecretMissing,
                    "import session secret is missing",
                )
                .for_session(&session_id)
            })?;
        let (base, stable_source_file) =
            parse_stable(&content, snapshot.source_file.as_deref(), &[])?;
        let reparsed_preview = preview_value(&base.preview)?;
        if reparsed_preview != snapshot.preview {
            return Err(ImportSessionError::new(
                ImportSessionErrorCode::SnapshotMismatch,
                "import session snapshot does not match its secret",
            )
            .for_session(&session_id));
        }
        let mut parsed = if existing_identity_keys.is_empty() {
            base
        } else {
            parse_import(
                &content,
                stable_source_file.as_deref(),
                existing_identity_keys,
            )
            .map_err(ImportSessionError::from_import)?
        };
        let prepared = snapshot.final_preview.is_some();
        if let Some(final_preview) = snapshot.final_preview {
            let final_preview: ImportPreview =
                serde_json::from_value(final_preview).map_err(|_| {
                    ImportSessionError::new(
                        ImportSessionErrorCode::SnapshotInvalid,
                        "prepared import preview is invalid",
                    )
                    .for_session(&session_id)
                })?;
            if selectable_row_count(&final_preview) != parsed.items.len() {
                return Err(ImportSessionError::new(
                    ImportSessionErrorCode::SnapshotMismatch,
                    "prepared import preview does not match its secret",
                )
                .for_session(&session_id));
            }
            for (item, row) in parsed
                .items
                .iter_mut()
                .zip(final_preview.rows.iter().filter(|row| row.selectable))
            {
                item.item_id = row.item_id.clone();
            }
            parsed.preview = final_preview;
        }
        Ok(session_from_parsed(
            session_id,
            snapshot.created_at_ms,
            parsed,
            prepared,
        ))
    }

    pub fn prepare(
        &self,
        session_id: &str,
        content: Option<&str>,
        final_preview: ImportPreview,
        existing_identity_keys: &[String],
    ) -> Result<ImportSession, ImportSessionError> {
        let session_id = validate_session_id(session_id)?;
        let original = read_snapshot(&self.root, &session_id, false)?;
        self.clear_prepared(&session_id)?;
        let original_content = if content.is_none() {
            Some(
                self.secrets
                    .load(&original.secret_ref)
                    .map_err(|_| {
                        ImportSessionError::new(
                            ImportSessionErrorCode::SecretStoreUnavailable,
                            "import session secret is unavailable",
                        )
                        .for_session(&session_id)
                    })?
                    .ok_or_else(|| {
                        ImportSessionError::new(
                            ImportSessionErrorCode::SecretMissing,
                            "import session secret is missing",
                        )
                        .for_session(&session_id)
                    })?,
            )
        } else {
            None
        };
        let content = content
            .or(original_content.as_deref())
            .expect("prepared import content is available");
        let (base, stable_source_file) =
            parse_stable(content, original.source_file.as_deref(), &[])?;
        if base.items.len() != selectable_row_count(&final_preview) {
            return Err(ImportSessionError::new(
                ImportSessionErrorCode::SnapshotMismatch,
                "prepared import preview does not match prepared credentials",
            )
            .for_session(&session_id));
        }
        let preview = preview_value(&base.preview)?;
        let final_preview = preview_value(&final_preview)?;
        validate_preview(&final_preview)?;
        let stores_prepared_secret = original_content.is_none();
        let secret_ref = if stores_prepared_secret {
            prepared_secret_ref(&session_id)
        } else {
            original.secret_ref.clone()
        };
        let snapshot = SessionSnapshot {
            version: SNAPSHOT_VERSION,
            session_id: session_id.clone(),
            created_at_ms: original.created_at_ms,
            source_file: stable_source_file,
            secret_ref: secret_ref.clone(),
            preview,
            final_preview: Some(final_preview),
        };
        if stores_prepared_secret {
            self.secrets.save(&secret_ref, content).map_err(|_| {
                ImportSessionError::new(
                    ImportSessionErrorCode::SecretStoreUnavailable,
                    "failed to save prepared import credentials",
                )
                .for_session(&session_id)
            })?;
        }
        let path = prepared_snapshot_path(&self.root, &session_id)?;
        if let Err(error) = write_snapshot_new(&path, &snapshot) {
            if stores_prepared_secret && self.secrets.delete(&secret_ref).is_err() {
                return Err(ImportSessionError::new(
                    ImportSessionErrorCode::RecoveryRequired,
                    "failed to roll back prepared import credentials",
                )
                .for_session(&session_id));
            }
            return Err(error.for_session(&session_id));
        }
        self.resume(&session_id, existing_identity_keys)
    }

    pub fn cancel(&self, session_id: &str) -> Result<(), ImportSessionError> {
        self.clear(session_id)
    }

    pub fn complete(&self, session_id: &str) -> Result<(), ImportSessionError> {
        self.clear(session_id)
    }

    fn start_with_id(
        &self,
        session_id: &str,
        content: &str,
        source_file: Option<&str>,
        existing_identity_keys: &[String],
    ) -> Result<ImportSession, ImportSessionError> {
        let session_id = validate_session_id(session_id)?;
        let (base, stable_source_file) = parse_stable(content, source_file, &[])?;
        let preview = preview_value(&base.preview)?;
        validate_preview(&preview)?;
        let created_at_ms = now_ms();
        let secret_ref = secret_ref(&session_id);
        let snapshot = SessionSnapshot {
            version: SNAPSHOT_VERSION,
            session_id: session_id.clone(),
            created_at_ms,
            source_file: stable_source_file.clone(),
            secret_ref: secret_ref.clone(),
            preview,
            final_preview: None,
        };
        let parsed = if existing_identity_keys.is_empty() {
            base
        } else {
            parse_import(
                content,
                stable_source_file.as_deref(),
                existing_identity_keys,
            )
            .map_err(ImportSessionError::from_import)?
        };
        let path = snapshot_path(&self.root, &session_id)?;
        if path.exists() {
            return Err(ImportSessionError::new(
                ImportSessionErrorCode::SessionCollision,
                "import session already exists",
            )
            .for_session(&session_id));
        }
        if self
            .secrets
            .load(&secret_ref)
            .map_err(|_| {
                ImportSessionError::new(
                    ImportSessionErrorCode::SecretStoreUnavailable,
                    "import session secret store is unavailable",
                )
                .for_session(&session_id)
            })?
            .is_some()
        {
            return Err(ImportSessionError::new(
                ImportSessionErrorCode::SessionCollision,
                "import session already exists",
            )
            .for_session(&session_id));
        }
        self.secrets.save(&secret_ref, content).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::SecretStoreUnavailable,
                "failed to save import session secret",
            )
            .for_session(&session_id)
        })?;
        if let Err(error) = write_snapshot_new(&path, &snapshot) {
            if self.secrets.delete(&secret_ref).is_err() {
                return Err(ImportSessionError::new(
                    ImportSessionErrorCode::RecoveryRequired,
                    "failed to roll back import session secret",
                )
                .for_session(&session_id));
            }
            return Err(error.for_session(&session_id));
        }

        Ok(session_from_parsed(
            session_id,
            created_at_ms,
            parsed,
            false,
        ))
    }

    fn clear(&self, session_id: &str) -> Result<(), ImportSessionError> {
        let session_id = validate_session_id(session_id)?;
        self.clear_prepared(&session_id)?;
        let secret_ref = secret_ref(&session_id);
        self.secrets.delete(&secret_ref).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::SecretStoreUnavailable,
                "failed to delete import session secret",
            )
            .for_session(&session_id)
        })?;
        let path = snapshot_path(&self.root, &session_id)?;
        remove_snapshot_file(&path).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::CleanupIncomplete,
                "import session secret was cleared but snapshot cleanup is incomplete",
            )
            .for_session(&session_id)
        })?;
        let temp = snapshot_temp_path(&path);
        remove_snapshot_file(&temp).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::CleanupIncomplete,
                "import session secret was cleared but temporary snapshot cleanup is incomplete",
            )
            .for_session(&session_id)
        })?;
        Ok(())
    }

    fn clear_prepared(&self, session_id: &str) -> Result<(), ImportSessionError> {
        let secret_ref = prepared_secret_ref(session_id);
        self.secrets.delete(&secret_ref).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::SecretStoreUnavailable,
                "failed to delete prepared import credentials",
            )
            .for_session(session_id)
        })?;
        let path = prepared_snapshot_path(&self.root, session_id)?;
        remove_snapshot_file(&path).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::CleanupIncomplete,
                "prepared import credentials were cleared but snapshot cleanup is incomplete",
            )
            .for_session(session_id)
        })?;
        remove_snapshot_file(&snapshot_temp_path(&path)).map_err(|_| {
            ImportSessionError::new(
                ImportSessionErrorCode::CleanupIncomplete,
                "prepared import temporary snapshot cleanup is incomplete",
            )
            .for_session(session_id)
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionSnapshot {
    version: u32,
    session_id: String,
    created_at_ms: u64,
    source_file: Option<String>,
    secret_ref: String,
    preview: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    final_preview: Option<Value>,
}

fn parse_stable(
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

fn session_from_parsed(
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

fn preview_value(preview: &ImportPreview) -> Result<Value, ImportSessionError> {
    serde_json::to_value(preview).map_err(|_| {
        ImportSessionError::new(
            ImportSessionErrorCode::SnapshotInvalid,
            "failed to serialize import preview",
        )
    })
}

fn selectable_row_count(preview: &ImportPreview) -> usize {
    preview.rows.iter().filter(|row| row.selectable).count()
}

fn read_snapshot(
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

fn validate_snapshot(
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

fn validate_preview(preview: &Value) -> Result<(), ImportSessionError> {
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

fn sensitive_snapshot_key(key: &str) -> bool {
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

fn write_snapshot_new(path: &Path, snapshot: &SessionSnapshot) -> Result<(), ImportSessionError> {
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

fn ensure_import_dir(path: &Path) -> Result<(), ImportSessionError> {
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

fn remove_snapshot_file(path: &Path) -> Result<(), ()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(|_| ())
        }
        Ok(_) => Err(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(()),
    }
}

fn snapshot_path(root: &Path, session_id: &str) -> Result<PathBuf, ImportSessionError> {
    let session_id = validate_session_id(session_id)?;
    Ok(root.join("imports").join(format!("{session_id}.json")))
}

fn prepared_snapshot_path(root: &Path, session_id: &str) -> Result<PathBuf, ImportSessionError> {
    let session_id = validate_session_id(session_id)?;
    Ok(root
        .join("imports")
        .join(format!("{session_id}.prepared.json")))
}

fn snapshot_temp_path(path: &Path) -> PathBuf {
    path.with_extension("tmp")
}

fn secret_ref(session_id: &str) -> String {
    format!("import-session:{session_id}")
}

fn prepared_secret_ref(session_id: &str) -> String {
    format!("import-session-prepared:{session_id}")
}

fn validate_session_id(session_id: &str) -> Result<String, ImportSessionError> {
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(1)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex, MutexGuard},
    };

    const API_KEY: &str = "synthetic-session-api-key";
    const ACCESS_TOKEN: &str = "synthetic-access-token";
    const EMAIL: &str = "session.user@example.test";
    const FIXED_ID: &str = "11111111-2222-4333-8444-555555555555";

    #[derive(Clone, Default)]
    struct MemorySecrets(Arc<Mutex<MemorySecretState>>);

    #[derive(Default)]
    struct MemorySecretState {
        values: HashMap<String, String>,
        fail_save: bool,
        fail_load: bool,
        fail_delete: bool,
    }

    impl MemorySecrets {
        fn state(&self) -> MutexGuard<'_, MemorySecretState> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        fn contains(&self, secret_ref: &str) -> bool {
            self.state().values.contains_key(secret_ref)
        }
    }

    impl SecretBackend for MemorySecrets {
        fn save(&self, secret_ref: &str, value: &str) -> Result<(), SecretBackendError> {
            let mut state = self.state();
            if state.fail_save {
                return Err(SecretBackendError);
            }
            state
                .values
                .insert(secret_ref.to_string(), value.to_string());
            Ok(())
        }

        fn load(&self, secret_ref: &str) -> Result<Option<String>, SecretBackendError> {
            let state = self.state();
            if state.fail_load {
                return Err(SecretBackendError);
            }
            Ok(state.values.get(secret_ref).cloned())
        }

        fn delete(&self, secret_ref: &str) -> Result<(), SecretBackendError> {
            let mut state = self.state();
            if state.fail_delete {
                return Err(SecretBackendError);
            }
            state.values.remove(secret_ref);
            Ok(())
        }
    }

    #[test]
    fn restart_resume_reparses_secret_and_updates_existing_state() {
        let root = temp_root("resume");
        let secrets = MemorySecrets::default();
        let first_store = ImportSessionStore::new(root.clone(), secrets.clone());
        let started = first_store
            .start(&fixture(), Some("session.user@example.test.json"), &[])
            .unwrap();
        let identity_key = started.items[0].identity_key.clone();
        let session_id = started.session_id.clone();
        drop(first_store);

        let reopened = ImportSessionStore::new(root.clone(), secrets.clone());
        let resumed = reopened.resume(&session_id, &[identity_key]).unwrap();
        assert_eq!(resumed.session_id, session_id);
        assert_eq!(resumed.items[0].secrets().api_key(), Some(API_KEY));
        assert!(resumed.preview.rows[0].existing);
        assert!(!resumed.preview.rows[0].default_selected);

        reopened.cancel(&session_id).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_preview_and_exchanged_credentials_survive_restart() {
        let root = temp_root("prepared-resume");
        let secrets = MemorySecrets::default();
        let store = ImportSessionStore::new(root.clone(), secrets.clone());
        let started = store
            .start(r#"{"refresh_token":"refresh-original"}"#, None, &[])
            .unwrap();
        let original_item_id = started.preview.rows[0].item_id.clone();
        let mut final_preview = started.preview.clone();
        final_preview.rows[0].identity = "Account ••••1234".into();
        final_preview.rows[0].quota_status =
            crate::local_pool::accounts::imports::ImportQuotaStatus::Success;
        let prepared = store
            .prepare(
                &started.session_id,
                Some(r#"{"account_id":"provider-account","access_token":"access-exchanged","refresh_token":"refresh-original"}"#),
                final_preview.clone(),
                &[],
            )
            .unwrap();
        assert_eq!(prepared.preview, final_preview);
        assert_eq!(prepared.items[0].item_id, original_item_id);
        assert_eq!(
            prepared.items[0].secrets().access_token(),
            Some("access-exchanged")
        );

        let reopened = ImportSessionStore::new(root.clone(), secrets.clone());
        let resumed = reopened.resume(&started.session_id, &[]).unwrap();
        assert_eq!(resumed.preview, final_preview);
        assert_eq!(resumed.items[0].item_id, original_item_id);
        reopened.cancel(&started.session_id).unwrap();
        assert!(!secrets.contains(&prepared_secret_ref(&started.session_id)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_preview_keeps_rejected_rows_without_recovery_error() {
        let root = temp_root("prepared-invalid-row");
        let secrets = MemorySecrets::default();
        let store = ImportSessionStore::new(root.clone(), secrets.clone());
        let input = r#"[{"email":"same@example.test","access_token":"first"},{"email":"same@example.test","access_token":"second"}]"#;
        let started = store.start(input, None, &[]).unwrap();
        assert_eq!(started.items.len(), 1);
        assert_eq!(started.preview.rows.len(), 2);
        let prepared = store
            .prepare(
                &started.session_id,
                Some(r#"[{"email":"same@example.test","access_token":"first"}]"#),
                started.preview.clone(),
                &[],
            )
            .unwrap();
        assert_eq!(prepared.items.len(), 1);
        assert_eq!(prepared.preview.rows.len(), 2);
        store.cancel(&started.session_id).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_preview_reuses_original_secret_when_credentials_are_unchanged() {
        let root = temp_root("prepared-reused-secret");
        let secrets = MemorySecrets::default();
        let store = ImportSessionStore::new(root.clone(), secrets.clone());
        let started = store
            .start(
                r#"{"email":"same@example.test","access_token":"access"}"#,
                None,
                &[],
            )
            .unwrap();
        let prepared = store
            .prepare(&started.session_id, None, started.preview.clone(), &[])
            .unwrap();
        assert!(prepared.prepared);
        assert!(!secrets.contains(&prepared_secret_ref(&started.session_id)));
        assert!(secrets.contains(&secret_ref(&started.session_id)));
        store.cancel(&started.session_id).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancel_and_complete_clear_secret_before_snapshot() {
        let root = temp_root("clear");
        let secrets = MemorySecrets::default();
        let store = ImportSessionStore::new(root.clone(), secrets.clone());
        let canceled = store.start(&fixture(), None, &[]).unwrap();
        let canceled_ref = secret_ref(&canceled.session_id);
        assert!(secrets.contains(&canceled_ref));
        store.cancel(&canceled.session_id).unwrap();
        assert!(!secrets.contains(&canceled_ref));
        assert!(!snapshot_path(&root, &canceled.session_id).unwrap().exists());

        let completed = store.start(&fixture(), None, &[]).unwrap();
        let completed_ref = secret_ref(&completed.session_id);
        store.complete(&completed.session_id).unwrap();
        assert!(!secrets.contains(&completed_ref));
        assert!(!snapshot_path(&root, &completed.session_id)
            .unwrap()
            .exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_unsafe_and_traversal_snapshots_are_rejected() {
        let root = temp_root("unsafe");
        let secrets = MemorySecrets::default();
        let store = ImportSessionStore::new(root.clone(), secrets.clone());
        let started = store.start(&fixture(), None, &[]).unwrap();
        let path = snapshot_path(&root, &started.session_id).unwrap();
        fs::write(&path, b"{not-json").unwrap();
        assert_eq!(
            store.resume(&started.session_id, &[]).unwrap_err().code,
            ImportSessionErrorCode::SnapshotInvalid
        );
        assert_eq!(
            store.resume("../unsafe", &[]).unwrap_err().code,
            ImportSessionErrorCode::InvalidSessionId
        );
        store.cancel(&started.session_id).unwrap();

        let started = store.start(&fixture(), None, &[]).unwrap();
        let path = snapshot_path(&root, &started.session_id).unwrap();
        let mut snapshot: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        snapshot["preview"]["access_token"] = Value::String(ACCESS_TOKEN.to_string());
        fs::write(&path, serde_json::to_vec_pretty(&snapshot).unwrap()).unwrap();
        assert_eq!(
            store.resume(&started.session_id, &[]).unwrap_err().code,
            ImportSessionErrorCode::SnapshotUnsafe
        );
        store.cancel(&started.session_id).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_failure_rolls_back_secret_and_delete_failure_is_retryable() {
        let root = temp_root("rollback");
        let secrets = MemorySecrets::default();
        let store = ImportSessionStore::new(root.clone(), secrets.clone());
        let imports = root.join("imports");
        fs::create_dir_all(&imports).unwrap();
        fs::create_dir(imports.join("11111111-2222-4333-8444-555555555555.tmp")).unwrap();
        let error = store
            .start_with_id(FIXED_ID, &fixture(), None, &[])
            .unwrap_err();
        assert_eq!(error.code, ImportSessionErrorCode::SnapshotIo);
        assert!(!secrets.contains(&secret_ref(FIXED_ID)));
        fs::remove_dir_all(imports.join("11111111-2222-4333-8444-555555555555.tmp")).unwrap();

        let started = store.start(&fixture(), None, &[]).unwrap();
        secrets.state().fail_delete = true;
        assert_eq!(
            store.cancel(&started.session_id).unwrap_err().code,
            ImportSessionErrorCode::SecretStoreUnavailable
        );
        assert!(snapshot_path(&root, &started.session_id).unwrap().exists());
        secrets.state().fail_delete = false;
        store.cancel(&started.session_id).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn serialized_snapshot_contains_only_redacted_preview_and_reference() {
        let root = temp_root("redacted");
        let secrets = MemorySecrets::default();
        let store = ImportSessionStore::new(root.clone(), secrets);
        let started = store
            .start(&fixture(), Some("session.user@example.test.json"), &[])
            .unwrap();
        let snapshot =
            fs::read_to_string(snapshot_path(&root, &started.session_id).unwrap()).unwrap();
        for forbidden in [
            API_KEY,
            ACCESS_TOKEN,
            EMAIL,
            "OPENAI_API_KEY",
            "access_token",
        ] {
            assert!(!snapshot.contains(forbidden));
        }
        assert!(snapshot.contains("secretRef"));
        assert!(snapshot.contains("preview"));
        store.cancel(&started.session_id).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn secret_save_failure_never_creates_snapshot() {
        let root = temp_root("save-failure");
        let secrets = MemorySecrets::default();
        secrets.state().fail_save = true;
        let store = ImportSessionStore::new(root.clone(), secrets);
        let error = store
            .start_with_id(FIXED_ID, &fixture(), None, &[])
            .unwrap_err();
        assert_eq!(error.code, ImportSessionErrorCode::SecretStoreUnavailable);
        assert!(!snapshot_path(&root, FIXED_ID).unwrap().exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn fixture() -> String {
        format!(
            r#"{{"auth_mode":"apikey","OPENAI_API_KEY":"{API_KEY}","access_note":"{ACCESS_TOKEN}","email":"{EMAIL}"}}"#
        )
    }

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-import-session-{label}-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
