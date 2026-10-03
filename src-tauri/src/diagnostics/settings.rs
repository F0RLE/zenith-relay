use super::persist;
use super::record::record_operation;
use super::{
    DiagnosticPaths, DiagnosticSettings, CRASHES_DIRECTORY, DEBUG_ENABLED, DEBUG_MARKER_FILE,
    ERRORS_DIRECTORY, LOGS_DIRECTORY, OPERATIONS_DIRECTORY,
};
use std::fs::{self, OpenOptions};
use std::sync::atomic::Ordering;

/// Return the user-visible diagnostics locations without exposing any file
/// contents to the renderer.
pub(crate) fn paths() -> DiagnosticPaths {
    let root = persist::root_path();
    let logs = root.join(LOGS_DIRECTORY);
    DiagnosticPaths {
        logs_path: logs.to_string_lossy().into_owned(),
        error_logs_path: logs.join(ERRORS_DIRECTORY).to_string_lossy().into_owned(),
        crash_logs_path: logs.join(CRASHES_DIRECTORY).to_string_lossy().into_owned(),
        operation_logs_path: logs
            .join(OPERATIONS_DIRECTORY)
            .to_string_lossy()
            .into_owned(),
    }
}

pub(crate) fn settings() -> DiagnosticSettings {
    DiagnosticSettings {
        debug_enabled: is_debug_enabled(),
    }
}

/// Return whether verbose operation diagnostics are enabled for this Relay
/// installation. Errors and crash reports intentionally do not use this flag.
pub(crate) fn is_debug_enabled() -> bool {
    DEBUG_ENABLED.load(Ordering::Acquire)
}

/// Persist the opt-in diagnostic verbosity setting as a marker owned by the
/// Relay logs directory. The marker contains no user data and is created with
/// `create_new` so a symlink or other unexpected file can never be followed.
pub(crate) fn set_debug_enabled(enabled: bool) -> Result<(), String> {
    let state = persist::state();
    let _guard = zenith_relay_core::poison::mutex(&state.write_lock);
    let root = persist::root_path();
    if !persist::ensure_layout(&root) {
        return Err("diagnostic log directory is unavailable".to_string());
    }
    let marker = root.join(LOGS_DIRECTORY).join(DEBUG_MARKER_FILE);
    if enabled {
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err("diagnostic debug marker is unsafe".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&marker)
                    .map_err(|_| "diagnostic debug setting could not be saved".to_string())?;
            }
            Err(_) => return Err("diagnostic debug setting could not be read".to_string()),
        }
    } else {
        match fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                fs::remove_file(&marker)
                    .map_err(|_| "diagnostic debug setting could not be saved".to_string())?;
            }
            Ok(_) => return Err("diagnostic debug marker is unsafe".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("diagnostic debug setting could not be read".to_string()),
        }
    }
    DEBUG_ENABLED.store(enabled, Ordering::Release);
    drop(_guard);
    if enabled {
        record_operation("diagnostics", "debug_enabled", &[]);
    }
    Ok(())
}
