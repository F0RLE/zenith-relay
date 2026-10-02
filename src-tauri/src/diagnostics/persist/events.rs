use chrono::{SecondsFormat, Utc};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic::PanicHookInfo;
use std::path::Path;
use std::sync::atomic::Ordering;

use super::super::*;
use super::layout::{ensure_layout, ensure_real_directory, prune_files, root_path, state};
use super::stage::read_latest_stage;

pub(in crate::diagnostics) fn record_event(
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
    let _guard = zenith_relay_core::poison::mutex(&state.write_lock);
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

pub(in crate::diagnostics) fn write_panic_report(info: &PanicHookInfo<'_>) {
    let state = state();
    // A panic can happen while another diagnostic write is in progress.  Do
    // not wait for that mutex here: the panicking thread may own it.
    let _guard = zenith_relay_core::poison::try_mutex(&state.write_lock);
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
    let breadcrumb = zenith_relay_core::poison::try_mutex(&state.breadcrumb)
        .and_then(|value| value.clone())
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
