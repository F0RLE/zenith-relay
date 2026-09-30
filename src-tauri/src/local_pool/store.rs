mod persistence;
mod records;
mod refresh;
mod source_refresh;
pub(crate) use source_refresh::SourceRefreshFence;
use source_refresh::{SourceRefreshRevisions, STATE_SOURCE_REVISIONS};
pub mod secret_store;
pub mod telemetry_db;
pub(crate) mod vault;

use self::telemetry_db::TelemetryDb;
use crate::{
    local_pool::{
        error::{ErrorCode, LocalPoolError, Result},
        models::{
            AutomationRecords, GatewaySettings, LocalAccountRecord, LocalGatewayKeyRecord,
            OwnershipOperationRecord, ProviderSourceRecord, RemoteTargetRecord, MAX_LOCAL_ACCOUNTS,
        },
    },
    storage_paths::StoragePaths,
};
pub(crate) use persistence::migrate_database_layout;
use persistence::serialize_state;
pub(crate) use refresh::{AccountRefreshFence, AppliedAccountRefresh};
use refresh::{RefreshRevisions, STATE_REFRESH_REVISIONS};
use std::{path::PathBuf, sync::Arc};
use zenith_relay_core::automations::WakeExecutionPolicy;
use zenith_relay_core::PoolRoutingPolicy;

const STATE_GATEWAY: &str = "gateway";
const STATE_SOURCES: &str = "sources";
const STATE_ACCOUNTS: &str = "accounts";
const STATE_KEYS: &str = "keys";
const STATE_AUTOMATIONS: &str = "automations";
const STATE_REMOTE_TARGET: &str = "remote_target";
const STATE_OWNERSHIP_OPERATION: &str = "ownership_operation";
const LEGACY_STATE_FILES: [&str; 7] = [
    "metadata.json",
    "settings.json",
    "connections.json",
    "accounts.json",
    "pool-keys.json",
    "automations.json",
    "remote-target.json",
];
const MAX_LEGACY_JSON_BYTES: u64 = 16 * 1024 * 1024;
const SQLITE_SIDECAR_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];

pub struct LocalPoolStore {
    database: Arc<TelemetryDb>,
    gateway: GatewaySettings,
    sources: Vec<ProviderSourceRecord>,
    accounts: Vec<LocalAccountRecord>,
    keys: Vec<LocalGatewayKeyRecord>,
    automations: AutomationRecords,
    remote_target: Option<RemoteTargetRecord>,
    ownership_operation: Option<OwnershipOperationRecord>,
    refresh_revisions: RefreshRevisions,
    source_refresh_revisions: SourceRefreshRevisions,
    refresh_changed: tokio::sync::watch::Sender<u64>,
}

impl LocalPoolStore {
    pub fn open(app_root: PathBuf) -> Result<Self> {
        persistence::migrate_database_layout(&app_root)?;
        let paths = StoragePaths::from_root(&app_root);
        let root = paths.data_root();
        let database = Arc::new(TelemetryDb::open(&paths.database_file())?);
        let state = persistence::load_or_initialize_state(&root, &database)?;
        let mut gateway = state.gateway;
        gateway
            .validate()
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        let mut sources = state.sources;
        for source in &mut sources {
            source.normalize();
            source
                .validate_price_overrides()
                .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message))?;
        }
        let mut accounts = state.accounts;
        upgrade_saved_gateway(&database, &mut gateway, &sources, &accounts)?;

        let mut automations = state.automations;
        let mut automation_policy_changed = false;
        for task in &mut automations.tasks {
            if task.execution_policy == WakeExecutionPolicy::RequireConfirmation {
                task.execution_policy = WakeExecutionPolicy::Automatic;
                automations
                    .state
                    .clear_task_confirmation_requirement(&task.id);
                automation_policy_changed = true;
            }
        }
        if automation_policy_changed {
            database.replace_state_json(&[(
                STATE_AUTOMATIONS,
                persistence::serialize_state(&automations)?,
            )])?;
        }
        if accounts.len() > MAX_LOCAL_ACCOUNTS {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!("local account count exceeds the supported limit of {MAX_LOCAL_ACCOUNTS}"),
            ));
        }
        let mut cleared_false_blocks = false;
        for account in &mut accounts {
            if zenith_relay_core::accounts::clear_false_upstream_block(
                &mut account.account.health,
                &mut account.account.last_error_code,
            ) {
                cleared_false_blocks = true;
            }
        }
        if cleared_false_blocks {
            database.replace_state_json(&[(
                STATE_ACCOUNTS,
                persistence::serialize_state(&accounts)?,
            )])?;
        }
        if let Some(operation) = &state.ownership_operation {
            operation
                .validate()
                .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message))?;
        }
        let refresh_revisions = match state.refresh_revisions {
            Some(revisions) => {
                revisions.validate(&accounts)?;
                revisions
            }
            None => {
                let revisions = RefreshRevisions::initialize(&accounts)?;
                database.replace_state_json(&[(
                    STATE_REFRESH_REVISIONS,
                    persistence::serialize_state(&revisions)?,
                )])?;
                revisions
            }
        };
        let source_refresh_revisions = match state.source_refresh_revisions {
            Some(revisions) => {
                revisions.validate(&sources)?;
                revisions
            }
            None => {
                let revisions = SourceRefreshRevisions::default().with_sources(&[], &sources)?;
                database.replace_state_json(&[(
                    STATE_SOURCE_REVISIONS,
                    persistence::serialize_state(&revisions)?,
                )])?;
                revisions
            }
        };
        persistence::cleanup_legacy_state_files(&root)?;
        Ok(Self {
            database,
            gateway,
            sources,
            accounts,
            keys: state.keys,
            automations,
            remote_target: state.remote_target,
            ownership_operation: state.ownership_operation,
            refresh_revisions,
            source_refresh_revisions,
            refresh_changed: tokio::sync::watch::channel(0).0,
        })
    }

    pub fn database(&self) -> Arc<TelemetryDb> {
        self.database.clone()
    }

    pub fn gateway(&self) -> &GatewaySettings {
        &self.gateway
    }

    pub fn sources(&self) -> &[ProviderSourceRecord] {
        &self.sources
    }

    pub fn keys(&self) -> &[LocalGatewayKeyRecord] {
        &self.keys
    }

    pub fn accounts(&self) -> &[LocalAccountRecord] {
        &self.accounts
    }

    pub fn automations(&self) -> &AutomationRecords {
        &self.automations
    }

    pub fn remote_target(&self) -> Option<&RemoteTargetRecord> {
        self.remote_target.as_ref()
    }

    pub fn ownership_operation(&self) -> Option<&OwnershipOperationRecord> {
        self.ownership_operation.as_ref()
    }

    pub fn source(&self, id: &str) -> Option<&ProviderSourceRecord> {
        self.sources.iter().find(|source| source.id == id)
    }

    pub fn key(&self, id: &str) -> Option<&LocalGatewayKeyRecord> {
        self.keys.iter().find(|key| key.id == id)
    }

    pub fn account(&self, id: &str) -> Option<&LocalAccountRecord> {
        self.accounts
            .iter()
            .find(|account| account.account.id == id)
    }

    pub fn update_client_auth_observation(
        &mut self,
        account_id: &str,
        status: Option<String>,
        login_redirect_at_ms: Option<u64>,
    ) -> Result<bool> {
        let Some(current) = self.account(account_id).cloned() else {
            return Ok(false);
        };
        if current.client_auth_status == status
            && current.last_client_login_redirect_at_ms == login_redirect_at_ms
        {
            return Ok(false);
        }
        let mut updated = current;
        updated.client_auth_status = status;
        updated.last_client_login_redirect_at_ms = login_redirect_at_ms;
        self.upsert_account(updated)?;
        Ok(true)
    }
}

fn upgrade_saved_gateway(
    database: &TelemetryDb,
    gateway: &mut GatewaySettings,
    sources: &[ProviderSourceRecord],
    accounts: &[LocalAccountRecord],
) -> Result<()> {
    // A saved current policy can still coexist with obsolete scalar fields from an
    // older build. Remove them on disk without changing any active controls.
    let remove_v1_scalars = database
        .state_json(STATE_GATEWAY)?
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .is_some_and(|value| {
            value.as_object().is_some_and(|fields| {
                [
                    "cooldownAfterFailures",
                    "keepLastCandidateAvailable",
                    "routingStrategy",
                    "subscriptionPlanOrder",
                ]
                .iter()
                .any(|key| fields.contains_key(*key))
            })
        });
    let upgrade_policy = !gateway
        .pool_routing
        .as_ref()
        .is_some_and(PoolRoutingPolicy::is_current_rotation);
    if upgrade_policy {
        let policy = gateway.pool_routing_for(sources, accounts);
        policy
            .validate_activation()
            .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message))?;
        gateway.pool_routing = Some(policy);
    }
    if upgrade_policy || remove_v1_scalars {
        database.replace_state_json(&[(STATE_GATEWAY, serialize_state(gateway)?)])?;
    }
    Ok(())
}

/// Prepares the dedicated SQLite directory before any code opens the database.
/// This also lets startup validate a database-layout conflict before it changes
/// credentials or other durable state.
#[cfg(test)]
mod tests;
