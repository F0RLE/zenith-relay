mod affinity;
mod automations;
mod backups;
mod configuration;
mod imports;
mod migrations;
mod records;
mod refresh;
mod source_refresh;
mod sqlite;
mod usage;
pub mod vault;

pub use configuration::{
    configuration_revision, ConfigurationReplaceError, ConfigurationReplacement,
};
pub use imports::PendingImport;
pub(crate) use refresh::AccountRefreshFence;
pub(crate) use source_refresh::SourceRefreshFence;
pub use sqlite::Store;
pub use vault::Vault;

#[cfg(test)]
mod test_support {
    use std::path::PathBuf;

    pub(super) fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "zenith-relay-store-{name}-{}",
            uuid::Uuid::new_v4()
        ))
    }
}
