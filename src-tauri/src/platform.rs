use serde::Serialize;
use std::{
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager};

const RELAY_DIRECTORY: &str = "Zenith Relay";
const LEGACY_WEBVIEW_DIRECTORY: &str = "com.zenith.codex";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCapabilities {
    pub native_secret_storage: bool,
    pub oauth_browser: bool,
    pub process_detection: bool,
    pub folder_open: bool,
    pub autostart: bool,
    pub background_runtime: bool,
}

pub fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

pub fn system_locale() -> Option<String> {
    sys_locale::get_locale().or_else(|| env::var("LANG").ok())
}

pub fn system_language_is_russian() -> bool {
    system_locale()
        .map(|locale| locale.to_lowercase().starts_with("ru"))
        .unwrap_or(false)
}

pub fn ui_text(en: &'static str, ru: &'static str) -> &'static str {
    if system_language_is_russian() {
        ru
    } else {
        en
    }
}

pub fn default_codex_home() -> PathBuf {
    user_home().join(".codex")
}

fn user_home() -> PathBuf {
    if cfg!(windows) {
        env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    } else {
        env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

fn roaming_app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|err| format!("failed to resolve app data directory: {err}"))
}

fn local_app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map_err(|err| format!("failed to resolve local app data directory: {err}"))
}

pub fn legacy_roaming_local_pool_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(roaming_app_data_dir(app)?.join("local-pool"))
}

pub fn legacy_local_pool_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(local_app_data_dir(app)?.join("local-pool"))
}

pub fn legacy_app_local_dir(app: &AppHandle) -> Result<PathBuf, String> {
    local_app_data_dir(app)
}

pub fn relay_dir(app: &AppHandle) -> Result<PathBuf, String> {
    relay_dir_from_local(&local_app_data_dir(app)?)
}

fn relay_dir_from_local(local_app_data: &Path) -> Result<PathBuf, String> {
    local_app_data
        .parent()
        .map(|parent| parent.join(RELAY_DIRECTORY))
        .ok_or_else(|| "local app data directory has no parent".to_string())
}

pub fn webview_cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(webview_cache_dir_from_root(&relay_dir(app)?))
}

fn webview_cache_dir_from_root(root: &Path) -> PathBuf {
    root.join("cache")
        .join(LEGACY_WEBVIEW_DIRECTORY)
        .join("EBWebView")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StorageMigration {
    Current,
    Moved,
    Conflict,
}

pub(crate) fn migrate_directory(legacy: &Path, target: &Path) -> Result<StorageMigration, String> {
    if legacy == target {
        validate_storage_directory(target)?;
        return Ok(StorageMigration::Current);
    }

    let legacy_exists = validate_storage_directory(legacy)?;
    let target_exists = validate_storage_directory(target)?;
    if target_exists {
        return Ok(if legacy_exists {
            StorageMigration::Conflict
        } else {
            StorageMigration::Current
        });
    }
    if !legacy_exists {
        return Ok(StorageMigration::Current);
    }

    let parent = target
        .parent()
        .ok_or_else(|| format!("local app data path has no parent: {}", target.display()))?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create local app data directory {}: {error}",
            parent.display()
        )
    })?;
    fs::rename(legacy, target).map_err(|error| {
        format!(
            "failed to move local data from {} to {}: {error}",
            legacy.display(),
            target.display()
        )
    })?;
    Ok(StorageMigration::Moved)
}

fn validate_storage_directory(path: &Path) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "failed to inspect local data path {}: {error}",
                path.display()
            ))
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "local data path must not be a symbolic link: {}",
            path.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "local data path is not a directory: {}",
            path.display()
        ));
    }
    Ok(true)
}

pub fn capabilities() -> PlatformCapabilities {
    PlatformCapabilities {
        native_secret_storage: true,
        oauth_browser: true,
        process_detection: true,
        folder_open: true,
        autostart: false,
        background_runtime: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branded_data_root_owns_the_legacy_named_webview_cache() {
        let legacy = PathBuf::from("local").join("com.zenith.codex");
        let root = relay_dir_from_local(&legacy).unwrap();
        assert_eq!(root, PathBuf::from("local").join("Zenith Relay"));
        assert_eq!(
            webview_cache_dir_from_root(&root),
            root.join("cache/com.zenith.codex/EBWebView")
        );
    }

    #[test]
    fn migration_leaves_missing_legacy_storage_uncreated() {
        let root = temp_root("missing");
        let legacy = root.join("roaming/local-pool");
        let target = root.join("local/local-pool");

        assert_eq!(
            migrate_directory(&legacy, &target).unwrap(),
            StorageMigration::Current
        );
        assert!(!legacy.exists());
        assert!(!target.exists());
        cleanup(root);
    }

    #[test]
    fn migration_moves_legacy_storage_without_copying() {
        let root = temp_root("move");
        let legacy = root.join("roaming/local-pool");
        let target = root.join("local/local-pool");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("metadata.json"), "legacy").unwrap();

        assert_eq!(
            migrate_directory(&legacy, &target).unwrap(),
            StorageMigration::Moved
        );
        assert!(!legacy.exists());
        assert_eq!(
            fs::read_to_string(target.join("metadata.json")).unwrap(),
            "legacy"
        );
        cleanup(root);
    }

    #[test]
    fn migration_does_not_merge_conflicting_stores() {
        let root = temp_root("conflict");
        let legacy = root.join("roaming/local-pool");
        let target = root.join("local/local-pool");
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(legacy.join("origin"), "legacy").unwrap();
        fs::write(target.join("origin"), "current").unwrap();

        assert_eq!(
            migrate_directory(&legacy, &target).unwrap(),
            StorageMigration::Conflict
        );
        assert_eq!(fs::read_to_string(legacy.join("origin")).unwrap(), "legacy");
        assert_eq!(
            fs::read_to_string(target.join("origin")).unwrap(),
            "current"
        );
        cleanup(root);
    }

    #[test]
    fn migration_rejects_symbolic_link_storage() {
        let root = temp_root("symlink");
        let real = root.join("real");
        let legacy = root.join("roaming/local-pool");
        let target = root.join("local/local-pool");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        if create_directory_symlink(&real, &legacy).is_err() {
            cleanup(root);
            return;
        }

        let error = migrate_directory(&legacy, &target).unwrap_err();
        assert!(error.contains("symbolic link"));
        assert!(!target.exists());
        cleanup(root);
    }

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "zenith-relay-storage-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    fn cleanup(root: PathBuf) {
        if root.exists() {
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(unix)]
    fn create_directory_symlink(original: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(original, link)
    }

    #[cfg(windows)]
    fn create_directory_symlink(original: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(original, link)
    }
}
