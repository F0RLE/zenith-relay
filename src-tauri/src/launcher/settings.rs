use crate::storage_paths::StoragePaths;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use tauri_plugin_dialog::DialogExt;

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Application {
    Chatgpt,
    ClaudeDesktop,
    Opencode,
}

impl Application {
    fn is_desktop_path(self, path: &Path) -> bool {
        match self {
            Self::Chatgpt => path.file_name().is_some_and(|name| {
                super::process::is_codex_process_identity(
                    &name.to_string_lossy(),
                    Some(path),
                    &[] as &[&str],
                )
            }),
            Self::ClaudeDesktop => super::claude_desktop::is_desktop_executable(path),
            Self::Opencode => super::opencode_desktop::is_opencode_desktop_path(path),
        }
    }

    fn detected_path(self) -> Option<String> {
        match self {
            Self::Chatgpt => super::process::detected_chatgpt_launch_path(),
            Self::ClaudeDesktop => super::claude_desktop::detected_launch_path(),
            Self::Opencode => super::opencode_desktop::resolve_opencode_desktop_command()
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LaunchPathSetting {
    path: Option<String>,
    detected_path: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preferences {
    #[serde(default)]
    launch_paths: BTreeMap<Application, PathBuf>,
}

struct PreferenceStore {
    path: PathBuf,
    lock: Mutex<()>,
}

static STORE: OnceLock<PreferenceStore> = OnceLock::new();

pub(crate) fn initialize(root: &Path) {
    let _ = STORE.set(PreferenceStore {
        path: StoragePaths::from_root(root).application_launch_settings_file(),
        lock: Mutex::new(()),
    });
}

fn store() -> Result<&'static PreferenceStore> {
    STORE
        .get()
        .ok_or_else(|| "Application launch settings are unavailable".to_string())
}

impl PreferenceStore {
    fn read(&self) -> Result<Preferences> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Preferences::default());
            }
            Err(_) => return Err(settings_io_error()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 32 * 1024 {
            return Err(settings_io_error());
        }
        serde_json::from_slice(&fs::read(&self.path).map_err(|_| settings_io_error())?)
            .map_err(|_| settings_io_error())
    }

    fn path(&self, application: Application) -> Result<Option<PathBuf>> {
        let _guard = self.lock.lock().map_err(|_| settings_io_error())?;
        Ok(self.read()?.launch_paths.remove(&application))
    }

    fn set(&self, application: Application, path: Option<&str>) -> Result<Option<PathBuf>> {
        let path = path
            .filter(|path| !path.trim().is_empty())
            .map(|path| validate_path(application, path.trim()))
            .transpose()?;
        let _guard = self.lock.lock().map_err(|_| settings_io_error())?;
        let mut preferences = self.read()?;
        match &path {
            Some(path) => {
                preferences.launch_paths.insert(application, path.clone());
            }
            None => {
                preferences.launch_paths.remove(&application);
            }
        }
        let content = serde_json::to_vec_pretty(&preferences).map_err(|_| settings_io_error())?;
        let parent = self.path.parent().ok_or_else(settings_io_error)?;
        crate::platform::ensure_real_directory(parent).map_err(|_| settings_io_error())?;
        let temporary = parent.join(format!(".application-launch-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&content)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
            return Err(settings_io_error());
        }
        Ok(path)
    }
}

fn settings_io_error() -> String {
    "Application launch settings could not be read or saved".to_string()
}

fn validate_path(application: Application, input: &str) -> Result<PathBuf> {
    let invalid = || {
        "Select an existing desktop application, not a terminal CLI, shortcut, or command with arguments".to_string()
    };
    if input.len() > 8192 || input.chars().any(char::is_control) {
        return Err(invalid());
    }
    let path = PathBuf::from(input);
    if !path.is_absolute() {
        return Err(invalid());
    }
    #[cfg(target_os = "macos")]
    let path = if path.extension().is_some_and(|extension| extension == "app") {
        let executable = match application {
            Application::Chatgpt
                if path
                    .file_stem()
                    .is_some_and(|name| name.eq_ignore_ascii_case("Codex")) =>
            {
                "Codex"
            }
            Application::Chatgpt => "ChatGPT",
            Application::ClaudeDesktop => "Claude",
            Application::Opencode => "OpenCode",
        };
        path.join("Contents/MacOS").join(executable)
    } else {
        path
    };
    if !path.is_file() {
        return Err(invalid());
    }
    let canonical = fs::canonicalize(&path).map_err(|_| invalid())?;
    if !application.is_desktop_path(&canonical) {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&canonical)
            .map_err(|_| invalid())?
            .permissions()
            .mode()
            & 0o111
            == 0
        {
            return Err(invalid());
        }
    }
    Ok(path)
}

pub(super) fn configured_path(
    application: Application,
) -> std::result::Result<Option<PathBuf>, String> {
    let Some(store) = STORE.get() else {
        return Ok(None);
    };
    configured_path_from(store, application)
}

fn configured_path_from(
    store: &PreferenceStore,
    application: Application,
) -> Result<Option<PathBuf>> {
    store
        .path(application)?
        .map(|path| {
            let path = validate_path(application, &path.to_string_lossy())?;
            fs::canonicalize(path).map_err(|_| settings_io_error())
        })
        .transpose()
}

#[tauri::command]
pub(crate) async fn get_application_launch_path(
    application: Application,
) -> Result<LaunchPathSetting> {
    tauri::async_runtime::spawn_blocking(move || {
        Ok(LaunchPathSetting {
            path: store()?
                .path(application)?
                .map(|path| path.to_string_lossy().into_owned()),
            detected_path: application.detected_path(),
        })
    })
    .await
    .map_err(|_| settings_io_error())?
}

#[tauri::command]
pub(crate) async fn set_application_launch_path(
    application: Application,
    path: Option<String>,
) -> Result<LaunchPathSetting> {
    tauri::async_runtime::spawn_blocking(move || {
        Ok(LaunchPathSetting {
            path: store()?
                .set(application, path.as_deref())?
                .map(|path| path.to_string_lossy().into_owned()),
            detected_path: application.detected_path(),
        })
    })
    .await
    .map_err(|_| settings_io_error())?
}

#[tauri::command]
pub(crate) async fn pick_application_launch_path(
    app: tauri::AppHandle,
    application: Application,
) -> Result<Option<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        let picker = app.dialog().file();
        #[cfg(target_os = "windows")]
        let picker = picker.add_filter("Desktop application", &["exe"]);
        #[cfg(target_os = "macos")]
        let picker = picker.add_filter("Application", &["app"]);
        let Some(path) = picker.blocking_pick_file() else {
            return Ok(None);
        };
        let path = path
            .into_path()
            .map_err(|_| "Selected application path is invalid".to_string())?;
        let path = validate_path(application, &path.to_string_lossy())?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
    .map_err(|_| settings_io_error())?
}

#[cfg(test)]
mod tests;
