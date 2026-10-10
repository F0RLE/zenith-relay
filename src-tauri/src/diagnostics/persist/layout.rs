use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use super::super::*;

pub(in crate::diagnostics) fn state() -> &'static DiagnosticState {
    STATE.get_or_init(|| DiagnosticState {
        root: Mutex::new(fallback_root()),
        write_lock: Mutex::new(()),
        breadcrumb: Mutex::new(None),
    })
}

pub(in crate::diagnostics) fn root_path() -> PathBuf {
    let state = state();
    zenith_relay_core::poison::try_mutex(&state.root)
        .map(|root| root.clone())
        .unwrap_or_else(fallback_root)
}

fn fallback_root() -> PathBuf {
    #[cfg(debug_assertions)]
    if let Some(value) = env::var_os("ZENITH_RELAY_DEV_DATA_DIR") {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return path;
        }
    }

    if cfg!(target_os = "windows") {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("Zenith Relay")
    } else if cfg!(target_os = "macos") {
        env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("Library")
            .join("Application Support")
            .join("Zenith Relay")
    } else {
        env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
            .unwrap_or_else(env::temp_dir)
            .join("Zenith Relay")
    }
}

pub(in crate::diagnostics) fn read_debug_marker(root: &Path) -> Result<bool, String> {
    let marker = root.join(LOGS_DIRECTORY).join(DEBUG_MARKER_FILE);
    match fs::symlink_metadata(marker) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err("diagnostic debug marker is unsafe".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("diagnostic debug marker could not be read".to_string()),
    }
}

pub(in crate::diagnostics) fn ensure_layout(root: &Path) -> bool {
    if !ensure_real_directory(root) {
        return false;
    }
    let logs = root.join(LOGS_DIRECTORY);
    for directory in [
        logs.clone(),
        logs.join(ERRORS_DIRECTORY),
        logs.join(CRASHES_DIRECTORY),
        logs.join(OPERATIONS_DIRECTORY),
    ] {
        if !ensure_real_directory(&directory) {
            return false;
        }
    }
    let readme = logs.join("README.txt");
    let readme_is_real_file = fs::symlink_metadata(&readme)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink());
    if !readme_is_real_file {
        let content = concat!(
            "Zenith Relay diagnostics\n",
            "========================\n",
            "errors/      redacted native and renderer error events (JSONL)\n",
            "crashes/     panic reports with the last safe operation breadcrumb\n",
            "operations/  short lifecycle records useful for import/runtime debugging (JSONL)\n",
            "last-stage-* is a bounded marker for the last interrupted operation stage.\n",
            "debug.enabled enables the detailed operation stream and is off by default.\n",
            "session.active is removed on clean exit; its presence marks an interrupted run.\n",
            "\n",
            "Secrets, cookies, prompts, responses, and raw account identities are not stored here.\n",
            "Attach only the relevant redacted file when reporting a problem.\n",
        );
        let _ = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(readme)
            .and_then(|mut file| file.write_all(content.as_bytes()));
    }
    true
}

pub(in crate::diagnostics::persist) fn ensure_real_directory(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) => metadata.is_dir() && !metadata.file_type().is_symlink(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if fs::create_dir_all(path).is_err() {
                return false;
            }
            fs::symlink_metadata(path)
                .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        }
        Err(_) => false,
    }
}

pub(in crate::diagnostics::persist) fn prune_files(directory: &Path, prefix: &str, keep: usize) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut files = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let file_name = path.file_name()?.to_str()?;
            let metadata = entry.file_type().ok()?;
            if !metadata.is_file() || metadata.is_symlink() || !file_name.starts_with(prefix) {
                return None;
            }
            Some((file_name.to_string(), path))
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let remove_count = files.len().saturating_sub(keep);
    for (_, path) in files.into_iter().take(remove_count) {
        let _ = fs::remove_file(path);
    }
}
