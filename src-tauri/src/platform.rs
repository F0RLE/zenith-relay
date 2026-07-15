use serde::Serialize;
use std::{env, path::PathBuf};
use tauri::{AppHandle, Manager};

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

pub fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|err| format!("failed to resolve app data directory: {err}"))
}

pub fn local_pool_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("local-pool"))
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
