mod migrations;
mod quarantine;
pub mod secret_store;
mod settings_store;

use self::{
    migrations::migrate,
    settings_store::{load_json, save_json},
};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::{GatewaySettings, StoreMetadata},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct LocalPoolStore {
    root: PathBuf,
    metadata: StoreMetadata,
    gateway: GatewaySettings,
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
        Ok(Self {
            root,
            metadata,
            gateway,
        })
    }

    pub fn metadata(&self) -> &StoreMetadata {
        &self.metadata
    }

    pub fn gateway(&self) -> &GatewaySettings {
        &self.gateway
    }

    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        sync::atomic::{AtomicU64, Ordering},
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
        assert_eq!(store.metadata().schema_version, 1);
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
}
