use super::refresh::RefreshRevisions;
use super::source_refresh::SourceRefreshRevisions;
use super::{
    telemetry_db::TelemetryDb, LEGACY_STATE_FILES, MAX_LEGACY_JSON_BYTES, SQLITE_SIDECAR_SUFFIXES,
    STATE_ACCOUNTS, STATE_AUTOMATIONS, STATE_GATEWAY, STATE_KEYS, STATE_OWNERSHIP_OPERATION,
    STATE_REFRESH_REVISIONS, STATE_REMOTE_TARGET, STATE_SOURCES, STATE_SOURCE_REVISIONS,
};
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use crate::local_pool::models::{
    AutomationRecords, GatewaySettings, LocalAccountRecord, LocalGatewayKeyRecord,
    OwnershipOperationRecord, ProviderSourceRecord, RemoteTargetRecord,
};
use crate::storage_paths::StoragePaths;
use serde::{de::DeserializeOwned, Serialize};
use std::path::Path;

mod layout;

pub(super) use layout::{cleanup_legacy_state_files, migrate_database_file};

pub(crate) fn migrate_database_layout(app_root: &Path) -> Result<()> {
    let paths = StoragePaths::from_root(app_root);
    let data_root = paths.data_root();
    let database_root = paths.database_root();
    layout::ensure_storage_directory(&data_root)?;
    layout::ensure_storage_directory(&database_root)?;
    migrate_database_file(&data_root, &database_root)
}

#[derive(Clone, Copy)]
pub(super) struct RecordChanges {
    pub(super) sources: bool,
    pub(super) accounts: bool,
    pub(super) keys: bool,
    pub(super) automations: bool,
}

impl RecordChanges {
    pub(super) fn any(self) -> bool {
        self.sources || self.accounts || self.keys || self.automations
    }
}

#[derive(Default)]
pub(super) struct PersistedState {
    pub(super) gateway: GatewaySettings,
    pub(super) sources: Vec<ProviderSourceRecord>,
    pub(super) accounts: Vec<LocalAccountRecord>,
    pub(super) keys: Vec<LocalGatewayKeyRecord>,
    pub(super) automations: AutomationRecords,
    pub(super) remote_target: Option<RemoteTargetRecord>,
    pub(super) ownership_operation: Option<OwnershipOperationRecord>,
    pub(super) refresh_revisions: Option<RefreshRevisions>,
    pub(super) source_refresh_revisions: Option<SourceRefreshRevisions>,
}

pub(super) fn load_or_initialize_state(
    root: &Path,
    database: &TelemetryDb,
) -> Result<PersistedState> {
    let values = database.state_json_values()?;
    if values.is_empty() {
        let state = layout::load_legacy_state(root)?.unwrap_or_default();
        persist_state(database, &state)?;
        return Ok(state);
    }
    Ok(PersistedState {
        gateway: load_state_from_values(&values, STATE_GATEWAY)?,
        sources: load_state_from_values(&values, STATE_SOURCES)?,
        accounts: load_state_from_values(&values, STATE_ACCOUNTS)?,
        keys: load_state_from_values(&values, STATE_KEYS)?,
        automations: load_state_from_values(&values, STATE_AUTOMATIONS)?,
        remote_target: load_optional_state_from_values(&values, STATE_REMOTE_TARGET)?,
        ownership_operation: load_optional_state_from_values(&values, STATE_OWNERSHIP_OPERATION)?,
        source_refresh_revisions: values
            .contains_key(STATE_SOURCE_REVISIONS)
            .then(|| load_state_from_values(&values, STATE_SOURCE_REVISIONS))
            .transpose()?,
        refresh_revisions: values
            .contains_key(STATE_REFRESH_REVISIONS)
            .then(|| load_state_from_values(&values, STATE_REFRESH_REVISIONS))
            .transpose()?,
    })
}

fn persist_state(database: &TelemetryDb, state: &PersistedState) -> Result<()> {
    database.replace_state_json(&[
        (STATE_GATEWAY, serialize_state(&state.gateway)?),
        (STATE_SOURCES, serialize_state(&state.sources)?),
        (STATE_ACCOUNTS, serialize_state(&state.accounts)?),
        (STATE_KEYS, serialize_state(&state.keys)?),
        (STATE_AUTOMATIONS, serialize_state(&state.automations)?),
        (STATE_REMOTE_TARGET, serialize_state(&state.remote_target)?),
        (
            STATE_OWNERSHIP_OPERATION,
            serialize_state(&state.ownership_operation)?,
        ),
    ])
}

fn load_optional_state_from_values<T: DeserializeOwned>(
    values: &std::collections::HashMap<String, String>,
    key: &str,
) -> Result<Option<T>> {
    let Some(content) = values.get(key) else {
        return Ok(None);
    };
    serde_json::from_str(content).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("local database state '{key}' is invalid: {error}"),
        )
    })
}

fn load_state_from_values<T: DeserializeOwned>(
    values: &std::collections::HashMap<String, String>,
    key: &str,
) -> Result<T> {
    let content = values.get(key).ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("local database state '{key}' is missing"),
        )
    })?;
    serde_json::from_str(content).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("local database state '{key}' is invalid: {error}"),
        )
    })
}

pub(super) fn serialize_state<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("local state serialization failed: {error}"),
        )
    })
}
