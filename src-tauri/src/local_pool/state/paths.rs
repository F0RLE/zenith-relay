use super::DesktopState;
use crate::{
    local_pool::error::{ErrorCode, LocalPoolError, Result},
    storage_paths::StoragePaths,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

const APPLICATIONS_DIR: &str = "applications";
const CHATGPT_DIR: &str = "chatgpt";
const OPERATIONS_DIR: &str = "operations";
const HISTORY_REPAIR_DIR: &str = "history-repair";
const LEGACY_KEYRING_MIGRATION_MARKER: &str = ".legacy-keyring-migrated-v1";
const LEGACY_WEBVIEW_DIRECTORY: &str = "com.zenith.codex";

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

/// Moves Relay-owned artifacts from previous flat layouts into the current
/// categorized layout. Existing recovery entries win; a conflicting durable
/// file aborts the migration rather than choosing one copy and losing state.
/// Nothing outside Relay's own application root is considered here.
pub(crate) fn migrate_storage_layout(root: &Path) -> Result<()> {
    let paths = StoragePaths::from_root(root);
    super::super::store::migrate_database_layout(root)?;
    migrate_recovery_layout(&paths)?;
    migrate_durable_files(&paths)?;
    migrate_webview_layout(&paths)?;
    migrate_deployment_layout(&paths)?;
    Ok(())
}

fn migrate_recovery_layout(paths: &StoragePaths) -> Result<()> {
    let recovery = paths.recovery_root();
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

fn migrate_durable_files(paths: &StoragePaths) -> Result<()> {
    for (source, destination) in [
        (
            paths.data_root().join("secrets.enc"),
            paths.vault_root().join("secrets.enc"),
        ),
        (
            paths.data_root().join("secrets.enc.bak"),
            paths.vault_root().join("secrets.enc.bak"),
        ),
        (
            paths.data_root().join("litellm-prices.json"),
            paths.pricing_catalog_file(),
        ),
        (
            paths.data_root().join("models-dev.json"),
            paths.model_metadata_catalog_file(),
        ),
    ] {
        migrate_regular_file(&source, &destination)?;
    }
    migrate_keyring_marker(paths)
}

fn migrate_keyring_marker(paths: &StoragePaths) -> Result<()> {
    let source = paths.data_root().join(LEGACY_KEYRING_MIGRATION_MARKER);
    let source_metadata = match fs::symlink_metadata(&source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };
    if !source_metadata.is_file()
        || source_metadata.file_type().is_symlink()
        || source_metadata.len() != 0
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "legacy keyring migration marker is unsafe: {}",
                source.display()
            ),
        ));
    }

    let destination = paths.keyring_migration_marker();
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(&source).map_err(io_error)
        }
        Ok(_) => Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "keyring migration marker destination is unsafe: {}",
                destination.display()
            ),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure_destination_parent(&destination)?;
            fs::rename(&source, &destination).map_err(io_error)
        }
        Err(error) => Err(io_error(error)),
    }
}

fn migrate_webview_layout(paths: &StoragePaths) -> Result<()> {
    move_directory_if_destination_absent(
        &paths.cache_root().join(LEGACY_WEBVIEW_DIRECTORY),
        &paths.webview_root(),
    )
}

fn migrate_deployment_layout(paths: &StoragePaths) -> Result<()> {
    migrate_directory(
        &paths.cache_root().join("deployments"),
        &paths.exports_root().join("deployments"),
    )
}

fn migrate_directory(source: &Path, destination: &Path) -> Result<()> {
    match fs::symlink_metadata(source) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    }
    ensure_real_directory(source, "legacy storage path")?;
    ensure_real_directory(destination, "storage destination")?;
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

fn move_directory_if_destination_absent(source: &Path, destination: &Path) -> Result<()> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!("legacy storage path is unsafe: {}", source.display()),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    }
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!("storage destination is unsafe: {}", destination.display()),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    ensure_destination_parent(destination)?;
    fs::rename(source, destination).map_err(io_error)
}

fn migrate_regular_file(source: &Path, destination: &Path) -> Result<()> {
    let source_metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };
    if !source_metadata.is_file() || source_metadata.file_type().is_symlink() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("legacy storage file is unsafe: {}", source.display()),
        ));
    }
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "both legacy and categorized storage files exist: {} and {}",
                    source.display(),
                    destination.display()
                ),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    ensure_destination_parent(destination)?;
    fs::rename(source, destination).map_err(io_error)
}

fn ensure_destination_parent(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("storage path has no parent: {}", path.display()),
        )
    })?;
    ensure_real_directory(parent, "storage destination parent")
}

fn ensure_real_directory(path: &Path, description: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!("{description} must be a real directory: {}", path.display()),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    fs::create_dir_all(path).map_err(io_error)?;
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("{description} must be a real directory: {}", path.display()),
        ))
    }
}

fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::Io,
        format!("storage layout migration failed: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::migrate_storage_layout;
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

        migrate_storage_layout(&root).unwrap();

        assert_eq!(
            fs::read_to_string(current.join("moved.json")).unwrap(),
            "legacy"
        );
        assert_eq!(
            fs::read_to_string(current.join("conflict.json")).unwrap(),
            "current-value"
        );
        assert!(legacy.join("conflict.json").exists());
        migrate_storage_layout(&root).unwrap();
        assert!(legacy.join("conflict.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_relay_owned_files_into_separate_categories() {
        let root = temp_root("categorized");
        let data = root.join("data");
        let cache = root.join("cache");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(cache.join("com.zenith.codex")).unwrap();
        fs::create_dir_all(cache.join("deployments").join("deployment-a")).unwrap();
        fs::write(data.join("secrets.enc"), "synthetic-encrypted-data").unwrap();
        fs::write(data.join("secrets.enc.bak"), "synthetic-backup").unwrap();
        fs::write(data.join("litellm-prices.json"), "prices").unwrap();
        fs::write(data.join("models-dev.json"), "metadata").unwrap();
        fs::write(data.join(".legacy-keyring-migrated-v1"), "").unwrap();
        fs::write(cache.join("com.zenith.codex").join("profile"), "webview").unwrap();
        fs::write(
            cache
                .join("deployments")
                .join("deployment-a")
                .join("compose.yaml"),
            "bundle",
        )
        .unwrap();

        migrate_storage_layout(&root).unwrap();

        assert_eq!(
            fs::read_to_string(root.join("data/vault/secrets.enc")).unwrap(),
            "synthetic-encrypted-data"
        );
        assert!(root.join("data/vault/secrets.enc.bak").exists());
        assert!(root.join("data/catalogs/litellm-prices.json").exists());
        assert!(root.join("data/catalogs/models-dev.json").exists());
        assert!(root.join("data/migrations/keyring-v1.complete").exists());
        assert!(!root.join("data/.legacy-keyring-migrated-v1").exists());
        assert!(root.join("cache/webview/profile").exists());
        assert!(!root.join("cache/com.zenith.codex").exists());
        assert!(root
            .join("exports/deployments/deployment-a/compose.yaml")
            .exists());
        assert!(!root.join("cache/deployments").exists());

        migrate_storage_layout(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preserves_both_copies_when_a_durable_migration_conflicts() {
        let root = temp_root("conflict");
        let data = root.join("data");
        fs::create_dir_all(data.join("catalogs")).unwrap();
        fs::write(data.join("litellm-prices.json"), "legacy").unwrap();
        fs::write(data.join("catalogs/litellm-prices.json"), "current").unwrap();

        assert!(migrate_storage_layout(&root).is_err());
        assert_eq!(
            fs::read_to_string(data.join("litellm-prices.json")).unwrap(),
            "legacy"
        );
        assert_eq!(
            fs::read_to_string(data.join("catalogs/litellm-prices.json")).unwrap(),
            "current"
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "zenith-relay-layout-{label}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
