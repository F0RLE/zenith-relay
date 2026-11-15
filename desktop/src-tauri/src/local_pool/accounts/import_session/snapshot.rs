use serde::{Deserialize, Serialize};
use serde_json::Value;

mod io;
mod read;

pub(super) use io::{
    prepared_secret_ref, prepared_snapshot_path, remove_snapshot_file, secret_ref, snapshot_path,
    snapshot_temp_path, validate_session_id, write_snapshot_new,
};
pub(super) use read::{
    parse_stable, preview_value, read_snapshot, selectable_row_count, session_from_parsed,
    validate_preview,
};

use super::{ImportSession, ImportSessionError, ImportSessionErrorCode};

pub(super) const SNAPSHOT_VERSION: u32 = 1;
const MAX_SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SNAPSHOT_DEPTH: usize = 16;
const MAX_SNAPSHOT_NODES: usize = 65_536;
const MAX_SNAPSHOT_STRING_BYTES: usize = 4 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SessionSnapshot {
    pub(super) version: u32,
    pub(super) session_id: String,
    pub(super) created_at_ms: u64,
    pub(super) source_file: Option<String>,
    pub(super) secret_ref: String,
    pub(super) preview: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) final_preview: Option<Value>,
}
