pub mod secret_store;
pub mod telemetry_db;
pub(crate) mod vault;

use self::telemetry_db::TelemetryDb;
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::{
        AutomationRecords, GatewaySettings, LocalAccountRecord, LocalGatewayKeyRecord,
        OwnershipOperationRecord, ProviderSourceRecord, RemoteTargetRecord, CURRENT_SCHEMA_VERSION,
        MAX_LOCAL_ACCOUNTS,
    },
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use zenith_relay_core::{normalize_image_base_model, normalize_subscription_plan_order};

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

pub struct LocalPoolStore {
    database: Arc<TelemetryDb>,
    gateway: GatewaySettings,
    sources: Vec<ProviderSourceRecord>,
    accounts: Vec<LocalAccountRecord>,
    keys: Vec<LocalGatewayKeyRecord>,
    automations: AutomationRecords,
    remote_target: Option<RemoteTargetRecord>,
    ownership_operation: Option<OwnershipOperationRecord>,
}

impl LocalPoolStore {
    pub fn open(app_root: PathBuf) -> Result<Self> {
        let root = app_root.join("data");
        fs::create_dir_all(&root).map_err(|err| {
            LocalPoolError::new(
                ErrorCode::Io,
                format!("failed to create local pool store: {err}"),
            )
        })?;
        migrate_database_file(&root)?;
        let database = Arc::new(TelemetryDb::open(&root.join("relay.sqlite"))?);
        let state = load_or_initialize_state(&root, &database)?;
        let gateway = state.gateway;
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
        let accounts = state.accounts;
        let automations = state.automations;
        if accounts.len() > MAX_LOCAL_ACCOUNTS {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!("local account count exceeds the supported limit of {MAX_LOCAL_ACCOUNTS}"),
            ));
        }
        if let Some(operation) = &state.ownership_operation {
            operation
                .validate()
                .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message))?;
        }
        cleanup_legacy_state_files(&root)?;
        Ok(Self {
            database,
            gateway,
            sources,
            accounts,
            keys: state.keys,
            automations,
            remote_target: state.remote_target,
            ownership_operation: state.ownership_operation,
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

    pub fn upsert_source(&mut self, source: ProviderSourceRecord) -> Result<()> {
        let mut next = self.sources.clone();
        if let Some(current) = next.iter_mut().find(|current| current.id == source.id) {
            *current = source;
        } else {
            next.push(source);
        }
        self.replace_records(next, self.keys.clone())
    }

    pub fn upsert_key(&mut self, key: LocalGatewayKeyRecord) -> Result<()> {
        let mut next = self.keys.clone();
        if let Some(current) = next.iter_mut().find(|current| current.id == key.id) {
            *current = key;
        } else {
            next.push(key);
        }
        self.replace_records(self.sources.clone(), next)
    }

    pub fn upsert_account(&mut self, account: LocalAccountRecord) -> Result<()> {
        let mut next = self.accounts.clone();
        if let Some(current) = next
            .iter_mut()
            .find(|current| current.account.id == account.account.id)
        {
            *current = account;
        } else {
            next.push(account);
        }
        self.replace_accounts_and_keys(next, self.keys.clone())
    }

    pub fn replace_records(
        &mut self,
        sources: Vec<ProviderSourceRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
    ) -> Result<()> {
        self.replace_all_records(
            sources,
            self.accounts.clone(),
            keys,
            self.automations.clone(),
        )
    }

    pub fn replace_accounts_and_keys(
        &mut self,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
    ) -> Result<()> {
        self.replace_all_records(
            self.sources.clone(),
            accounts,
            keys,
            self.automations.clone(),
        )
    }

    pub fn replace_pool_records(
        &mut self,
        sources: Vec<ProviderSourceRecord>,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
    ) -> Result<()> {
        self.replace_all_records(sources, accounts, keys, self.automations.clone())
    }

    pub fn replace_automations(&mut self, automations: AutomationRecords) -> Result<()> {
        self.replace_all_records(
            self.sources.clone(),
            self.accounts.clone(),
            self.keys.clone(),
            automations,
        )
    }

    pub fn replace_account_state(
        &mut self,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        automations: AutomationRecords,
    ) -> Result<()> {
        self.replace_all_records(self.sources.clone(), accounts, keys, automations)
    }

    pub fn delete_account_state(
        &mut self,
        account_id: &str,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        automations: AutomationRecords,
    ) -> Result<()> {
        self.delete_accounts_state(
            std::slice::from_ref(&account_id.to_string()),
            accounts,
            keys,
            automations,
        )
    }

    pub fn delete_accounts_state(
        &mut self,
        account_ids: &[String],
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        automations: AutomationRecords,
    ) -> Result<()> {
        self.replace_all_records_inner(
            self.sources.clone(),
            accounts,
            keys,
            automations,
            account_ids,
        )
    }

    fn replace_all_records(
        &mut self,
        sources: Vec<ProviderSourceRecord>,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        automations: AutomationRecords,
    ) -> Result<()> {
        self.replace_all_records_inner(sources, accounts, keys, automations, &[])
    }

    fn replace_all_records_inner(
        &mut self,
        sources: Vec<ProviderSourceRecord>,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        automations: AutomationRecords,
        deleted_account_ids: &[String],
    ) -> Result<()> {
        if accounts.len() > MAX_LOCAL_ACCOUNTS {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("local account count exceeds the supported limit of {MAX_LOCAL_ACCOUNTS}"),
            ));
        }
        let changed = RecordChanges {
            sources: sources != self.sources,
            accounts: accounts != self.accounts,
            keys: keys != self.keys,
            automations: automations != self.automations,
        };
        if !changed.any() {
            return Ok(());
        }

        let mut values = Vec::with_capacity(4);
        if changed.sources {
            values.push((STATE_SOURCES, serialize_state(&sources)?));
        }
        if changed.accounts {
            values.push((STATE_ACCOUNTS, serialize_state(&accounts)?));
        }
        if changed.keys {
            values.push((STATE_KEYS, serialize_state(&keys)?));
        }
        if changed.automations {
            values.push((STATE_AUTOMATIONS, serialize_state(&automations)?));
        }
        if deleted_account_ids.is_empty() {
            self.database.replace_state_json(&values)?;
        } else if deleted_account_ids.len() == 1 {
            self.database
                .replace_state_json_and_delete_account_data(&values, &deleted_account_ids[0])?;
        } else {
            self.database
                .replace_state_json_and_delete_accounts_data(&values, deleted_account_ids)?;
        }

        self.sources = sources;
        self.accounts = accounts;
        self.keys = keys;
        self.automations = automations;
        Ok(())
    }

    pub fn replace_gateway(&mut self, mut gateway: GatewaySettings) -> Result<()> {
        gateway.hidden_models = crate::local_pool::models::normalized_values(gateway.hidden_models);
        gateway.model_price_overrides = gateway
            .model_price_overrides
            .into_iter()
            .map(|(model, price)| (model.trim().to_ascii_lowercase(), price))
            .collect();
        gateway.model_reasoning_allowed_levels =
            zenith_relay_core::normalize_model_reasoning_allowed_levels(
                gateway.model_reasoning_allowed_levels,
            )
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        gateway.subscription_plan_order =
            normalize_subscription_plan_order(gateway.subscription_plan_order)
                .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        gateway.image_base_model = normalize_image_base_model(gateway.image_base_model)
            .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
        if gateway == self.gateway {
            return Ok(());
        }
        gateway
            .validate()
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        self.database
            .replace_state_json(&[(STATE_GATEWAY, serialize_state(&gateway)?)])?;
        self.gateway = gateway;
        Ok(())
    }

    pub fn replace_remote_target(&mut self, target: Option<RemoteTargetRecord>) -> Result<()> {
        if target == self.remote_target {
            return Ok(());
        }
        self.database
            .replace_state_json(&[(STATE_REMOTE_TARGET, serialize_state(&target)?)])?;
        self.remote_target = target;
        Ok(())
    }

    pub fn replace_ownership_operation(
        &mut self,
        operation: Option<OwnershipOperationRecord>,
    ) -> Result<()> {
        if let Some(operation) = &operation {
            operation
                .validate()
                .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        }
        if operation == self.ownership_operation {
            return Ok(());
        }
        self.database
            .replace_state_json(&[(STATE_OWNERSHIP_OPERATION, serialize_state(&operation)?)])?;
        self.ownership_operation = operation;
        Ok(())
    }

    pub fn replace_accounts_keys_and_ownership_operation(
        &mut self,
        accounts: Vec<LocalAccountRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
        operation: Option<OwnershipOperationRecord>,
    ) -> Result<()> {
        if accounts.len() > MAX_LOCAL_ACCOUNTS {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("local account count exceeds the supported limit of {MAX_LOCAL_ACCOUNTS}"),
            ));
        }
        if let Some(operation) = &operation {
            operation
                .validate()
                .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        }
        self.database.replace_state_json(&[
            (STATE_ACCOUNTS, serialize_state(&accounts)?),
            (STATE_KEYS, serialize_state(&keys)?),
            (STATE_OWNERSHIP_OPERATION, serialize_state(&operation)?),
        ])?;
        self.accounts = accounts;
        self.keys = keys;
        self.ownership_operation = operation;
        Ok(())
    }

    pub fn touch_usage(
        &mut self,
        key_id: &str,
        source_id: &str,
        account_id: Option<&str>,
        at: String,
    ) -> Result<()> {
        let mut sources = self.sources.clone();
        let mut keys = self.keys.clone();
        if let Some(source) = sources.iter_mut().find(|source| source.id == source_id) {
            source.last_used_at = Some(at.clone());
        }
        if let Some(key) = keys.iter_mut().find(|key| key.id == key_id) {
            key.last_used_at = Some(at.clone());
        }
        let mut accounts = self.accounts.clone();
        if let (Some(account_id), Some(last_used_at_ms)) = (
            account_id,
            chrono::DateTime::parse_from_rfc3339(&at)
                .ok()
                .and_then(|value| u64::try_from(value.timestamp_millis()).ok()),
        ) {
            if let Some(account) = accounts
                .iter_mut()
                .find(|account| account.account.id == account_id)
            {
                account.account.last_used_at_ms = Some(last_used_at_ms);
            }
        }
        self.replace_all_records(sources, accounts, keys, self.automations.clone())
    }

    pub fn set_gateway_enabled(&mut self, enabled: bool) -> Result<()> {
        let mut next = self.gateway.clone();
        next.enabled = enabled;
        self.replace_gateway(next)
    }

    pub fn reset_local_records(&mut self) -> Result<()> {
        self.replace_gateway(GatewaySettings::default())?;
        self.replace_all_records(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            AutomationRecords::default(),
        )
    }
}

#[derive(Clone, Copy)]
struct RecordChanges {
    sources: bool,
    accounts: bool,
    keys: bool,
    automations: bool,
}

impl RecordChanges {
    fn any(self) -> bool {
        self.sources || self.accounts || self.keys || self.automations
    }
}

#[derive(Default)]
struct PersistedState {
    gateway: GatewaySettings,
    sources: Vec<ProviderSourceRecord>,
    accounts: Vec<LocalAccountRecord>,
    keys: Vec<LocalGatewayKeyRecord>,
    automations: AutomationRecords,
    remote_target: Option<RemoteTargetRecord>,
    ownership_operation: Option<OwnershipOperationRecord>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyMetadata {
    schema_version: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyRemoteTargets {
    active: Option<RemoteTargetRecord>,
}

fn load_or_initialize_state(root: &Path, database: &TelemetryDb) -> Result<PersistedState> {
    let values = database.state_json_values()?;
    if values.is_empty() {
        let state = load_legacy_state(root)?.unwrap_or_default();
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

fn serialize_state<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("local state serialization failed: {error}"),
        )
    })
}

fn load_legacy_state(root: &Path) -> Result<Option<PersistedState>> {
    let metadata_path = root.join("metadata.json");
    if !metadata_path.exists() {
        if LEGACY_STATE_FILES
            .iter()
            .skip(1)
            .any(|name| root.join(name).exists())
        {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "legacy local data exist but metadata.json is missing",
            ));
        }
        return Ok(None);
    }
    let metadata: LegacyMetadata = read_legacy_json(&metadata_path)?;
    if metadata.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(LocalPoolError::new(
            ErrorCode::UnsupportedSchema,
            format!(
                "legacy local data schema {} is unsupported; expected {CURRENT_SCHEMA_VERSION}",
                metadata.schema_version
            ),
        ));
    }
    let remote_target = if root.join("remote-target.json").exists() {
        read_legacy_json::<LegacyRemoteTargets>(&root.join("remote-target.json"))?.active
    } else {
        None
    };
    Ok(Some(PersistedState {
        gateway: read_legacy_json(&root.join("settings.json"))?,
        sources: read_legacy_json(&root.join("connections.json"))?,
        accounts: read_legacy_json(&root.join("accounts.json"))?,
        keys: read_legacy_json(&root.join("pool-keys.json"))?,
        automations: read_legacy_json(&root.join("automations.json"))?,
        remote_target,
        ownership_operation: None,
    }))
}

fn read_legacy_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let metadata = fs::symlink_metadata(path).map_err(legacy_io_error)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_LEGACY_JSON_BYTES
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("legacy local state file is unsafe: {}", path.display()),
        ));
    }
    let content = fs::read_to_string(path).map_err(legacy_io_error)?;
    serde_json::from_str(&content).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "legacy local state is invalid in {}: {error}",
                path.display()
            ),
        )
    })
}

fn cleanup_legacy_state_files(root: &Path) -> Result<()> {
    for name in LEGACY_STATE_FILES {
        let path = root.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                fs::remove_file(&path).map_err(legacy_io_error)?;
            }
            Ok(_) => {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("legacy local state path is unsafe: {}", path.display()),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(legacy_io_error(error)),
        }
    }
    Ok(())
}

fn migrate_database_file(root: &Path) -> Result<()> {
    let legacy = root.join("usage.sqlite");
    let target = root.join("relay.sqlite");
    if legacy.exists() && target.exists() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "both usage.sqlite and relay.sqlite exist",
        ));
    }
    if !legacy.exists() {
        return Ok(());
    }
    fs::rename(&legacy, &target).map_err(legacy_io_error)?;
    for suffix in ["-wal", "-shm"] {
        let source = companion_path(&legacy, suffix);
        if source.exists() {
            fs::rename(&source, companion_path(&target, suffix)).map_err(legacy_io_error)?;
        }
    }
    Ok(())
}

fn companion_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn legacy_io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::Io,
        format!("local data migration failed: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::models::{OwnershipOperationKind, OwnershipOperationPhase};
    use std::collections::{BTreeMap, BTreeSet};
    use std::{
        env,
        sync::atomic::{AtomicU64, Ordering},
    };
    use zenith_relay_core::{
        accounts::{
            AccountAuthMode, AccountAuthState, AccountHealthState, AccountIdentity, AccountRecord,
        },
        quota::{QuotaSnapshot, Subscription},
        RoutingStrategy, WireApi,
    };

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        let root = env::temp_dir().join(format!(
            "zenith-relay-store-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn fresh_store_is_versioned_and_restart_safe() {
        let root = temp_root();
        let store = LocalPoolStore::open(root.clone()).unwrap();
        assert_eq!(store.gateway().port, 14998);
        assert_eq!(store.database().state_count().unwrap(), 7);
        assert!(root.join("data/relay.sqlite").exists());
        assert!(!root.join("data/metadata.json").exists());
        drop(store);
        assert_eq!(
            LocalPoolStore::open(root.clone()).unwrap().gateway().port,
            14998
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quota_and_routing_policy_survive_restart() {
        let root = temp_root();
        let mut store = LocalPoolStore::open(root.clone()).unwrap();
        let mut gateway = store.gateway().clone();
        gateway.quota_request_timeout_seconds = 10;
        gateway.chatgpt_interface_quota_reserve_basis_points = 700;
        gateway.routing_strategy = RoutingStrategy::SubscriptionExpiry;
        gateway.subscription_plan_order = vec!["business".into(), "plus".into()];
        gateway.image_base_model = Some("gpt-5.4-mini".into());
        gateway.model_price_overrides.insert(
            "GPT-5.4".into(),
            zenith_relay_core::ApiModelPriceOverride {
                input_micro_usd_per_million: 1_250_000,
                cached_input_micro_usd_per_million: Some(125_000),
                cache_write_5m_micro_usd_per_million: None,
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 7_500_000,
            },
        );
        store.replace_gateway(gateway).unwrap();
        drop(store);

        let reopened = LocalPoolStore::open(root.clone()).unwrap();
        assert_eq!(reopened.gateway().quota_request_timeout_seconds, 10);
        assert_eq!(
            reopened
                .gateway()
                .chatgpt_interface_quota_reserve_basis_points,
            700
        );
        assert_eq!(
            reopened.gateway().routing_strategy,
            RoutingStrategy::SubscriptionExpiry
        );
        assert_eq!(
            reopened.gateway().subscription_plan_order,
            ["business", "plus"]
        );
        assert_eq!(
            reopened.gateway().image_base_model.as_deref(),
            Some("gpt-5.4-mini")
        );
        assert_eq!(
            reopened.gateway().model_price_overrides.get("gpt-5.4"),
            Some(&zenith_relay_core::ApiModelPriceOverride {
                input_micro_usd_per_million: 1_250_000,
                cached_input_micro_usd_per_million: Some(125_000),
                cache_write_5m_micro_usd_per_million: None,
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 7_500_000,
            })
        );
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remote_target_survives_restart() {
        let root = temp_root();
        let target = RemoteTargetRecord {
            origin: "https://relay.example.test".into(),
            server_id: "server_1".into(),
            identity_fingerprint: "sha256:test".into(),
            server_version: "1.1.0".into(),
            protocol_version: 1,
            allow_insecure_http: false,
            secret_ref: "remote:server_1".into(),
            connected_at_ms: 123,
        };
        let mut store = LocalPoolStore::open(root.clone()).unwrap();
        store.replace_remote_target(Some(target.clone())).unwrap();
        drop(store);

        let reopened = LocalPoolStore::open(root.clone()).unwrap();
        assert_eq!(reopened.remote_target(), Some(&target));
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ownership_operation_survives_restart_without_secret_material() {
        let root = temp_root();
        let operation = OwnershipOperationRecord {
            id: "ownership_0123456789abcdef0123456789abcdef".into(),
            kind: OwnershipOperationKind::MoveToRemote,
            phase: OwnershipOperationPhase::MoveRemoteCommitted,
            server_id: "server_1".into(),
            local_account_ids: vec!["account_local".into()],
            remote_account_ids: vec!["account_remote".into()],
            created_remote_account_ids: vec!["account_remote".into()],
            created_at_ms: 100,
            updated_at_ms: 200,
        };
        let mut store = LocalPoolStore::open(root.clone()).unwrap();
        store
            .replace_ownership_operation(Some(operation.clone()))
            .unwrap();
        drop(store);

        let reopened = LocalPoolStore::open(root.clone()).unwrap();
        assert_eq!(reopened.ownership_operation(), Some(&operation));
        let stored = reopened
            .database()
            .state_json(STATE_OWNERSHIP_OPERATION)
            .unwrap()
            .unwrap();
        assert!(!stored.contains("access_token"));
        assert!(!stored.contains("refresh_token"));
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_database_state_is_rejected_without_deleting_the_database() {
        let root = temp_root();
        let store = LocalPoolStore::open(root.clone()).unwrap();
        store
            .database()
            .replace_state_json(&[(STATE_GATEWAY, "not-json".to_string())])
            .unwrap();
        drop(store);
        let error = LocalPoolStore::open(root.clone()).err().unwrap();
        assert!(matches!(error.code, ErrorCode::RecoveryRequired));
        assert!(root.join("data/relay.sqlite").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_json_store_is_imported_once_and_removed() {
        let root = temp_root();
        let data = root.join("data");
        fs::create_dir_all(&data).unwrap();
        write_json(
            &data.join("metadata.json"),
            &serde_json::json!({"schemaVersion": CURRENT_SCHEMA_VERSION}),
        );
        write_json(&data.join("settings.json"), &GatewaySettings::default());
        write_json(
            &data.join("connections.json"),
            &Vec::<ProviderSourceRecord>::new(),
        );
        write_json(
            &data.join("accounts.json"),
            &vec![account_record("imported")],
        );
        write_json(
            &data.join("pool-keys.json"),
            &Vec::<LocalGatewayKeyRecord>::new(),
        );
        write_json(
            &data.join("automations.json"),
            &AutomationRecords::default(),
        );
        rusqlite::Connection::open(data.join("usage.sqlite")).unwrap();

        let store = LocalPoolStore::open(root.clone()).unwrap();
        assert_eq!(store.accounts()[0].account.id, "imported");
        assert!(data.join("relay.sqlite").exists());
        assert!(!data.join("usage.sqlite").exists());
        for name in LEGACY_STATE_FILES {
            assert!(!data.join(name).exists());
        }
        drop(store);
        assert_eq!(
            LocalPoolStore::open(root.clone()).unwrap().accounts()[0]
                .account
                .id,
            "imported"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_and_key_records_survive_restart_without_secret_values() {
        let root = temp_root();
        let mut store = LocalPoolStore::open(root.clone()).unwrap();
        store
            .upsert_source(ProviderSourceRecord {
                id: "source_1".into(),
                name: "Synthetic".into(),
                enabled: true,
                in_pool: true,
                draining: false,
                base_url: "https://example.test/v1".into(),
                secret_ref: "source:source_1".into(),
                wire_api: WireApi::Responses,
                protocol_bindings: Vec::new(),
                models: vec!["gpt-test".into()],
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                priority: 0,
                weight: 1,
                recovery_delay_seconds: 0,
                model_price_overrides: Default::default(),
                detected_model_prices: std::collections::BTreeMap::from([(
                    "gpt-test".into(),
                    zenith_relay_core::ApiModelPriceOverride {
                        input_micro_usd_per_million: 1_000_000,
                        cached_input_micro_usd_per_million: Some(100_000),
                        cache_write_5m_micro_usd_per_million: None,
                        cache_write_1h_micro_usd_per_million: None,
                        output_micro_usd_per_million: 2_000_000,
                    },
                )]),
                last_used_at: None,
                last_test_at: None,
                last_test_status: None,
                last_error: None,
            })
            .unwrap();
        store
            .upsert_key(LocalGatewayKeyRecord {
                id: "key_1".into(),
                label: "Default".into(),
                enabled: true,
                system: false,
                secret_ref: "key:key_1".into(),
                created_at: "2026-07-10T00:00:00Z".into(),
                last_used_at: None,
            })
            .unwrap();
        drop(store);

        let reopened = LocalPoolStore::open(root.clone()).unwrap();
        assert_eq!(reopened.sources()[0].models, ["gpt-test"]);
        assert_eq!(
            reopened.sources()[0].detected_model_prices.get("gpt-test"),
            Some(&zenith_relay_core::ApiModelPriceOverride {
                input_micro_usd_per_million: 1_000_000,
                cached_input_micro_usd_per_million: Some(100_000),
                cache_write_5m_micro_usd_per_million: None,
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 2_000_000,
            })
        );
        assert_eq!(reopened.keys()[0].secret_ref, "key:key_1");
        let records = reopened
            .database()
            .state_json(STATE_SOURCES)
            .unwrap()
            .unwrap();
        assert!(!records.contains("upstream-secret"));
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_transaction_preserves_all_state_and_memory() {
        let root = temp_root();
        let mut store = LocalPoolStore::open(root.clone()).unwrap();
        let source = ProviderSourceRecord {
            id: "source_1".into(),
            name: "Before".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "https://example.test/v1".into(),
            secret_ref: "source:source_1".into(),
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            models: vec!["gpt-test".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: Default::default(),
            detected_model_prices: Default::default(),
            last_used_at: None,
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        };
        let key = LocalGatewayKeyRecord {
            id: "key_1".into(),
            label: "Before".into(),
            enabled: true,
            system: false,
            secret_ref: "key:key_1".into(),
            created_at: "2026-07-10T00:00:00Z".into(),
            last_used_at: None,
        };
        store
            .replace_records(vec![source.clone()], vec![key.clone()])
            .unwrap();
        let mut changed_source = source;
        changed_source.name = "After".into();
        let mut changed_key = key;
        changed_key.label = "x".repeat(17 * 1024 * 1024);

        assert!(store
            .replace_records(vec![changed_source], vec![changed_key])
            .is_err());
        assert_eq!(store.sources()[0].name, "Before");
        drop(store);
        assert_eq!(
            LocalPoolStore::open(root.clone()).unwrap().sources()[0].name,
            "Before"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn store_rejects_account_overflow_without_changing_persisted_state() {
        let root = temp_root();
        let mut store = LocalPoolStore::open(root.clone()).unwrap();
        let accounts = (0..MAX_LOCAL_ACCOUNTS)
            .map(|index| account_record(&format!("account-{index}")))
            .collect::<Vec<_>>();
        store
            .replace_accounts_and_keys(accounts.clone(), Vec::new())
            .unwrap();

        let mut overflow = accounts;
        overflow.push(account_record("account-overflow"));
        let error = store
            .replace_accounts_and_keys(overflow, Vec::new())
            .unwrap_err();
        assert!(matches!(error.code, ErrorCode::InvalidState));
        assert_eq!(store.accounts().len(), MAX_LOCAL_ACCOUNTS);
        drop(store);
        assert_eq!(
            LocalPoolStore::open(root.clone()).unwrap().accounts().len(),
            MAX_LOCAL_ACCOUNTS
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reset_clears_runtime_records_but_keeps_profile_backups() {
        let root = temp_root();
        let mut store = LocalPoolStore::open(root.clone()).unwrap();
        store
            .replace_accounts_and_keys(vec![account_record("account-reset")], Vec::new())
            .unwrap();
        let backup = root.join("recovery/profiles/config.toml");
        fs::create_dir_all(backup.parent().unwrap()).unwrap();
        fs::write(&backup, "preserved").unwrap();

        store.reset_local_records().unwrap();
        drop(store);

        let reopened = LocalPoolStore::open(root.clone()).unwrap();
        assert!(reopened.accounts().is_empty());
        assert!(reopened.sources().is_empty());
        assert!(reopened.keys().is_empty());
        assert!(reopened.automations().tasks.is_empty());
        assert!(!reopened.gateway().enabled);
        assert_eq!(fs::read_to_string(backup).unwrap(), "preserved");
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    fn account_record(id: &str) -> LocalAccountRecord {
        LocalAccountRecord {
            account: AccountRecord {
                id: id.into(),
                label: id.into(),
                identity: AccountIdentity::from_hashed_parts(
                    "openai",
                    "chatgpt.com/backend-api/codex",
                    &format!("identity-{id}"),
                    &format!("secret-{id}"),
                    "default",
                    None,
                )
                .unwrap(),
                auth_mode: AccountAuthMode::OAuth,
                auth_state: AccountAuthState::Active,
                health: AccountHealthState::Healthy,
                source_id: "openai_codex".into(),
                secret_refs: vec![format!("account:{id}")],
                subscription: Subscription::default(),
                quota: QuotaSnapshot::default(),
                token_generation: 1,
                token_updated_at_ms: Some(1),
                tags: BTreeSet::new(),
                enabled: true,
                in_pool: true,
                draining: false,
                created_at_ms: 1,
                last_used_at_ms: None,
                last_error_code: None,
            },
            purchase_cost_micro_usd: None,
            remote_location: None,
            wire_api: WireApi::Responses,
            models: vec!["gpt-test".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            cooldowns: BTreeMap::new(),
            consecutive_failures: 0,
        }
    }

    fn write_json(path: &Path, value: &impl Serialize) {
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }
}
