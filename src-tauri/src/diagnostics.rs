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

mod redact;
use redact::{safe_code, safe_detail, safe_text, truncate_text};

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
