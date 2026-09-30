use super::codex::{self, UserProfileSnapshot};
use super::io::{self, SnapshotSecrets};
use super::{
    ProfileSnapshotSummary, MAX_METADATA_BYTES, MAX_NAME_CHARS, MAX_PROFILE_FILE_BYTES,
    PAYLOAD_VERSION, SNAPSHOT_VERSION,
};
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SnapshotRecord {
    pub(super) version: u32,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) profile_dir: String,
    pub(super) created_at_ms: u64,
    pub(super) config_available: bool,
    pub(super) auth_available: bool,
    #[serde(default)]
    pub(super) is_original: bool,
    pub(super) payload_secret_ref: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SnapshotPayload {
    pub(super) version: u32,
    pub(super) config: Option<String>,
    pub(super) auth: Option<String>,
}

pub(super) fn load_payload(
    record: &SnapshotRecord,
    secrets: &impl SnapshotSecrets,
) -> Result<SnapshotPayload> {
    let content = secrets.load(&record.payload_secret_ref)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT snapshot payload is missing",
        )
    })?;
    let payload: SnapshotPayload = serde_json::from_str(&content).map_err(io::invalid_data)?;
    if payload.version != PAYLOAD_VERSION {
        return Err(LocalPoolError::new(
            ErrorCode::UnsupportedSchema,
            "ChatGPT snapshot payload uses an unsupported version",
        ));
    }
    validate_profile_content(&UserProfileSnapshot {
        config: payload.config.clone(),
        auth: payload.auth.clone(),
    })?;
    Ok(payload)
}

pub(super) fn read_record(path: &Path) -> Result<SnapshotRecord> {
    let bytes = io::read_bounded(path, MAX_METADATA_BYTES)?;
    let content = std::str::from_utf8(&bytes).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT snapshot metadata is not UTF-8",
        )
    })?;
    serde_json::from_str(content).map_err(io::invalid_data)
}

pub(super) fn validate_record(record: &SnapshotRecord, expected_id: &str) -> Result<()> {
    let id = io::parse_id(&record.id)?;
    if record.version != SNAPSHOT_VERSION
        || id != expected_id
        || record.name != normalize_name(&record.name)?
        || record.profile_dir.trim().is_empty()
        || !Path::new(&record.profile_dir).is_absolute()
        || record.profile_dir.chars().any(char::is_control)
        || record.created_at_ms == 0
        || record.payload_secret_ref != io::payload_secret_ref(&id)
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT snapshot metadata is invalid",
        ));
    }
    Ok(())
}

pub(super) fn validate_profile_content(snapshot: &UserProfileSnapshot) -> Result<()> {
    if snapshot
        .config
        .as_ref()
        .is_some_and(|value| value.len() > MAX_PROFILE_FILE_BYTES)
        || snapshot
            .auth
            .as_ref()
            .is_some_and(|value| value.len() > MAX_PROFILE_FILE_BYTES)
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT profile snapshot is too large",
        ));
    }
    Ok(())
}

pub(super) fn normalize_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_NAME_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT snapshot name is invalid",
        ));
    }
    Ok(value.to_string())
}

pub(super) fn summary(record: &SnapshotRecord) -> ProfileSnapshotSummary {
    ProfileSnapshotSummary {
        id: record.id.clone(),
        name: record.name.clone(),
        profile_dir: codex::portable_path_value(&record.profile_dir),
        created_at_ms: record.created_at_ms,
        config_available: record.config_available,
        auth_available: record.auth_available,
        is_original: record.is_original,
    }
}
