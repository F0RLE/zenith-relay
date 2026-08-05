use serde::Serialize;
use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager};

const RELAY_DIRECTORY: &str = "Zenith Relay";
const WEBVIEW_DIRECTORY: &str = "com.zenith.codex";

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
    resolve_codex_home().unwrap_or_else(|error| panic!("{error}"))
}

pub fn resolve_codex_home() -> Result<PathBuf, String> {
    resolve_codex_home_from(env::var_os("CODEX_HOME"), user_home())
}

fn resolve_codex_home_from(value: Option<OsString>, home: PathBuf) -> Result<PathBuf, String> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(home.join(".codex"));
    };
    let configured = PathBuf::from(value);
    let metadata = fs::metadata(&configured).map_err(|error| {
        format!(
            "CODEX_HOME must point to an existing directory ({}): {error}",
            configured.display()
        )
    })?;
    if !metadata.is_dir() {
        return Err(format!(
            "CODEX_HOME must point to a directory: {}",
            configured.display()
        ));
    }
    fs::canonicalize(&configured).map_err(|error| {
        format!(
            "failed to resolve CODEX_HOME {}: {error}",
            configured.display()
        )
    })
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

fn local_app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map_err(|error| format!("failed to resolve local app data directory: {error}"))
}

pub fn relay_dir(app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(root) = relay_dir_override(env::var_os("ZENITH_RELAY_DEV_DATA_DIR"))? {
        return Ok(root);
    }
    relay_dir_from_local(&local_app_data_dir(app)?)
}

#[cfg(debug_assertions)]
fn relay_dir_override(value: Option<std::ffi::OsString>) -> Result<Option<PathBuf>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let root = PathBuf::from(value);
    if !root.is_absolute() {
        return Err("ZENITH_RELAY_DEV_DATA_DIR must be an absolute path".to_string());
    }
    ensure_real_directory(&root)?;
    Ok(Some(root))
}

fn relay_dir_from_local(local_app_data: &Path) -> Result<PathBuf, String> {
    local_app_data
        .parent()
        .map(|parent| parent.join(RELAY_DIRECTORY))
        .ok_or_else(|| "local app data directory has no parent".to_string())
}

pub fn webview_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    webview_data_dir_from_root(&relay_dir(app)?)
}

fn webview_data_dir_from_root(root: &Path) -> Result<PathBuf, String> {
    let directory = root.join("cache").join(WEBVIEW_DIRECTORY);
    ensure_real_directory(&directory)?;
    if cfg!(windows) {
        Ok(directory)
    } else {
        let profile = directory.join("EBWebView");
        ensure_real_directory(&profile)?;
        Ok(profile)
    }
}

fn ensure_real_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| {
        format!(
            "failed to create local data directory {}: {error}",
            path.display()
        )
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "local data path must be a real directory: {}",
            path.display()
        ));
    }
    Ok(())
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
    fn branded_data_and_webview_paths_are_stable() {
        let local = PathBuf::from("local").join("com.zenith.codex");
        let root = relay_dir_from_local(&local).unwrap();
        assert_eq!(root, PathBuf::from("local").join("Zenith Relay"));
        let expected = if cfg!(windows) {
            root.join("cache/com.zenith.codex")
        } else {
            root.join("cache/com.zenith.codex/EBWebView")
        };
        assert_eq!(webview_data_dir_from_root(&root).unwrap(), expected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_home_uses_an_existing_override_and_canonicalizes_it() {
        let root = std::env::temp_dir().join(format!("zenith-codex-home-{}", uuid::Uuid::new_v4()));
        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();
        let resolved = resolve_codex_home_from(Some(nested.clone().into()), root.clone()).unwrap();
        assert_eq!(resolved, fs::canonicalize(nested).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_home_rejects_a_missing_or_file_override() {
        let root = std::env::temp_dir().join(format!("zenith-codex-home-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        assert!(resolve_codex_home_from(Some(root.join("missing").into()), root.clone()).is_err());
        let file = root.join("file");
        fs::write(&file, "x").unwrap();
        assert!(resolve_codex_home_from(Some(file.into()), root.clone()).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_home_falls_back_only_when_override_is_empty_or_unset() {
        let home = PathBuf::from("C:/Users/test");
        assert_eq!(
            resolve_codex_home_from(None, home.clone()).unwrap(),
            home.join(".codex")
        );
        assert_eq!(
            resolve_codex_home_from(Some(OsString::new()), home.clone()).unwrap(),
            home.join(".codex")
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_data_override_requires_an_absolute_path() {
        assert!(relay_dir_override(Some("relative".into())).is_err());
        assert_eq!(relay_dir_override(None).unwrap(), None);
    }
}
