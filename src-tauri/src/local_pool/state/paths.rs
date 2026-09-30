use super::DesktopState;
use crate::storage_paths::StoragePaths;
use std::path::PathBuf;

impl DesktopState {
    pub fn profile_backup_root(&self) -> PathBuf {
        self.storage_paths().profile_backup_root()
    }

    pub fn history_repair_backup_root(&self) -> PathBuf {
        self.storage_paths().history_repair_backup_root()
    }

    pub fn ready_api_backup_root(&self) -> PathBuf {
        self.storage_paths().ready_api_backup_root()
    }

    pub fn opencode_backup_root(&self) -> PathBuf {
        self.storage_paths().opencode_backup_root()
    }

    pub fn data_root(&self) -> PathBuf {
        self.storage_paths().data_root()
    }

    pub fn transient_root(&self) -> PathBuf {
        self.cache_root()
    }

    pub fn output_root(&self) -> PathBuf {
        self.storage_paths().exports_root()
    }

    pub fn logs_root(&self) -> PathBuf {
        self.storage_paths().logs_root()
    }

    pub fn error_logs_root(&self) -> PathBuf {
        self.storage_paths().error_logs_root()
    }

    pub fn crash_logs_root(&self) -> PathBuf {
        self.storage_paths().crash_logs_root()
    }

    pub fn operation_logs_root(&self) -> PathBuf {
        self.storage_paths().operation_logs_root()
    }

    pub fn cache_root(&self) -> PathBuf {
        self.storage_paths().cache_root()
    }

    fn storage_paths(&self) -> StoragePaths {
        StoragePaths::from_root(&self.root)
    }
}

mod migrate;

pub(crate) use migrate::migrate_storage_layout;
