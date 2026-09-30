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
pub(crate) fn breadcrumb(operation: &str, stage: &str, details: &[(&str, String)]) {
    let state = persist::state();
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
        persist::persist_last_stage(&value);
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

pub(crate) fn record_frontend_error(input: FrontendDiagnosticInput) {
    let operation = input.operation.as_deref().unwrap_or("renderer");
    let mut details = Vec::new();
    if let Some(stack) = input.stack.as_deref() {
        details.push(("stack", safe_text(stack, MAX_STACK_BYTES)));
    }
    details.push(("source", safe_text(&input.source, 120)));
    details.push(("fatal", input.fatal.to_string()));
    breadcrumb(operation, "renderer_error", &details);
    persist::record_event(
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
