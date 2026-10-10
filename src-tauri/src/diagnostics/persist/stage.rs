use chrono::Utc;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::Ordering;

use super::super::*;
use super::layout::{ensure_layout, ensure_real_directory, prune_files, root_path, state};

/// Keep one small, redacted stage marker on disk. Unlike the in-memory
/// breadcrumb, this survives a hard process termination and is consumed on
/// the next launch when `session.active` indicates an interrupted run.
pub(in crate::diagnostics) fn persist_last_stage(breadcrumb: &Breadcrumb) {
    let state = state();
    let _guard = zenith_relay_core::poison::mutex(&state.write_lock);
    let root = root_path();
    persist_last_stage_at(&root, breadcrumb);
}

pub(in crate::diagnostics) fn persist_last_stage_at(root: &Path, breadcrumb: &Breadcrumb) {
    let Ok(mut line) = serde_json::to_vec(breadcrumb) else {
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

pub(in crate::diagnostics) fn read_latest_stage(root: &Path) -> Option<Breadcrumb> {
    let directory = root.join(LOGS_DIRECTORY).join(OPERATIONS_DIRECTORY);
    let Ok(entries) = fs::read_dir(directory) else {
        return None;
    };
    let mut paths = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let file_name = path.file_name()?.to_str()?;
            let metadata = entry.file_type().ok()?;
            if !metadata.is_file()
                || metadata.is_symlink()
                || !file_name.starts_with(LAST_STAGE_PREFIX)
            {
                return None;
            }
            Some((file_name.to_string(), path))
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
    let breadcrumb = serde_json::from_slice::<Breadcrumb>(&bytes).ok()?;
    sanitize_breadcrumb(breadcrumb)
}

fn sanitize_breadcrumb(mut breadcrumb: Breadcrumb) -> Option<Breadcrumb> {
    if breadcrumb.timestamp.len() > 64 {
        return None;
    }
    breadcrumb.timestamp = safe_text(&breadcrumb.timestamp, 64);
    breadcrumb.operation = safe_text(&breadcrumb.operation, 120);
    breadcrumb.stage = safe_text(&breadcrumb.stage, 120);
    if breadcrumb.operation.is_empty() || breadcrumb.stage.is_empty() {
        return None;
    }
    breadcrumb.details = breadcrumb
        .details
        .into_iter()
        .take(64)
        .map(|(key, detail)| (safe_text(&key, 120), safe_detail(&detail)))
        .collect();
    Some(breadcrumb)
}

pub(in crate::diagnostics) fn clear_last_stages(root: &Path) {
    let directory = root.join(LOGS_DIRECTORY).join(OPERATIONS_DIRECTORY);
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|file_name| file_name.to_str()) else {
            continue;
        };
        let Ok(metadata) = entry.file_type() else {
            continue;
        };
        if metadata.is_file() && !metadata.is_symlink() && file_name.starts_with(LAST_STAGE_PREFIX)
        {
            let _ = fs::remove_file(path);
        }
    }
}

pub(in crate::diagnostics) fn begin_session_marker(root: &Path) -> Option<bool> {
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
