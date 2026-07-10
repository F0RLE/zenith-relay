mod migrations;
mod quarantine;
pub mod secret_store;
mod settings_store;
pub mod telemetry_db;

use self::{
    migrations::migrate,
    settings_store::{load_json, save_json},
};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::{GatewaySettings, LocalGatewayKeyRecord, ProviderSourceRecord, StoreMetadata},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct LocalPoolStore {
    root: PathBuf,
    metadata: StoreMetadata,
    gateway: GatewaySettings,
    sources: Vec<ProviderSourceRecord>,
    keys: Vec<LocalGatewayKeyRecord>,
}

impl LocalPoolStore {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("settings")).map_err(|err| {
            LocalPoolError::new(
                ErrorCode::Io,
                format!("failed to create local pool store: {err}"),
            )
        })?;
        let metadata = migrate(&root)?;
        let gateway_path = root.join("settings").join("gateway.json");
        let gateway = match load_json(&gateway_path) {
            Ok(Some(gateway)) => gateway,
            Ok(None) => {
                let gateway = GatewaySettings::default();
                save_json(&gateway_path, &gateway)?;
                gateway
            }
            Err(error) => {
                let quarantined = quarantine::move_file(&root, &gateway_path)?;
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!(
                        "invalid gateway settings were moved to {}: {}",
                        quarantined.display(),
                        error
                    ),
                ));
            }
        };
        gateway
            .validate()
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        let sources = load_record_file(&root, "sources.json")?;
        let keys = load_record_file(&root, "keys.json")?;
        Ok(Self {
            root,
            metadata,
            gateway,
            sources,
            keys,
        })
    }

    pub fn metadata(&self) -> &StoreMetadata {
        &self.metadata
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

    pub fn source(&self, id: &str) -> Option<&ProviderSourceRecord> {
        self.sources.iter().find(|source| source.id == id)
    }

    pub fn key(&self, id: &str) -> Option<&LocalGatewayKeyRecord> {
        self.keys.iter().find(|key| key.id == id)
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

    pub fn replace_records(
        &mut self,
        sources: Vec<ProviderSourceRecord>,
        keys: Vec<LocalGatewayKeyRecord>,
    ) -> Result<()> {
        let sources_changed = sources != self.sources;
        let keys_changed = keys != self.keys;
        if !sources_changed && !keys_changed {
            return Ok(());
        }

        let sources_path = self.root.join("records").join("sources.json");
        let keys_path = self.root.join("records").join("keys.json");
        if sources_changed {
            save_json(&sources_path, &sources)?;
        }
        if keys_changed {
            if let Err(error) = save_json(&keys_path, &keys) {
                if sources_changed {
                    if let Err(rollback) = save_json(&sources_path, &self.sources) {
                        return Err(LocalPoolError::new(
                            ErrorCode::RecoveryRequired,
                            format!(
                                "{error}; failed to restore source records after key write failure: {rollback}"
                            ),
                        ));
                    }
                }
                return Err(error);
            }
        }
        self.sources = sources;
        self.keys = keys;
        Ok(())
    }

    pub fn replace_gateway(&mut self, gateway: GatewaySettings) -> Result<()> {
        if gateway == self.gateway {
            return Ok(());
        }
        gateway
            .validate()
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        save_json(&self.root.join("settings").join("gateway.json"), &gateway)?;
        self.gateway = gateway;
        Ok(())
    }

    pub fn touch_usage(&mut self, key_id: &str, source_id: &str, at: String) -> Result<()> {
        let mut sources = self.sources.clone();
        let mut keys = self.keys.clone();
        if let Some(source) = sources.iter_mut().find(|source| source.id == source_id) {
            source.last_used_at = Some(at.clone());
        }
        if let Some(key) = keys.iter_mut().find(|key| key.id == key_id) {
            key.last_used_at = Some(at);
        }
        self.replace_records(sources, keys)
    }

    pub fn set_gateway_enabled(&mut self, enabled: bool) -> Result<()> {
        let mut next = self.gateway.clone();
        next.enabled = enabled;
        self.replace_gateway(next)
    }

    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn load_record_file<T: serde::de::DeserializeOwned + serde::Serialize>(
    root: &Path,
    name: &str,
) -> Result<Vec<T>> {
    let path = root.join("records").join(name);
    match load_json(&path) {
        Ok(Some(records)) => Ok(records),
        Ok(None) => {
            let records = Vec::new();
            save_json(&path, &records)?;
            Ok(records)
        }
        Err(error) => {
            let quarantined = quarantine::move_file(root, &path)?;
            Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "invalid record file was moved to {}: {error}",
                    quarantined.display()
                ),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        sync::atomic::{AtomicU64, Ordering},
    };
    use zenith_relay_core::WireApi;

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
        assert_eq!(store.metadata().schema_version, 3);
        assert_eq!(store.gateway().port, 14998);
        drop(store);
        assert_eq!(
            LocalPoolStore::open(root.clone()).unwrap().gateway().port,
            14998
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_settings_are_quarantined() {
        let root = temp_root();
        fs::create_dir_all(root.join("settings")).unwrap();
        fs::write(root.join("settings").join("gateway.json"), "not-json").unwrap();
        let error = LocalPoolStore::open(root.clone()).err().unwrap();
        assert!(matches!(error.code, ErrorCode::RecoveryRequired));
        assert!(root.join("quarantine").read_dir().unwrap().next().is_some());
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
                draining: false,
                base_url: "https://example.test/v1".into(),
                secret_ref: "source:source_1".into(),
                wire_api: WireApi::Responses,
                models: vec!["gpt-test".into()],
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                priority: 0,
                weight: 1,
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
                secret_ref: "key:key_1".into(),
                source_ids: None,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                model_prefix: None,
                created_at: "2026-07-10T00:00:00Z".into(),
                last_used_at: None,
            })
            .unwrap();
        drop(store);

        let reopened = LocalPoolStore::open(root.clone()).unwrap();
        assert_eq!(reopened.sources()[0].models, ["gpt-test"]);
        assert_eq!(reopened.keys()[0].secret_ref, "key:key_1");
        let records = fs::read_to_string(root.join("records").join("sources.json")).unwrap();
        assert!(!records.contains("upstream-secret"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_and_key_scope_changes_are_written_together() {
        let root = temp_root();
        let mut store = LocalPoolStore::open(root.clone()).unwrap();
        let source = ProviderSourceRecord {
            id: "source_1".into(),
            name: "Synthetic".into(),
            enabled: true,
            draining: false,
            base_url: "https://example.test/v1".into(),
            secret_ref: "source:source_1".into(),
            wire_api: WireApi::Responses,
            models: vec!["gpt-test".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            last_used_at: None,
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        };
        let key = LocalGatewayKeyRecord {
            id: "key_1".into(),
            label: "Scoped".into(),
            enabled: true,
            secret_ref: "key:key_1".into(),
            source_ids: Some(vec![source.id.clone()]),
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
            created_at: "2026-07-10T00:00:00Z".into(),
            last_used_at: None,
        };
        store
            .replace_records(vec![source], vec![key.clone()])
            .unwrap();
        let mut unavailable_key = key;
        unavailable_key.source_ids = Some(Vec::new());
        store
            .replace_records(Vec::new(), vec![unavailable_key])
            .unwrap();
        drop(store);

        let reopened = LocalPoolStore::open(root.clone()).unwrap();
        assert!(reopened.sources().is_empty());
        assert_eq!(reopened.keys()[0].source_ids, Some(Vec::new()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_key_write_restores_source_file_and_memory() {
        let root = temp_root();
        let mut store = LocalPoolStore::open(root.clone()).unwrap();
        let source = ProviderSourceRecord {
            id: "source_1".into(),
            name: "Before".into(),
            enabled: true,
            draining: false,
            base_url: "https://example.test/v1".into(),
            secret_ref: "source:source_1".into(),
            wire_api: WireApi::Responses,
            models: vec!["gpt-test".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            last_used_at: None,
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        };
        let key = LocalGatewayKeyRecord {
            id: "key_1".into(),
            label: "Before".into(),
            enabled: true,
            secret_ref: "key:key_1".into(),
            source_ids: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
            created_at: "2026-07-10T00:00:00Z".into(),
            last_used_at: None,
        };
        store
            .replace_records(vec![source.clone()], vec![key.clone()])
            .unwrap();
        fs::create_dir(root.join("records").join("keys.tmp")).unwrap();
        let mut changed_source = source;
        changed_source.name = "After".into();
        let mut changed_key = key;
        changed_key.label = "After".into();

        assert!(store
            .replace_records(vec![changed_source], vec![changed_key])
            .is_err());
        assert_eq!(store.sources()[0].name, "Before");
        let persisted: Vec<ProviderSourceRecord> =
            load_json(&root.join("records").join("sources.json"))
                .unwrap()
                .unwrap();
        assert_eq!(persisted[0].name, "Before");
        fs::remove_dir_all(root).unwrap();
    }
}
