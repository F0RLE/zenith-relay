use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetProvider {
    Openai,
    ZenithRelayLocal,
    CodexLocalAccess,
}

impl TargetProvider {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::ZenithRelayLocal => "zenith_relay_local",
            Self::CodexLocalAccess => "codex_local_access",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairPreview {
    pub session_id: String,
    pub target_provider: String,
    pub profile_count: usize,
    pub rollout_file_count: usize,
    pub rollout_record_count: usize,
    pub sqlite_row_count: usize,
    pub codex_running: bool,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairResult {
    pub backup_id: String,
    pub backup_path: String,
    pub rollout_records_changed: usize,
    pub sqlite_rows_changed: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackResult {
    pub backup_id: String,
    pub files_restored: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RepairSnapshot {
    pub(super) version: u32,
    pub(super) session_id: String,
    pub(super) target_provider: String,
    pub(super) profile_roots: Vec<String>,
    pub(super) rollout_files: Vec<RolloutSnapshot>,
    #[serde(default)]
    pub(super) history_rollouts: Vec<RolloutSnapshot>,
    pub(super) databases: Vec<DatabaseSnapshot>,
    pub(super) created_at_ms: u64,
    pub(super) expires_at_ms: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RolloutSnapshot {
    pub(super) path: String,
    pub(super) hash: String,
    pub(super) records: usize,
    #[serde(default)]
    pub(super) session_ids: Vec<String>,
    #[serde(default)]
    pub(super) session_meta_count: usize,
}

#[derive(Default)]
pub(super) struct RolloutCollection {
    pub(super) rewrites: Vec<RolloutSnapshot>,
    pub(super) history: Vec<RolloutSnapshot>,
}

#[derive(Clone)]
pub(super) struct SessionMetadata {
    pub(super) records: Vec<SessionMeta>,
}

#[derive(Clone)]
pub(super) struct SessionMeta {
    pub(super) start: u64,
    pub(super) end: u64,
    pub(super) separator: Vec<u8>,
    pub(super) value: Value,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DatabaseSnapshot {
    pub(super) path: String,
    pub(super) hash: String,
    pub(super) rows: usize,
    #[serde(default)]
    pub(super) threads: Vec<DatabaseThreadSnapshot>,
    #[serde(default)]
    pub(super) catalog_threads: Vec<DatabaseCatalogThreadSnapshot>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DatabaseThreadSnapshot {
    pub(super) id: String,
    pub(super) rollout_path: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DatabaseCatalogThreadSnapshot {
    pub(super) host_id: String,
    pub(super) thread_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RepairManifest {
    pub(super) version: u32,
    pub(super) backup_id: String,
    pub(super) profile_roots: Vec<String>,
    pub(super) entries: Vec<BackupEntry>,
    #[serde(default)]
    pub(super) created_at_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BackupTimestamp {
    #[serde(default)]
    pub(super) created_at_ms: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BackupEntry {
    pub(super) original_path: String,
    pub(super) backup_path: String,
    pub(super) sqlite: bool,
}
