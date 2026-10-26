//! Local-first diagnostics for the desktop application.
//!
//! Diagnostics are deliberately kept outside the database and outside the
//! encrypted vault.  They are short, structured, redacted records which make
//! an otherwise opaque renderer/native failure useful to a person debugging
//! their own Relay installation.  No request body, prompt, response, cookie,
//! credential, or raw account identity is written here.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64},
    },
};

pub(crate) mod memory;
mod persist;
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
const MAX_TEXT_BYTES: usize = 2_000;
const MAX_STACK_BYTES: usize = 24_000;
const REDACTED: &str = "[redacted]";
const SESSION_MARKER_FILE: &str = "session.active";

static STATE: OnceLock<DiagnosticState> = OnceLock::new();
static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();
static CRASH_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);
// Detailed operation breadcrumbs are opt-in. Errors and crash reports remain
// enabled regardless of this flag so a normal installation stays quiet while
// still retaining the information needed to diagnose a failure.
static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

struct DiagnosticState {
    root: Mutex<PathBuf>,
    write_lock: Mutex<persist::Writers>,
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
pub struct DiagnosticSettings {
    pub debug_enabled: bool,
}

mod lifecycle;
mod record;
mod settings;

pub(crate) use lifecycle::{initialize, install_panic_hook, shutdown};
pub(crate) use record::{
    breadcrumb, hash_identifier, record_error, record_frontend_error, record_operation,
};
pub(crate) use settings::{is_debug_enabled, set_debug_enabled, settings};

#[tauri::command]
pub async fn record_frontend_diagnostic(input: FrontendDiagnosticInput) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || record_frontend_error(input))
        .await
        .map_err(|_| "Frontend diagnostic write was interrupted".to_string())
}

#[tauri::command]
pub fn get_diagnostic_settings() -> DiagnosticSettings {
    settings()
}

#[tauri::command]
pub async fn set_diagnostic_debug_mode(
    enabled: bool,
    state: tauri::State<'_, crate::local_pool::DesktopState>,
) -> Result<DiagnosticSettings, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let was_enabled = is_debug_enabled();
        set_debug_enabled(enabled)?;
        if enabled && !was_enabled {
            memory::record_snapshot(&state, "debug_enabled");
        }
        Ok(settings())
    })
    .await
    .map_err(|_| "Diagnostic setting update was interrupted".to_string())?
}

#[cfg(test)]
mod tests;
