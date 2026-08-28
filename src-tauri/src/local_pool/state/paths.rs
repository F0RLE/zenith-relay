use super::DesktopState;
use std::path::PathBuf;

impl DesktopState {
    pub fn profile_backup_root(&self) -> PathBuf {
        self.recovery_root().join("profiles")
    }

    pub fn history_repair_backup_root(&self) -> PathBuf {
        self.recovery_root().join("history-repair")
    }

    pub fn ready_api_backup_root(&self) -> PathBuf {
        self.recovery_root().join("client-config")
    }

    pub fn data_root(&self) -> PathBuf {
        self.root.join("data")
    }

    pub fn recovery_root(&self) -> PathBuf {
        self.root.join("recovery")
    }

    pub fn transient_root(&self) -> PathBuf {
        self.cache_root()
    }

    pub fn output_root(&self) -> PathBuf {
        self.cache_root()
    }

    pub fn cache_root(&self) -> PathBuf {
        self.root.join("cache")
    }
}
