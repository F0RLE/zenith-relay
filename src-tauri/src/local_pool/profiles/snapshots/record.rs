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
    snapshot_record: &SnapshotRecord,
    secrets: &impl SnapshotSecrets,
) -> Result<SnapshotPayload> {
    let content = secrets
        .load(&snapshot_record.payload_secret_ref)?
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "ChatGPT snapshot payload is missing",
            )
        })?;
    let snapshot_payload: SnapshotPayload =
        serde_json::from_str(&content).map_err(io::invalid_data)?;
    if snapshot_payload.version != PAYLOAD_VERSION {
        return Err(LocalPoolError::new(
            ErrorCode::UnsupportedSchema,
            "ChatGPT snapshot payload uses an unsupported version",
        ));
    }
    validate_profile_content(&UserProfileSnapshot {
        config: snapshot_payload.config.clone(),
        auth: snapshot_payload.auth.clone(),
    })?;
    Ok(snapshot_payload)
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

pub(super) fn validate_record(snapshot_record: &SnapshotRecord, expected_id: &str) -> Result<()> {
    let snapshot_id = io::parse_snapshot_id(&snapshot_record.id)?;
    if snapshot_record.version != SNAPSHOT_VERSION
        || snapshot_id != expected_id
        || snapshot_record.name != normalize_name(&snapshot_record.name)?
        || snapshot_record.profile_dir.trim().is_empty()
        || !Path::new(&snapshot_record.profile_dir).is_absolute()
        || snapshot_record.profile_dir.chars().any(char::is_control)
        || snapshot_record.created_at_ms == 0
        || snapshot_record.payload_secret_ref != io::payload_secret_ref(&snapshot_id)
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
        .is_some_and(|profile_content| profile_content.len() > MAX_PROFILE_FILE_BYTES)
        || snapshot
            .auth
            .as_ref()
            .is_some_and(|profile_content| profile_content.len() > MAX_PROFILE_FILE_BYTES)
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT profile snapshot is too large",
        ));
    }
    Ok(())
}

pub(super) fn normalize_name(snapshot_name: &str) -> Result<String> {
    let snapshot_name = snapshot_name.trim();
    if snapshot_name.is_empty()
        || snapshot_name.chars().count() > MAX_NAME_CHARS
        || snapshot_name.chars().any(char::is_control)
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT snapshot name is invalid",
        ));
    }
    Ok(snapshot_name.to_string())
}

pub(super) fn summary(snapshot_record: &SnapshotRecord) -> ProfileSnapshotSummary {
    ProfileSnapshotSummary {
        id: snapshot_record.id.clone(),
        name: snapshot_record.name.clone(),
        profile_dir: codex::portable_path_value(&snapshot_record.profile_dir),
        created_at_ms: snapshot_record.created_at_ms,
        config_available: snapshot_record.config_available,
        auth_available: snapshot_record.auth_available,
        is_original: snapshot_record.is_original,
    }
}
