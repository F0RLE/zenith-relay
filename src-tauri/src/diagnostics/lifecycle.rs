use super::persist;
use super::{
    breadcrumb, record_error, record_operation, DEBUG_ENABLED, LOGS_DIRECTORY,
    PANIC_HOOK_INSTALLED, SESSION_ACTIVE, SESSION_MARKER_FILE,
};
use std::fs;
use std::panic;
use std::path::Path;
use std::sync::atomic::Ordering;

/// Install the process-wide panic hook before Tauri starts constructing the
/// application.  The hook is intentionally best effort: diagnostics must
/// never turn a useful panic into a second panic.
pub(crate) fn install_panic_hook() {
    if PANIC_HOOK_INSTALLED.set(()).is_err() {
        return;
    }
    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        persist::write_panic_report(info);
        previous_hook(info);
    }));
}

/// Point diagnostics at the same branded root as the rest of Relay.  This is
/// called during Tauri setup, while the fallback root lets startup panics be
/// recorded before the AppHandle exists.
pub(crate) fn initialize(root: &Path) {
    let state = persist::state();
    *zenith_relay_core::poison::mutex(&state.root) = root.to_path_buf();
    let layout_ready = persist::ensure_layout(root);
    let debug_marker = layout_ready.then(|| persist::read_debug_marker(root));
    let debug_enabled = match debug_marker
        .as_ref()
        .and_then(|debug_marker_result| debug_marker_result.as_ref().ok())
    {
        Some(enabled) => *enabled,
        None => false,
    };
    DEBUG_ENABLED.store(debug_enabled, Ordering::Release);
    if debug_marker.is_some_and(|debug_marker_result| debug_marker_result.is_err()) {
        record_error(
            "desktop",
            Some("diagnostic_debug_marker_invalid"),
            "diagnostic debug setting could not be read; detailed logging is disabled",
            &[],
        );
    }
    // Read the previous durable stage before replacing the session marker. A
    // forced process termination leaves both files behind, which lets the
    // next launch explain exactly where the interrupted operation stopped.
    let previous_stage = layout_ready
        .then(|| persist::read_latest_stage(root))
        .flatten();
    let marker_state = layout_ready
        .then(|| persist::begin_session_marker(root))
        .flatten();
    SESSION_ACTIVE.store(marker_state.is_some(), Ordering::Release);
    if marker_state == Some(true) {
        let mut diagnostic_details = Vec::new();
        if let Some(stage) = previous_stage {
            diagnostic_details.push(("previous_operation", stage.operation));
            diagnostic_details.push(("previous_stage", stage.stage));
            diagnostic_details.push(("previous_stage_at", stage.timestamp));
        }
        record_error(
            "desktop",
            Some("unclean_exit"),
            "previous Relay session ended before a clean shutdown",
            &diagnostic_details,
        );
    }
    breadcrumb("desktop", "diagnostics_ready", &[]);
}

/// Close the diagnostic session marker. If the process is terminated before
/// this function runs, the marker remains and the next launch records an
/// `unclean_exit` event instead of silently losing the crash signal.
pub(crate) fn shutdown() {
    if !SESSION_ACTIVE.swap(false, Ordering::AcqRel) {
        return;
    }
    let root = persist::root_path();
    let marker = root.join(LOGS_DIRECTORY).join(SESSION_MARKER_FILE);
    let removed = fs::symlink_metadata(&marker).is_ok_and(|metadata| {
        metadata.is_file() && !metadata.file_type().is_symlink() && fs::remove_file(&marker).is_ok()
    });
    if removed {
        record_operation("desktop", "clean_shutdown", &[]);
        persist::clear_last_stages(&root);
    }
}
