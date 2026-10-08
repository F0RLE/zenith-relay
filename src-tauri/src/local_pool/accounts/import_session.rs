use serde::Serialize;
use std::{fmt, fs, path::PathBuf};
use uuid::Uuid;
use zenith_relay_core::accounts::{
    parse_import, ImportError, ImportErrorCode, ImportPreview, ParsedImportItem,
};
use zenith_relay_core::unix_time_ms;

const IMPORT_SESSION_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

mod lifecycle;
mod snapshot;

#[cfg(test)]
mod tests;

use snapshot::{
    parse_stable, prepared_secret_ref, prepared_snapshot_path, preview_value, read_snapshot,
    remove_snapshot_file, secret_ref, selectable_row_count, session_from_parsed, snapshot_path,
    snapshot_temp_path, validate_preview, validate_session_id, write_snapshot_new, SessionSnapshot,
    SNAPSHOT_VERSION,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SecretBackendError;

pub trait SecretBackend {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<(), SecretBackendError>;
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
    pub(super) fn new(code: ImportSessionErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
            session_id: None,
            import_code: None,
        }
    }

    pub(super) fn for_session(mut self, session_id: &str) -> Self {
        self.session_id = Some(session_id.to_string());
        self
    }

    pub(super) fn from_import(error: ImportError) -> Self {
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

fn now_ms() -> u64 {
    unix_time_ms().max(1)
}
