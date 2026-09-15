//! Local-first diagnostics for the desktop application.
//!
//! Diagnostics are deliberately kept outside the database and outside the
//! encrypted vault.  They are short, structured, redacted records which make
//! an otherwise opaque renderer/native failure useful to a person debugging
//! their own Relay installation.  No request body, prompt, response, cookie,
//! credential, or raw account identity is written here.

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env,
    fs::{self, OpenOptions},
    io::Write,
    panic::{self, PanicHookInfo},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

const LOGS_DIRECTORY: &str = "logs";
const ERRORS_DIRECTORY: &str = "errors";
const CRASHES_DIRECTORY: &str = "crashes";
const OPERATIONS_DIRECTORY: &str = "operations";
const DEBUG_MARKER_FILE: &str = "debug.enabled";
const LAST_STAGE_PREFIX: &str = "last-stage-";
const MAX_EVENT_BYTES: usize = 16 * 1024;
const MAX_STAGE_BYTES: usize = 8 * 1024;
const MAX_LOG_FILE_BYTES: u64 = 1_048_576;
const MAX_RETAINED_ERROR_FILES: usize = 14;
const MAX_RETAINED_OPERATION_FILES: usize = 14;
const MAX_RETAINED_CRASH_FILES: usize = 20;
const MAX_RETAINED_STAGE_FILES: usize = 2;
const MAX_TEXT_BYTES: usize = 2_000;
const MAX_STACK_BYTES: usize = 24_000;
const REDACTED: &str = "[redacted]";
const SESSION_MARKER_FILE: &str = "session.active";

static STATE: OnceLock<DiagnosticState> = OnceLock::new();
static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();
static CRASH_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);
// Detailed operation breadcrumbs are opt-in. Errors and crash reports remain
// enabled regardless of this flag so a normal installation stays quiet while
// still retaining the information needed to diagnose a failure.
static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

struct DiagnosticState {
    root: Mutex<PathBuf>,
    write_lock: Mutex<()>,
    breadcrumb: Mutex<Option<Breadcrumb>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Breadcrumb {
    timestamp: String,
    operation: String,
    stage: String,
    details: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct LogEvent {
    timestamp: String,
    schema_version: u8,
    app_version: &'static str,
    platform: &'static str,
    level: &'static str,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    message: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    details: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrontendDiagnosticInput {
    pub source: String,
    pub message: String,
    #[serde(default)]
    pub operation: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub stack: Option<String>,
    #[serde(default)]
    pub fatal: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticPaths {
    pub logs_path: String,
    pub error_logs_path: String,
    pub crash_logs_path: String,
    pub operation_logs_path: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSettings {
    pub debug_enabled: bool,
}

/// Install the process-wide panic hook before Tauri starts constructing the
/// application.  The hook is intentionally best effort: diagnostics must
/// never turn a useful panic into a second panic.
pub(crate) fn install_panic_hook() {
    if PANIC_HOOK_INSTALLED.set(()).is_err() {
        return;
    }
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        write_panic_report(info);
        previous(info);
    }));
}

/// Point diagnostics at the same branded root as the rest of Relay.  This is
/// called during Tauri setup, while the fallback root lets startup panics be
/// recorded before the AppHandle exists.
pub(crate) fn initialize(root: &Path) {
    let state = state();
    match state.root.lock() {
        Ok(mut current) => *current = root.to_path_buf(),
        Err(poisoned) => *poisoned.into_inner() = root.to_path_buf(),
    }
    let layout_ready = ensure_layout(root);
    let debug_marker = layout_ready.then(|| read_debug_marker(root));
    let debug_enabled = match debug_marker
        .as_ref()
        .and_then(|result| result.as_ref().ok())
    {
        Some(enabled) => *enabled,
        None => false,
    };
    DEBUG_ENABLED.store(debug_enabled, Ordering::Release);
    if debug_marker.is_some_and(|result| result.is_err()) {
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
    let previous_stage = layout_ready.then(|| read_latest_stage(root)).flatten();
    let marker_state = layout_ready.then(|| begin_session_marker(root)).flatten();
    SESSION_ACTIVE.store(marker_state.is_some(), Ordering::Release);
    if marker_state == Some(true) {
        let mut details = Vec::new();
        if let Some(stage) = previous_stage {
            details.push(("previous_operation", stage.operation));
            details.push(("previous_stage", stage.stage));
            details.push(("previous_stage_at", stage.timestamp));
        }
        record_error(
            "desktop",
            Some("unclean_exit"),
            "previous Relay session ended before a clean shutdown",
            &details,
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
    let root = root_path();
    let marker = root.join(LOGS_DIRECTORY).join(SESSION_MARKER_FILE);
    let removed = fs::symlink_metadata(&marker).is_ok_and(|metadata| {
        metadata.is_file() && !metadata.file_type().is_symlink() && fs::remove_file(&marker).is_ok()
    });
    if removed {
        record_operation("desktop", "clean_shutdown", &[]);
        clear_last_stages(&root);
    }
}

/// Return the user-visible diagnostics locations without exposing any file
/// contents to the renderer.
pub(crate) fn paths() -> DiagnosticPaths {
    let root = root_path();
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
    let state = state();
    let _guard = state
        .write_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = root_path();
    if !ensure_layout(&root) {
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

/// Record a native operation breadcrumb.  The latest breadcrumb is copied to
/// a crash report, so a panic can be tied to the last known stage even when no
/// normal error response reaches the UI.
pub(crate) fn breadcrumb(operation: &str, stage: &str, details: &[(&str, String)]) {
    let state = state();
    let mut values = BTreeMap::new();
    for (key, value) in details {
        values.insert((*key).to_string(), safe_detail(value));
    }
    let value = Breadcrumb {
        timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        operation: safe_text(operation, 120),
        stage: safe_text(stage, 120),
        details: values,
    };
    if let Ok(mut current) = state.breadcrumb.lock() {
        *current = Some(value.clone());
    }
    if SESSION_ACTIVE.load(Ordering::Acquire) {
        persist_last_stage(&value);
    }
}

pub(crate) fn record_operation(operation: &str, outcome: &str, details: &[(&str, String)]) {
    breadcrumb(operation, outcome, details);
    if !is_debug_enabled() {
        return;
    }
    record_event(
        "operations",
        "info",
        "operation",
        Some(operation),
        None,
        outcome,
        details,
    );
}

pub(crate) fn record_error(
    operation: &str,
    code: Option<&str>,
    message: &str,
    details: &[(&str, String)],
) {
    breadcrumb(operation, "error", details);
    record_event(
        "errors",
        "error",
        "native_error",
        Some(operation),
        code,
        message,
        details,
    );
}

pub(crate) fn record_frontend_error(input: FrontendDiagnosticInput) {
    let operation = input.operation.as_deref().unwrap_or("renderer");
    let mut details = Vec::new();
    if let Some(stack) = input.stack.as_deref() {
        details.push(("stack", safe_text(stack, MAX_STACK_BYTES)));
    }
    details.push(("source", safe_text(&input.source, 120)));
    details.push(("fatal", input.fatal.to_string()));
    breadcrumb(operation, "renderer_error", &details);
    record_event(
        "errors",
        if input.fatal { "fatal" } else { "error" },
        "frontend_error",
        Some(operation),
        input.code.as_deref(),
        &input.message,
        &details,
    );
}

/// Hash an identifier before it is placed in a diagnostic.  A stable short
/// hash is enough to correlate repeated failures while keeping the identity
/// itself out of the log directory.
pub(crate) fn hash_identifier(value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(value.as_bytes());
    let encoded = hex::encode(digest.finalize());
    format!("id_{}", &encoded[..12])
}

#[tauri::command]
pub fn record_frontend_diagnostic(input: FrontendDiagnosticInput) {
    record_frontend_error(input);
}

#[tauri::command]
pub fn get_diagnostic_paths() -> DiagnosticPaths {
    paths()
}

#[tauri::command]
pub fn get_diagnostic_settings() -> DiagnosticSettings {
    settings()
}

#[tauri::command]
pub fn set_diagnostic_debug_mode(enabled: bool) -> Result<DiagnosticSettings, String> {
    set_debug_enabled(enabled)?;
    Ok(settings())
}

fn state() -> &'static DiagnosticState {
    STATE.get_or_init(|| DiagnosticState {
        root: Mutex::new(fallback_root()),
        write_lock: Mutex::new(()),
        breadcrumb: Mutex::new(None),
    })
}

fn root_path() -> PathBuf {
    let state = state();
    match state.root.try_lock() {
        Ok(root) => root.clone(),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().clone(),
        Err(std::sync::TryLockError::WouldBlock) => fallback_root(),
    }
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

fn read_debug_marker(root: &Path) -> Result<bool, String> {
    let marker = root.join(LOGS_DIRECTORY).join(DEBUG_MARKER_FILE);
    match fs::symlink_metadata(marker) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err("diagnostic debug marker is unsafe".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("diagnostic debug marker could not be read".to_string()),
    }
}

fn ensure_layout(root: &Path) -> bool {
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

/// Keep one small, redacted stage marker on disk. Unlike the in-memory
/// breadcrumb, this survives a hard process termination and is consumed on
/// the next launch when `session.active` indicates an interrupted run.
fn persist_last_stage(value: &Breadcrumb) {
    let state = state();
    let _guard = state
        .write_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = root_path();
    persist_last_stage_at(&root, value);
}

fn persist_last_stage_at(root: &Path, value: &Breadcrumb) {
    let Ok(mut line) = serde_json::to_vec(value) else {
        return;
    };
    line.push(b'\n');
    if line.len() > MAX_STAGE_BYTES {
        return;
    }
    if !ensure_layout(root) {
        return;
    }
    let directory = root.join(LOGS_DIRECTORY).join(OPERATIONS_DIRECTORY);
    if !ensure_real_directory(&directory) {
        return;
    }
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S-%3f").to_string();
    let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(format!("{LAST_STAGE_PREFIX}{timestamp}-{sequence:04}.json"));
    let Ok(mut file) = OpenOptions::new().write(true).create_new(true).open(path) else {
        return;
    };
    // The newline makes manual inspection pleasant while retaining the
    // strict size bound above.
    let _ = file.write_all(&line);
    let _ = file.flush();
    prune_files(&directory, LAST_STAGE_PREFIX, MAX_RETAINED_STAGE_FILES);
}

fn read_latest_stage(root: &Path) -> Option<Breadcrumb> {
    let directory = root.join(LOGS_DIRECTORY).join(OPERATIONS_DIRECTORY);
    let Ok(entries) = fs::read_dir(directory) else {
        return None;
    };
    let mut paths = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let metadata = entry.file_type().ok()?;
            if !metadata.is_file() || metadata.is_symlink() || !name.starts_with(LAST_STAGE_PREFIX)
            {
                return None;
            }
            Some((name.to_string(), path))
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| left.0.cmp(&right.0));
    let (_, path) = paths.pop()?;
    let metadata = fs::symlink_metadata(&path).ok()?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_STAGE_BYTES as u64
    {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let value = serde_json::from_slice::<Breadcrumb>(&bytes).ok()?;
    sanitize_breadcrumb(value)
}

fn sanitize_breadcrumb(mut value: Breadcrumb) -> Option<Breadcrumb> {
    if value.timestamp.len() > 64 {
        return None;
    }
    value.timestamp = safe_text(&value.timestamp, 64);
    value.operation = safe_text(&value.operation, 120);
    value.stage = safe_text(&value.stage, 120);
    if value.operation.is_empty() || value.stage.is_empty() {
        return None;
    }
    value.details = value
        .details
        .into_iter()
        .take(64)
        .map(|(key, detail)| (safe_text(&key, 120), safe_detail(&detail)))
        .collect();
    Some(value)
}

fn clear_last_stages(root: &Path) {
    let directory = root.join(LOGS_DIRECTORY).join(OPERATIONS_DIRECTORY);
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Ok(metadata) = entry.file_type() else {
            continue;
        };
        if metadata.is_file() && !metadata.is_symlink() && name.starts_with(LAST_STAGE_PREFIX) {
            let _ = fs::remove_file(path);
        }
    }
}

fn begin_session_marker(root: &Path) -> Option<bool> {
    let logs = root.join(LOGS_DIRECTORY);
    if !ensure_real_directory(&logs) {
        return None;
    }
    let marker = logs.join(SESSION_MARKER_FILE);
    let previous_session = match fs::symlink_metadata(&marker) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            if fs::remove_file(&marker).is_err() {
                return None;
            }
            true
        }
        Ok(_) => return None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => return None,
    };
    let created = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
        .is_ok();
    if !created {
        return None;
    }
    Some(previous_session)
}

fn ensure_real_directory(path: &Path) -> bool {
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

fn record_event(
    area: &str,
    level: &'static str,
    kind: &'static str,
    operation: Option<&str>,
    code: Option<&str>,
    message: &str,
    details: &[(&str, String)],
) {
    let mut values = BTreeMap::new();
    for (key, value) in details {
        values.insert((*key).to_string(), safe_detail(value));
    }
    let mut event = LogEvent {
        timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        schema_version: 1,
        app_version: env!("CARGO_PKG_VERSION"),
        platform: crate::platform::platform_name(),
        level,
        kind,
        operation: operation.map(|value| safe_text(value, 120)),
        // Error codes are machine-readable labels, not free-form messages.
        // Keep the bounded identifier visible so a user can correlate the
        // red status in the pool with the corresponding diagnostic entry.
        code: code.map(safe_code),
        message: safe_text(message, MAX_TEXT_BYTES),
        details: values,
    };
    let Ok(mut line) = serde_json::to_vec(&event) else {
        return;
    };
    if line.len() > MAX_EVENT_BYTES {
        event.message = truncate_text(&event.message, 512);
        event.details = event
            .details
            .iter()
            .map(|(key, value)| (key.clone(), truncate_text(value, 512)))
            .collect();
        line = match serde_json::to_vec(&event) {
            Ok(line) => line,
            Err(_) => return,
        };
    }
    if line.len() > MAX_EVENT_BYTES {
        event.details.clear();
        event.message = truncate_text(&event.message, 256);
        line = match serde_json::to_vec(&event) {
            Ok(line) => line,
            Err(_) => return,
        };
    }
    if line.len() > MAX_EVENT_BYTES {
        return;
    }
    line.push(b'\n');
    let state = state();
    let _guard = state
        .write_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = root_path();
    if !ensure_layout(&root) {
        return;
    }
    let (directory, prefix, keep) = match area {
        "operations" => (
            root.join(LOGS_DIRECTORY).join(OPERATIONS_DIRECTORY),
            "operations",
            MAX_RETAINED_OPERATION_FILES,
        ),
        _ => (
            root.join(LOGS_DIRECTORY).join(ERRORS_DIRECTORY),
            "errors",
            MAX_RETAINED_ERROR_FILES,
        ),
    };
    append_rotated_line(&directory, prefix, &line);
    prune_files(&directory, prefix, keep);
}

fn append_rotated_line(directory: &Path, prefix: &str, line: &[u8]) {
    if !ensure_real_directory(directory) {
        return;
    }
    let date = Utc::now().format("%Y-%m-%d").to_string();
    let mut part = 1_u32;
    loop {
        let suffix = if part == 1 {
            String::new()
        } else {
            format!("-{part:02}")
        };
        let path = directory.join(format!("{prefix}-{date}{suffix}.jsonl"));
        let current_size = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => return,
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(_) => return,
        };
        if current_size.saturating_add(line.len() as u64) <= MAX_LOG_FILE_BYTES {
            let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
                return;
            };
            let _ = file.write_all(line);
            return;
        }
        part = part.saturating_add(1);
        if part > 99 {
            return;
        }
    }
}

fn write_panic_report(info: &PanicHookInfo<'_>) {
    let state = state();
    // A panic can happen while another diagnostic write is in progress.  Do
    // not wait for that mutex here: the panicking thread may own it.
    let _guard = match state.write_lock.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    };
    let root = root_path();
    let directory = root.join(LOGS_DIRECTORY).join(CRASHES_DIRECTORY);
    if !ensure_layout(&root) || !ensure_real_directory(&directory) {
        return;
    }
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S-%3f").to_string();
    let sequence = CRASH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(format!("crash-{timestamp}-{sequence:04}.log"));
    let payload = panic_payload(info);
    let location = info
        .location()
        .map(|location| {
            // Source locations can contain the developer's full filesystem
            // path. Keep only the filename and coordinates so a crash report
            // remains useful without exposing a local username or directory.
            let file = Path::new(location.file())
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("unknown");
            format!("{file}:{}:{}", location.line(), location.column())
        })
        .unwrap_or_else(|| "unknown".to_string());
    let breadcrumb = match state.breadcrumb.try_lock() {
        Ok(value) => value.clone(),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner().clone(),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
    .or_else(|| read_latest_stage(&root));
    let mut report = String::new();
    report.push_str("Zenith Relay crash report\n");
    report.push_str("=========================\n");
    report.push_str(&format!("timestamp: {}\n", Utc::now().to_rfc3339()));
    report.push_str(&format!("location: {location}\n"));
    report.push_str(&format!("panic: {}\n", safe_text(&payload, MAX_TEXT_BYTES)));
    if let Some(value) = breadcrumb {
        report.push_str(&format!("stage_timestamp: {}\n", value.timestamp));
        report.push_str(&format!("operation: {}\n", value.operation));
        report.push_str(&format!("stage: {}\n", value.stage));
        for (key, detail) in value.details {
            report.push_str(&format!("{key}: {detail}\n"));
        }
    }
    report.push_str("\nbacktrace:\n");
    report.push_str(&safe_text(
        &std::backtrace::Backtrace::force_capture().to_string(),
        MAX_STACK_BYTES,
    ));
    report.push('\n');
    if let Ok(mut file) = OpenOptions::new().write(true).create_new(true).open(path) {
        let _ = file.write_all(report.as_bytes());
    }
    prune_files(&directory, "crash-", MAX_RETAINED_CRASH_FILES);
}

fn panic_payload(info: &PanicHookInfo<'_>) -> String {
    if let Some(value) = info.payload().downcast_ref::<&str>() {
        return (*value).to_string();
    }
    if let Some(value) = info.payload().downcast_ref::<String>() {
        return value.clone();
    }
    "panic payload is not a string".to_string()
}

fn prune_files(directory: &Path, prefix: &str, keep: usize) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut files = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let metadata = entry.file_type().ok()?;
            if !metadata.is_file() || metadata.is_symlink() || !name.starts_with(prefix) {
                return None;
            }
            Some((name.to_string(), path))
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let remove_count = files.len().saturating_sub(keep);
    for (_, path) in files.into_iter().take(remove_count) {
        let _ = fs::remove_file(path);
    }
}

fn safe_detail(value: &str) -> String {
    safe_text(value, MAX_TEXT_BYTES)
}

/// Preserve only the small, canonical alphabet used by diagnostic/error
/// codes.  Running codes through `safe_text` would treat prefixes such as
/// `source_` as identity-like text and turn useful labels into
/// `source_[redacted]`.
fn safe_code(value: &str) -> String {
    let value = value.trim();
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        value.to_ascii_lowercase()
    } else {
        REDACTED.to_string()
    }
}

fn safe_text(value: &str, max_bytes: usize) -> String {
    let text = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let redacted = redact_secrets(&text);
    truncate_text(&redacted, max_bytes)
}

fn truncate_text(value: &str, max_bytes: usize) -> String {
    if max_bytes == 0 {
        return String::new();
    }
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let ellipsis = "…";
    let mut end = max_bytes.saturating_sub(ellipsis.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &value[..end], ellipsis)
}

fn redact_secrets(value: &str) -> String {
    // Bearer credentials must be removed before generic `authorization: ...`
    // handling; otherwise an unquoted header could leave the token after the
    // first whitespace delimiter.
    let mut output = redact_bearer_values(value.to_string());
    output = redact_url_credentials(output);
    output = redact_sensitive_query_values(output);
    output = redact_sensitive_keys(output);
    output = redact_email_like(&output);
    output = redact_identity_like(&output);
    output = redact_uuid_like_substrings(&output);
    let mut result = String::with_capacity(output.len());
    let mut token = String::new();
    for character in output.chars() {
        if character.is_ascii_alphanumeric() || "._-".contains(character) {
            token.push(character);
            continue;
        }
        append_redacted_token(&mut result, &token);
        token.clear();
        result.push(character);
    }
    append_redacted_token(&mut result, &token);
    result
}

fn redact_sensitive_keys(mut output: String) -> String {
    for key in [
        "token",
        "access_token",
        "accesstoken",
        "refresh_token",
        "refreshtoken",
        "id_token",
        "idtoken",
        "api_key",
        "api-key",
        "apikey",
        "x_api_key",
        "x-api-key",
        "xapikey",
        "authorization",
        "password",
        "cookie",
        "set-cookie",
        "setcookie",
        "session_id",
        "sessionid",
        "client_secret",
        "clientsecret",
        "account_id",
        "accountid",
        "provider_account_id",
        "provideraccountid",
        "user_id",
        "userid",
        "identity",
        "email",
        "secret",
        "private_key",
        "privatekey",
        "prompt",
        "prompts",
        "input",
        "inputs",
        "output",
        "outputs",
        "content",
        "body",
        "request",
        "response",
        "headers",
        "messages",
        "tools",
        "arguments",
    ] {
        let mut search_from = 0;
        loop {
            let lower = output.to_ascii_lowercase();
            let Some(relative) = lower[search_from..].find(key) else {
                break;
            };
            let key_start = search_from + relative;
            let key_end = key_start + key.len();
            let boundary_before = key_start == 0
                || !lower[..key_start]
                    .chars()
                    .next_back()
                    .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_');
            if !boundary_before {
                search_from = key_end;
                continue;
            }
            let mut cursor = skip_key_separator(&output, key_end);
            if cursor >= output.len()
                || !matches!(output[cursor..].chars().next(), Some(':') | Some('='))
            {
                search_from = key_end;
                continue;
            }
            cursor += 1;
            let Some((start, end)) = sensitive_value_range(&output, cursor) else {
                search_from = key_end;
                continue;
            };
            if start < end {
                output.replace_range(start..end, "[redacted]");
                search_from = start + REDACTED.len();
            } else {
                search_from = key_end;
            }
            if search_from >= output.len() {
                break;
            }
        }
    }
    output
}

fn skip_key_separator(value: &str, mut cursor: usize) -> usize {
    while cursor < value.len() {
        let Some(character) = value[cursor..].chars().next() else {
            break;
        };
        if !(character.is_ascii_whitespace() || character == '"' || character == '\'') {
            break;
        }
        cursor += character.len_utf8();
    }
    cursor
}

/// Locate one sensitive value without treating an escaped quote as its end.
/// Malformed quoted input is redacted through the end of the diagnostic, which
/// is safer than attempting to preserve a possibly secret suffix.
fn sensitive_value_range(value: &str, mut cursor: usize) -> Option<(usize, usize)> {
    while cursor < value.len()
        && value[cursor..]
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_whitespace())
    {
        cursor += value[cursor..].chars().next()?.len_utf8();
    }
    if cursor >= value.len() {
        return Some((cursor, cursor));
    }
    let first = value[cursor..].chars().next()?;
    if first == '"' || first == '\'' {
        let quote = first;
        let start = cursor + quote.len_utf8();
        let mut escaped = false;
        for (offset, character) in value[start..].char_indices() {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == quote {
                return Some((start, start + offset));
            }
        }
        return Some((start, value.len()));
    }
    let end = value[cursor..]
        .find(|character: char| character.is_ascii_whitespace() || ",;)}\"'&#[".contains(character))
        .map(|offset| cursor + offset)
        .unwrap_or(value.len());
    Some((cursor, end))
}

fn redact_bearer_values(mut output: String) -> String {
    let mut search_from = 0;
    loop {
        let lower = output.to_ascii_lowercase();
        let Some(relative) = lower[search_from..].find("bearer ") else {
            break;
        };
        let start = search_from + relative + "bearer ".len();
        let end = output[start..]
            .find(|character: char| character.is_whitespace() || ",;)}\"'".contains(character))
            .map(|offset| start + offset)
            .unwrap_or(output.len());
        output.replace_range(start..end, REDACTED);
        search_from = start + REDACTED.len();
        if search_from >= output.len() {
            break;
        }
    }
    output
}

fn redact_url_credentials(mut output: String) -> String {
    let mut search_from = 0;
    while let Some(relative) = output[search_from..].find("://") {
        let scheme_end = search_from + relative + 3;
        let authority_end = output[scheme_end..]
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '/' | '?' | '#')
            })
            .map(|offset| scheme_end + offset)
            .unwrap_or(output.len());
        let Some(at_offset) = output[scheme_end..authority_end].find('@') else {
            search_from = authority_end;
            if search_from >= output.len() {
                break;
            }
            continue;
        };
        let at = scheme_end + at_offset;
        output.replace_range(scheme_end..at, REDACTED);
        search_from = scheme_end + REDACTED.len() + 1;
        if search_from >= output.len() {
            break;
        }
    }
    output
}

fn redact_sensitive_query_values(mut output: String) -> String {
    // Parse one query segment at a time.  Looking for the first `=` after a
    // `?`/`&` marker is subtly wrong for inputs such as `?flag&token=secret`:
    // the key would be seen as `flag&token` and the credential would survive.
    // Collect ranges against the original string and replace from the end so
    // multiple credentials can be removed without invalidating offsets.
    const KEYS: [&str; 22] = [
        "token",
        "access_token",
        "accesstoken",
        "refresh_token",
        "refreshtoken",
        "id_token",
        "idtoken",
        "api_key",
        "apikey",
        "secret",
        "code",
        "auth",
        "authorization",
        "session",
        "session_id",
        "sessionid",
        "account_id",
        "accountid",
        "client_secret",
        "clientsecret",
        "x_api_key",
        "xapikey",
    ];
    let mut ranges = Vec::<(usize, usize)>::new();
    let bytes = output.as_bytes();
    let mut marker = 0;
    while marker < bytes.len() {
        if !matches!(bytes[marker], b'?' | b'&') {
            marker += 1;
            continue;
        }
        let key_start = marker + 1;
        let segment_end = output[key_start..]
            .find(|character: char| {
                character.is_ascii_whitespace() || matches!(character, '&' | '#')
            })
            .map(|offset| key_start + offset)
            .unwrap_or(output.len());
        let Some(equal_offset) = output[key_start..segment_end].find('=') else {
            marker = segment_end;
            continue;
        };
        let equal = key_start + equal_offset;
        let key = output[key_start..equal].trim_matches(|character: char| {
            character.is_ascii_whitespace() || matches!(character, '"' | '\'')
        });
        let normalized_key = key
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if !KEYS.iter().any(|candidate| {
            candidate
                .chars()
                .filter(|character| character.is_ascii_alphanumeric())
                .eq(normalized_key.chars())
        }) {
            marker = segment_end;
            continue;
        }
        let value_start = equal + 1;
        let first_value_character = output[value_start..segment_end].chars().next();
        let value_end =
            if first_value_character.is_some_and(|character| matches!(character, '"' | '\'')) {
                // The quote itself is retained; only its contents are replaced.
                let quote = first_value_character.unwrap();
                let content_start = value_start + quote.len_utf8();
                output[content_start..segment_end]
                    .find(quote)
                    .map(|offset| content_start + offset)
                    .unwrap_or(segment_end)
            } else {
                output[value_start..segment_end]
                    .find(['"', '\''])
                    .map(|offset| value_start + offset)
                    .unwrap_or(segment_end)
            };
        if value_start < value_end {
            let content_start = if output[value_start..value_end]
                .chars()
                .next()
                .is_some_and(|character| matches!(character, '"' | '\''))
            {
                value_start + output[value_start..].chars().next().unwrap().len_utf8()
            } else {
                value_start
            };
            if content_start < value_end {
                ranges.push((content_start, value_end));
            }
        }
        marker = segment_end;
    }
    ranges.sort_unstable_by_key(|range| std::cmp::Reverse(range.0));
    for (start, end) in ranges {
        output.replace_range(start..end, REDACTED);
    }
    output
}

fn redact_identity_like(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut token = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || "._-".contains(character) {
            token.push(character);
            continue;
        }
        append_redacted_identity(&mut result, &token);
        token.clear();
        result.push(character);
    }
    append_redacted_identity(&mut result, &token);
    result
}

/// Remove UUIDs even when they are embedded in a useful operation label such
/// as `delete-account-<uuid>`. Account/source/session identifiers are commonly
/// UUIDs, and keeping the surrounding label preserves enough context to debug
/// the failed action without writing the identifier itself.
fn redact_uuid_like_substrings(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut result = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if is_uuid_at(bytes, cursor)
            && (cursor == 0 || !is_uuid_boundary_byte(bytes[cursor - 1]))
            && (cursor + 36 == bytes.len() || !is_uuid_boundary_byte(bytes[cursor + 36]))
        {
            result.push_str(REDACTED);
            cursor += 36;
        } else {
            let Some(character) = value[cursor..].chars().next() else {
                break;
            };
            result.push(character);
            cursor += character.len_utf8();
        }
    }
    result
}

fn is_uuid_at(bytes: &[u8], start: usize) -> bool {
    if start.saturating_add(36) > bytes.len() {
        return false;
    }
    for (offset, byte) in bytes[start..start + 36].iter().copied().enumerate() {
        if matches!(offset, 8 | 13 | 18 | 23) {
            if byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn is_uuid_boundary_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

fn append_redacted_identity(output: &mut String, token: &str) {
    let lower = token.to_ascii_lowercase();
    let identity_prefixes = [
        "account_",
        "account-",
        "acct_",
        "acct-",
        "user_",
        "user-",
        "provider_",
        "provider-",
        "source_",
        "source-",
        "session_",
        "session-",
        "import_",
        "import-",
        "proxy_",
        "proxy-",
        "task_",
        "task-",
    ];
    for prefix in identity_prefixes {
        let Some(index) = lower.find(prefix) else {
            continue;
        };
        if index > 0
            && lower[..index]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            continue;
        }
        let suffix = &token[index + prefix.len()..];
        // Keep human-readable operation names such as `account-import` while
        // still hiding account/user identifiers, which normally contain a
        // digit or are long opaque values.
        if suffix.is_empty()
            || (!suffix.chars().any(|character| character.is_ascii_digit()) && suffix.len() < 16)
        {
            continue;
        }
        output.push_str(&token[..index + prefix.len()]);
        output.push_str(REDACTED);
        return;
    }
    output.push_str(token);
}

fn redact_email_like(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut candidate = String::new();
    let flush = |result: &mut String, candidate: &mut String| {
        let at = candidate.find('@');
        let dot_after_at = at.and_then(|index| candidate[index + 1..].find('.'));
        // A URL whose userinfo was already replaced leaves `@host`; retain
        // the host so the diagnostic still identifies the failing endpoint.
        if at.is_some_and(|index| index > 0) && dot_after_at.is_some() {
            result.push_str("[redacted]");
        } else {
            result.push_str(candidate);
        }
        candidate.clear();
    };
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || "._%+-@".contains(character) {
            candidate.push(character);
        } else {
            flush(&mut result, &mut candidate);
            result.push(character);
        }
    }
    flush(&mut result, &mut candidate);
    result
}

fn append_redacted_token(output: &mut String, token: &str) {
    let token_lower = token.to_ascii_lowercase();
    let sensitive_prefix = [
        "sk-",
        "pk-",
        "rk-",
        "znt-",
        "zrs-",
        "ghp_",
        "github_pat_",
        "at-",
        "xoxb-",
        "xoxp-",
    ]
    .iter()
    .any(|prefix| token_lower.starts_with(prefix) && token.len() >= prefix.len() + 8);
    let jwt = token_lower.starts_with("eyj") && token.len() > 24;
    let looks_like_email = token.contains('@') && token.contains('.');
    if sensitive_prefix || jwt || looks_like_email {
        output.push_str(REDACTED);
    } else {
        output.push_str(token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn identifiers_are_stable_but_not_reversible() {
        let first = hash_identifier("account-synthetic");
        assert_eq!(first, hash_identifier("account-synthetic"));
        assert!(first.starts_with("id_"));
        assert!(!first.contains("account"));
    }

    #[test]
    fn redaction_removes_common_credentials_and_multiline_text() {
        let text = safe_text(
            "Bearer synthetic-token\nemail@example.test sk-1234567890abcdef",
            MAX_TEXT_BYTES,
        );
        assert!(!text.contains("synthetic-token"));
        assert!(!text.contains("email@example.test"));
        assert!(!text.contains("sk-1234567890abcdef"));
        assert!(!text.contains('\n'));
    }

    #[test]
    fn redaction_handles_escaped_json_values_and_identity_fields() {
        let text = safe_text(
            r#"{"api_key":"synthetic-\"quoted\"-secret","account_id":"account_private_123456","user_id":"user-opaque-value-123456789","authorization":"Bearer synthetic-bearer","url":"https://login-name@example.test/v1?state=ok&code=oauth-secret#done"}"#,
            MAX_TEXT_BYTES,
        );
        assert!(!text.contains("synthetic-"));
        assert!(!text.contains("account_private_123456"));
        assert!(!text.contains("user-opaque-value-123456789"));
        assert!(!text.contains("login-name@example.test"));
        assert!(!text.contains("Bearer synthetic-bearer"));
        assert!(text.contains("https://[redacted]@example.test/v1"));
        assert!(text.contains("?state=ok&code=[redacted]#done"));
    }

    #[test]
    fn redaction_covers_camel_case_fields_and_every_query_segment() {
        let text = safe_text(
            r#"{"accessToken":"camel-secret","refreshToken":"refresh-secret","apiKey":"key-secret","safe":"visible","url":"https://example.test/path?flag&token=query-secret&safe=visible&code=second-secret"}"#,
            MAX_TEXT_BYTES,
        );
        assert!(!text.contains("camel-secret"));
        assert!(!text.contains("refresh-secret"));
        assert!(!text.contains("key-secret"));
        assert!(!text.contains("query-secret"));
        assert!(!text.contains("second-secret"));
        assert!(text.contains("safe"));
        assert!(text.contains("?flag&token=[redacted]&safe=visible&code=[redacted]"));
    }

    #[test]
    fn redaction_keeps_operation_names_and_hides_identity_like_tokens() {
        assert_eq!(
            safe_text("account-import", MAX_TEXT_BYTES),
            "account-import"
        );
        let account = safe_text("account_private_123456", MAX_TEXT_BYTES);
        assert!(account.starts_with("account_"));
        assert!(!account.contains("private_123456"));
        assert_eq!(safe_text("acct-42", MAX_TEXT_BYTES), "acct-[redacted]");
    }

    #[test]
    fn diagnostic_codes_remain_actionable_without_allowing_free_form_text() {
        assert_eq!(
            safe_code(" source_protocol_invalid "),
            "source_protocol_invalid"
        );
        assert_eq!(
            safe_code("Gateway-Restart.Failed"),
            "gateway-restart.failed"
        );
        assert_eq!(safe_code("source account invalid"), REDACTED);
        assert_eq!(safe_code("https://example.test/?token=secret"), REDACTED);
    }

    #[test]
    fn redaction_hides_embedded_uuid_identifiers_without_losing_operation_context() {
        let value = safe_text(
            "delete-account-123e4567-e89b-12d3-a456-426614174000 failed",
            MAX_TEXT_BYTES,
        );
        assert_eq!(value, "delete-account-[redacted] failed");
    }

    #[test]
    fn truncation_respects_utf8_byte_limits() {
        assert_eq!(truncate_text("abcdef", 0), "");
        let value = truncate_text("Привет мир", 8);
        assert!(value.len() <= 8);
        assert!(value.ends_with('…'));
    }

    #[test]
    fn layout_uses_separate_error_crash_and_operation_directories() {
        let root = env::temp_dir().join(format!(
            "zenith-relay-diagnostics-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        ensure_layout(&root);
        assert!(root.join("logs/errors").is_dir());
        assert!(root.join("logs/crashes").is_dir());
        assert!(root.join("logs/operations").is_dir());
        assert!(root.join("logs/README.txt").is_file());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn debug_marker_is_disabled_by_default_and_enabled_by_presence() {
        let root = env::temp_dir().join(format!(
            "zenith-relay-debug-marker-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        assert!(ensure_layout(&root));
        assert!(!read_debug_marker(&root).expect("default marker state"));
        fs::write(root.join(LOGS_DIRECTORY).join(DEBUG_MARKER_FILE), b"").expect("marker");
        assert!(read_debug_marker(&root).expect("enabled marker state"));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn interrupted_stage_marker_survives_and_is_cleared_after_clean_shutdown() {
        let root = env::temp_dir().join(format!(
            "zenith-relay-stage-marker-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        assert!(ensure_layout(&root));
        let value = Breadcrumb {
            timestamp: "2026-09-15T12:34:56.000Z".to_string(),
            operation: "account-import".to_string(),
            stage: "runtime_sync_started".to_string(),
            details: BTreeMap::from([("account".to_string(), "id_synthetic".to_string())]),
        };
        persist_last_stage_at(&root, &value);
        let loaded = read_latest_stage(&root).expect("stage marker");
        assert_eq!(loaded.operation, value.operation);
        assert_eq!(loaded.stage, value.stage);
        assert_eq!(loaded.details, value.details);
        clear_last_stages(&root);
        assert!(read_latest_stage(&root).is_none());
        fs::remove_dir_all(root).expect("cleanup");
    }
}
