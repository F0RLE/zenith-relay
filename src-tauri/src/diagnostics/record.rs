use super::persist;
use super::settings::is_debug_enabled;
use super::{
    safe_detail, safe_text, Breadcrumb, FrontendDiagnosticInput, MAX_STACK_BYTES, SESSION_ACTIVE,
};
use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

/// Record a native operation breadcrumb.  The latest breadcrumb is copied to
/// a crash report, so a panic can be tied to the last known stage even when no
/// normal error response reaches the UI.
pub(crate) fn breadcrumb(operation: &str, operation_stage: &str, details: &[(&str, String)]) {
    let diagnostic_state = persist::state();
    let mut diagnostic_details = BTreeMap::new();
    for (detail_key, detail_value) in details {
        diagnostic_details.insert((*detail_key).to_string(), safe_detail(detail_value));
    }
    let breadcrumb = Breadcrumb {
        timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        operation: safe_text(operation, 120),
        stage: safe_text(operation_stage, 120),
        details: diagnostic_details,
    };
    if let Ok(mut current_breadcrumb) = diagnostic_state.breadcrumb.lock() {
        *current_breadcrumb = Some(breadcrumb.clone());
    }
    if SESSION_ACTIVE.load(Ordering::Acquire) {
        persist::persist_last_stage(&breadcrumb);
    }
}

pub(crate) fn record_operation(operation: &str, outcome: &str, details: &[(&str, String)]) {
    breadcrumb(operation, outcome, details);
    if !is_debug_enabled() {
        return;
    }
    persist::record_event(
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
    persist::record_event(
        "errors",
        "error",
        "native_error",
        Some(operation),
        code,
        message,
        details,
    );
}

pub(crate) fn record_frontend_error(frontend_diagnostic: FrontendDiagnosticInput) {
    let operation = frontend_diagnostic
        .operation
        .as_deref()
        .unwrap_or("renderer");
    let mut diagnostic_details = Vec::new();
    if let Some(stack) = frontend_diagnostic.stack.as_deref() {
        diagnostic_details.push(("stack", safe_text(stack, MAX_STACK_BYTES)));
    }
    diagnostic_details.push(("source", safe_text(&frontend_diagnostic.source, 120)));
    diagnostic_details.push(("fatal", frontend_diagnostic.fatal.to_string()));
    breadcrumb(operation, "renderer_error", &diagnostic_details);
    persist::record_event(
        "errors",
        if frontend_diagnostic.fatal {
            "fatal"
        } else {
            "error"
        },
        "frontend_error",
        Some(operation),
        frontend_diagnostic.code.as_deref(),
        &frontend_diagnostic.message,
        &diagnostic_details,
    );
}

/// Hash an identifier before it is placed in a diagnostic.  A stable short
/// hash is enough to correlate repeated failures while keeping the identity
/// itself out of the log directory.
pub(crate) fn hash_identifier(identifier: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(identifier.as_bytes());
    let encoded = hex::encode(digest.finalize());
    format!("id_{}", &encoded[..12])
}
