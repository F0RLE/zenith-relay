use super::DesktopState;
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

const APPLICATIONS_DIR: &str = "applications";
const CHATGPT_DIR: &str = "chatgpt";
const OPENCODE_DIR: &str = "opencode";
const OPERATIONS_DIR: &str = "operations";
const HISTORY_REPAIR_DIR: &str = "history-repair";

impl DesktopState {
    pub fn profile_backup_root(&self) -> PathBuf {
        self.recovery_root()
            .join(APPLICATIONS_DIR)
            .join(CHATGPT_DIR)
    }

    pub fn history_repair_backup_root(&self) -> PathBuf {
        self.recovery_root()
            .join(OPERATIONS_DIR)
            .join(HISTORY_REPAIR_DIR)
    }

    pub fn ready_api_backup_root(&self) -> PathBuf {
        self.profile_backup_root().join("client-config")
    }

    pub fn opencode_backup_root(&self) -> PathBuf {
        self.recovery_root()
            .join(APPLICATIONS_DIR)
            .join(OPENCODE_DIR)
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

/// Move recovery artifacts from the pre-application layout into the current
/// layout. Existing destination files always win; conflicts remain in the
/// legacy directory so migration cannot destroy user data.
pub(crate) fn migrate_recovery_layout(root: &Path) -> Result<()> {
    let recovery = root.join("recovery");
    migrate_directory(
        &recovery.join("profiles"),
        &recovery.join(APPLICATIONS_DIR).join(CHATGPT_DIR),
    )?;
    migrate_directory(
        &recovery.join("client-config"),
        &recovery
            .join(APPLICATIONS_DIR)
            .join(CHATGPT_DIR)
            .join("client-config"),
    )?;
    migrate_directory(
        &recovery.join("history-repair"),
        &recovery.join(OPERATIONS_DIR).join(HISTORY_REPAIR_DIR),
    )?;
    Ok(())
}

fn migrate_directory(source: &Path, destination: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    let source_metadata = fs::symlink_metadata(source).map_err(io_error)?;
    if !source_metadata.is_dir() || source_metadata.file_type().is_symlink() {
        return Err(LocalPoolError::new(
            ErrorCode::Io,
            format!(
                "recovery path must be a real directory: {}",
                source.display()
            ),
        ));
    }
    fs::create_dir_all(destination).map_err(io_error)?;
    for entry in fs::read_dir(source).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let source_entry = entry.path();
        let destination_entry = destination.join(entry.file_name());
        if destination_entry.exists() {
            continue;
        }
        fs::rename(&source_entry, &destination_entry).map_err(io_error)?;
    }
    if fs::read_dir(source).map_err(io_error)?.next().is_none() {
        fs::remove_dir(source).map_err(io_error)?;
    }
    Ok(())
}

fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::Io,
        format!("recovery layout migration failed: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::migrate_recovery_layout;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn migrates_legacy_recovery_directories_without_overwriting_conflicts() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-layout-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let legacy = root.join("recovery").join("profiles");
        let current = root.join("recovery").join("applications").join("chatgpt");
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(&current).unwrap();
        fs::write(legacy.join("moved.json"), "legacy").unwrap();
        fs::write(legacy.join("conflict.json"), "legacy-value").unwrap();
        fs::write(current.join("conflict.json"), "current-value").unwrap();

        migrate_recovery_layout(&root).unwrap();

        assert_eq!(
            fs::read_to_string(current.join("moved.json")).unwrap(),
            "legacy"
        );
        assert_eq!(
            fs::read_to_string(current.join("conflict.json")).unwrap(),
            "current-value"
        );
        assert!(legacy.join("conflict.json").exists());
        migrate_recovery_layout(&root).unwrap();
        assert!(legacy.join("conflict.json").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
