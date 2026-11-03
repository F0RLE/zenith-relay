use super::refresh::RefreshRevisions;
use super::source_refresh::SourceRefreshRevisions;
use super::{
    LEGACY_STATE_FILES, MAX_LEGACY_JSON_BYTES, SQLITE_SIDECAR_SUFFIXES, STATE_ACCOUNT_TRASH,
    STATE_ACCOUNTS, STATE_AUTOMATIONS, STATE_GATEWAY, STATE_KEYS, STATE_REFRESH_REVISIONS,
    STATE_SOURCE_REVISIONS, STATE_SOURCES, telemetry_db::TelemetryDb,
};
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use crate::local_pool::models::{
    AutomationRecords, GatewaySettings, LocalAccountRecord, LocalGatewayKeyRecord,
    ProviderSourceRecord, TrashedLocalAccount,
};
use crate::storage_paths::StoragePaths;
use serde::{Serialize, de::DeserializeOwned};
use std::path::Path;

mod layout;

pub(super) use layout::{cleanup_legacy_state_files, migrate_database_file, read_legacy_json};

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
    pub(super) account_trash: Vec<TrashedLocalAccount>,
    pub(super) keys: Vec<LocalGatewayKeyRecord>,
    pub(super) automations: AutomationRecords,
    pub(super) refresh_revisions: Option<RefreshRevisions>,
    pub(super) source_refresh_revisions: Option<SourceRefreshRevisions>,
}

pub(super) fn load_or_initialize_state(
    root: &Path,
    database: &TelemetryDb,
) -> Result<PersistedState> {
    let state_entries = database.state_json_entries()?;
    if state_entries.is_empty() {
        let state = layout::load_legacy_state(root)?.unwrap_or_default();
        persist_state(root, database, &state)?;
        return Ok(state);
    }
    Ok(PersistedState {
        gateway: load_state_from_entries(&state_entries, STATE_GATEWAY)?,
        sources: load_state_from_entries(&state_entries, STATE_SOURCES)?,
        accounts: load_state_from_entries(&state_entries, STATE_ACCOUNTS)?,
        account_trash: load_optional_state_from_entries(&state_entries, STATE_ACCOUNT_TRASH)?
            .unwrap_or_default(),
        keys: load_state_from_entries(&state_entries, STATE_KEYS)?,
        automations: load_state_from_entries(&state_entries, STATE_AUTOMATIONS)?,
        source_refresh_revisions: state_entries
            .contains_key(STATE_SOURCE_REVISIONS)
            .then(|| load_state_from_entries(&state_entries, STATE_SOURCE_REVISIONS))
            .transpose()?,
        refresh_revisions: state_entries
            .contains_key(STATE_REFRESH_REVISIONS)
            .then(|| load_state_from_entries(&state_entries, STATE_REFRESH_REVISIONS))
            .transpose()?,
    })
}

fn persist_state(root: &Path, database: &TelemetryDb, state: &PersistedState) -> Result<()> {
    let mut entries = vec![
        (STATE_ACCOUNT_TRASH, serialize_state(&state.account_trash)?),
        (STATE_GATEWAY, serialize_state(&state.gateway)?),
        (STATE_SOURCES, serialize_state(&state.sources)?),
        (STATE_ACCOUNTS, serialize_state(&state.accounts)?),
        (STATE_KEYS, serialize_state(&state.keys)?),
        (STATE_AUTOMATIONS, serialize_state(&state.automations)?),
    ];
    entries.extend(super::retired_accounts::legacy_entries(root)?);
    database.replace_state_json(&entries)
}

fn load_optional_state_from_entries<T: DeserializeOwned>(
    state_entries: &std::collections::HashMap<String, String>,
    key: &str,
) -> Result<Option<T>> {
    let Some(content) = state_entries.get(key) else {
        return Ok(None);
    };
    serde_json::from_str(content).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("local database state '{key}' is invalid: {error}"),
        )
    })
}

fn load_state_from_entries<T: DeserializeOwned>(
    state_entries: &std::collections::HashMap<String, String>,
    key: &str,
) -> Result<T> {
    load_optional_state_from_entries(state_entries, key)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("local database state '{key}' is missing"),
        )
    })
}

pub(super) fn serialize_state<T: Serialize>(state: &T) -> Result<String> {
    serde_json::to_string(state).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("local state serialization failed: {error}"),
        )
    })
}
