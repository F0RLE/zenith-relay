use std::path::{Path, PathBuf};

/// Stable, Relay-owned paths below the platform-selected application root.
///
/// The platform layer chooses the root using Tauri's per-user local-data
/// resolver. Keeping the layout itself independent from a platform avoids
/// Windows-only path assumptions and makes data migrations explicit.
#[derive(Clone, Debug)]
pub(crate) struct StoragePaths {
    root: PathBuf,
}

impl StoragePaths {
    pub(crate) fn from_root(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub(crate) fn data_root(&self) -> PathBuf {
        self.root.join("data")
    }

    pub(crate) fn database_root(&self) -> PathBuf {
        self.data_root().join("database")
    }

    pub(crate) fn database_file(&self) -> PathBuf {
        self.database_root().join("relay.sqlite")
    }

    pub(crate) fn vault_root(&self) -> PathBuf {
        self.data_root().join("vault")
    }

    pub(crate) fn catalog_root(&self) -> PathBuf {
        self.data_root().join("catalogs")
    }

    pub(crate) fn pricing_catalog_file(&self) -> PathBuf {
        self.catalog_root().join("litellm-prices.json")
    }

    pub(crate) fn model_metadata_catalog_file(&self) -> PathBuf {
        self.catalog_root().join("models-dev.json")
    }

    pub(crate) fn migration_root(&self) -> PathBuf {
        self.data_root().join("migrations")
    }

    pub(crate) fn keyring_migration_marker(&self) -> PathBuf {
        self.migration_root().join("keyring-v1.complete")
    }

    pub(crate) fn cache_root(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub(crate) fn webview_root(&self) -> PathBuf {
        self.cache_root().join("webview")
    }

    pub(crate) fn exports_root(&self) -> PathBuf {
        self.root.join("exports")
    }

    pub(crate) fn logs_root(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub(crate) fn error_logs_root(&self) -> PathBuf {
        self.logs_root().join("errors")
    }

    pub(crate) fn crash_logs_root(&self) -> PathBuf {
        self.logs_root().join("crashes")
    }

    pub(crate) fn operation_logs_root(&self) -> PathBuf {
        self.logs_root().join("operations")
    }

    pub(crate) fn recovery_root(&self) -> PathBuf {
        self.root.join("recovery")
    }

    pub(crate) fn profile_backup_root(&self) -> PathBuf {
        self.recovery_root().join("applications").join("chatgpt")
    }

    pub(crate) fn history_repair_backup_root(&self) -> PathBuf {
        self.recovery_root()
            .join("operations")
            .join("history-repair")
    }

    pub(crate) fn ready_api_backup_root(&self) -> PathBuf {
        self.profile_backup_root().join("client-config")
    }

    pub(crate) fn opencode_backup_root(&self) -> PathBuf {
        self.recovery_root().join("applications").join("opencode")
    }
}

#[cfg(test)]
mod tests {
    use super::StoragePaths;
    use std::path::PathBuf;

    #[test]
    fn durable_transient_recovery_and_export_paths_do_not_overlap() {
        let root = PathBuf::from("storage-root");
        let paths = StoragePaths::from_root(&root);

        assert_eq!(
            paths.database_file(),
            root.join("data/database/relay.sqlite")
        );
        assert_eq!(paths.vault_root(), root.join("data/vault"));
        assert_eq!(
            paths.pricing_catalog_file(),
            root.join("data/catalogs/litellm-prices.json")
        );
        assert_eq!(paths.webview_root(), root.join("cache/webview"));
        assert_eq!(paths.exports_root(), root.join("exports"));
        assert_eq!(paths.logs_root(), root.join("logs"));
        assert_eq!(paths.error_logs_root(), root.join("logs/errors"));
        assert_eq!(paths.crash_logs_root(), root.join("logs/crashes"));
        assert_eq!(paths.operation_logs_root(), root.join("logs/operations"));
        assert_eq!(
            paths.history_repair_backup_root(),
            root.join("recovery/operations/history-repair")
        );
    }
}
