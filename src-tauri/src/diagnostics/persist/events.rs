use chrono::{SecondsFormat, Utc};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic::PanicHookInfo;
use std::path::Path;
use std::sync::atomic::Ordering;

use super::super::*;
use super::layout::{
    ensure_area, ensure_layout, ensure_real_directory, prune_files, root_path, state,
};
use super::stage::read_latest_stage;

#[derive(Default)]
pub(super) struct LogCursor {
    directory: PathBuf,
    date: String,
    part: u64,
}

pub(in crate::diagnostics) fn record_event(
    area: &str,
    level: &'static str,
    kind: &'static str,
    operation: Option<&str>,
    code: Option<&str>,
    message: &str,
    details: &[(&str, String)],
) {
    let mut sanitized_details = BTreeMap::new();
    for (detail_key, detail_value) in details {
        sanitized_details.insert((*detail_key).to_string(), safe_detail(detail_value));
    }
    let mut event = LogEvent {
        timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        schema_version: 1,
        app_version: env!("CARGO_PKG_VERSION"),
        platform: crate::platform::platform_name(),
        level,
        kind,
        operation: operation.map(|operation_name| safe_text(operation_name, 120)),
        // Error codes are machine-readable labels, not free-form messages.
        // Keep the bounded identifier visible so a user can correlate the
        // red status in the pool with the corresponding diagnostic entry.
        code: code.map(safe_code),
        message: safe_text(message, MAX_TEXT_BYTES),
        details: sanitized_details,
    };
    let Ok(mut line) = serde_json::to_vec(&event) else {
        return;
    };
    if line.len() > MAX_EVENT_BYTES {
        event.message = truncate_text(&event.message, 512);
        event.details = event
            .details
            .iter()
            .map(|(detail_key, detail_value)| {
                (detail_key.clone(), truncate_text(detail_value, 512))
            })
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
    let mut writers = zenith_relay_core::poison::mutex(&state.write_lock);
    let root = root_path();
    let (area, prefix, keep, cursor) = match area {
        "operations" => (
            OPERATIONS_DIRECTORY,
            "operations",
            MAX_RETAINED_OPERATION_FILES,
            &mut writers.operations,
        ),
        _ => (
            ERRORS_DIRECTORY,
            "errors",
            MAX_RETAINED_ERROR_FILES,
            &mut writers.errors,
        ),
    };
    let Some(directory) = ensure_area(&root, area) else {
        return;
    };
    let _ = cursor.append(&directory, prefix, &line, keep, MAX_LOG_FILE_BYTES);
}

impl LogCursor {
    fn append(
        &mut self,
        directory: &Path,
        prefix: &str,
        line: &[u8],
        keep: usize,
        limit: u64,
    ) -> std::io::Result<()> {
        let date = Utc::now().format("%Y-%m-%d").to_string();
        let initialize = self.directory != directory || self.date != date || self.part == 0;
        if initialize {
            // Resume the newest part, never refill a deleted part 1. Otherwise
            // retention can delete the very event that was just appended.
            let part = log_files(directory, prefix)?
                .into_iter()
                .filter(|(day, _, _)| day == &date)
                .map(|(_, part, _)| part)
                .max()
                .unwrap_or(1);
            self.directory = directory.to_path_buf();
            self.date = date;
            self.part = part;
        }
        loop {
            let suffix = if self.part == 1 {
                String::new()
            } else {
                format!("-{:02}", self.part)
            };
            let path = directory.join(format!("{prefix}-{}{suffix}.jsonl", self.date));
            let (size, exists) = match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    (metadata.len(), true)
                }
                Ok(_) => return Err(std::io::ErrorKind::InvalidInput.into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (0, false),
                Err(error) => return Err(error),
            };
            if size == 0 || size.saturating_add(line.len() as u64) <= limit {
                let mut options = OpenOptions::new();
                options.append(true);
                if !exists {
                    options.create_new(true);
                }
                options.open(path)?.write_all(line)?;
                // No buffered queue, no timer, and no directory scan on append.
                if !exists || initialize {
                    prune_logs(directory, prefix, keep)?;
                }
                return Ok(());
            }
            self.part = self
                .part
                .checked_add(1)
                .ok_or(std::io::ErrorKind::InvalidData)?;
        }
    }
}

fn log_files(directory: &Path, prefix: &str) -> std::io::Result<Vec<(String, u64, PathBuf)>> {
    let mut files = Vec::new();
    let prefix = format!("{prefix}-");
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if !kind.is_file() || kind.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let Some(stem) = name
            .to_str()
            .and_then(|name| name.strip_prefix(&prefix))
            .and_then(|name| name.strip_suffix(".jsonl"))
        else {
            continue;
        };
        let Some(day) = stem
            .get(..10)
            .filter(|day| chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").is_ok())
        else {
            continue;
        };
        let part = if stem.len() == 10 {
            Some(1)
        } else {
            stem.get(10..)
                .and_then(|tail| tail.strip_prefix('-'))
                .and_then(|part| part.parse::<u64>().ok())
                .filter(|part| *part > 1)
        };
        if let Some(part) = part {
            files.push((day.to_string(), part, entry.path()));
        }
    }
    Ok(files)
}

fn prune_logs(directory: &Path, prefix: &str, keep: usize) -> std::io::Result<()> {
    let mut files = log_files(directory, prefix)?;
    files.sort_by(|left, right| (&left.0, left.1).cmp(&(&right.0, right.1)));
    let remove_count = files.len().saturating_sub(keep);
    for (_, _, path) in files.into_iter().take(remove_count) {
        fs::remove_file(path)?;
    }
    Ok(())
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
    let panic_message = panic_payload(info);
    let location = info
        .location()
        .map(|location| {
            // Source locations can contain the developer's full filesystem
            // path. Keep only the filename and coordinates so a crash report
            // remains useful without exposing a local username or directory.
            let file = Path::new(location.file())
                .file_name()
                .and_then(|file_name| file_name.to_str())
                .unwrap_or("unknown");
            format!("{file}:{}:{}", location.line(), location.column())
        })
        .unwrap_or_else(|| "unknown".to_string());
    let breadcrumb = zenith_relay_core::poison::try_mutex(&state.breadcrumb)
        .and_then(|breadcrumb| breadcrumb.clone())
        .or_else(|| read_latest_stage(&root));
    let mut report = String::new();
    report.push_str("Zenith Relay crash report\n");
    report.push_str("=========================\n");
    report.push_str(&format!("timestamp: {}\n", Utc::now().to_rfc3339()));
    report.push_str(&format!("location: {location}\n"));
    report.push_str(&format!(
        "panic: {}\n",
        safe_text(&panic_message, MAX_TEXT_BYTES)
    ));
    if let Some(stage_record) = breadcrumb {
        report.push_str(&format!("stage_timestamp: {}\n", stage_record.timestamp));
        report.push_str(&format!("operation: {}\n", stage_record.operation));
        report.push_str(&format!("stage: {}\n", stage_record.stage));
        for (detail_key, detail_value) in stage_record.details {
            report.push_str(&format!("{detail_key}: {detail_value}\n"));
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
    if let Some(panic_text) = info.payload().downcast_ref::<&str>() {
        return (*panic_text).to_string();
    }
    if let Some(panic_message) = info.payload().downcast_ref::<String>() {
        return panic_message.clone();
    }
    "panic payload is not a string".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_retains_newest_events_past_99_parts_and_after_restart() {
        let root = crate::diagnostics::tests::test_root("rotation");
        let directory = ensure_area(&root, OPERATIONS_DIRECTORY).unwrap();
        let mut cursor = LogCursor::default();
        for event in 1..=120 {
            cursor
                .append(
                    &directory,
                    "operations",
                    format!("{event:03}\n").as_bytes(),
                    14,
                    4,
                )
                .unwrap();
        }
        let mut files = log_files(&directory, "operations").unwrap();
        files.sort_by_key(|(_, part, _)| *part);
        assert_eq!(files.len(), 14);
        let values: Vec<_> = files
            .iter()
            .map(|(_, _, path)| fs::read_to_string(path).unwrap())
            .collect();
        assert_eq!(
            values,
            (107..=120)
                .map(|event| format!("{event:03}\n"))
                .collect::<Vec<_>>()
        );
        LogCursor::default()
            .append(&directory, "operations", b"121\n", 14, 4)
            .unwrap();
        assert_eq!(log_files(&directory, "operations").unwrap().len(), 14);
        let day = Utc::now().format("%Y-%m-%d");
        assert_eq!(
            fs::read_to_string(directory.join(format!("operations-{day}-121.jsonl"))).unwrap(),
            "121\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ordinary_appends_preserve_every_line_without_running_retention() {
        let root = crate::diagnostics::tests::test_root("append");
        let directory = ensure_area(&root, ERRORS_DIRECTORY).unwrap();
        let mut cursor = LogCursor::default();
        cursor
            .append(&directory, "errors", b"first\n", 1, 1024)
            .unwrap();
        let old = directory.join("errors-2000-01-01.jsonl");
        fs::write(&old, b"old\n").unwrap();
        for _ in 0..100 {
            cursor
                .append(&directory, "errors", b"next\n", 1, 1024)
                .unwrap();
        }
        assert!(
            old.exists(),
            "an ordinary append must not scan/prune the directory"
        );
        let day = Utc::now().format("%Y-%m-%d");
        let content = fs::read_to_string(directory.join(format!("errors-{day}.jsonl"))).unwrap();
        assert_eq!(content, format!("first\n{}", "next\n".repeat(100)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsafe_log_target_is_rejected_without_overwriting_it() {
        let root = crate::diagnostics::tests::test_root("unsafe-log");
        let directory = ensure_area(&root, ERRORS_DIRECTORY).unwrap();
        let day = Utc::now().format("%Y-%m-%d");
        let target = directory.join(format!("errors-{day}.jsonl"));
        fs::create_dir(&target).unwrap();
        assert!(
            LogCursor::default()
                .append(&directory, "errors", b"event\n", 14, 1024)
                .is_err()
        );
        assert!(target.is_dir());
        fs::remove_dir_all(root).unwrap();
    }
}
