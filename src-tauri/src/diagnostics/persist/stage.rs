use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use super::super::*;
use super::layout::{ensure_area, ensure_real_directory, root_path, state};

#[derive(Default)]
pub(in crate::diagnostics) struct StageWriter {
    root: PathBuf,
    next_sequence: Option<u64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredStage<T> {
    sequence: u64,
    breadcrumb: T,
}

const STAGE_SLOTS: [&str; 2] = ["last-stage-slot-0.json", "last-stage-slot-1.json"];
// Preserve the old breadcrumb capacity; the sequence envelope is bookkeeping.
const MAX_STAGE_RECORD_BYTES: usize = MAX_STAGE_BYTES + 64;

/// Keep one small, redacted stage marker on disk. Unlike the in-memory
/// breadcrumb, this survives a hard process termination and is consumed on
/// the next launch when `session.active` indicates an interrupted run.
pub(in crate::diagnostics) fn persist_last_stage(breadcrumb: &Breadcrumb) {
    let state = state();
    let mut writers = zenith_relay_core::poison::mutex(&state.write_lock);
    let root = root_path();
    writers.stage.write(&root, breadcrumb);
}

impl StageWriter {
    pub(in crate::diagnostics) fn write(&mut self, root: &Path, breadcrumb: &Breadcrumb) {
        let Some(directory) = ensure_area(root, OPERATIONS_DIRECTORY) else {
            return;
        };
        if self.root != root || self.next_sequence.is_none() {
            self.root = root.to_path_buf();
            self.next_sequence = Some(
                STAGE_SLOTS
                    .iter()
                    .filter_map(|name| read_stage(&directory.join(name)))
                    .filter_map(|(sequence, _)| sequence)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1),
            );
        }
        let sequence = self.next_sequence.unwrap_or(1);
        let record = StoredStage {
            sequence,
            breadcrumb,
        };
        let Ok(mut line) = serde_json::to_vec(&record) else {
            return;
        };
        line.push(b'\n');
        if line.len() > MAX_STAGE_RECORD_BYTES {
            return;
        }
        let path = directory.join(STAGE_SLOTS[(sequence % 2) as usize]);
        let mut options = OpenOptions::new();
        options.write(true);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                options.truncate(true);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                options.create_new(true);
            }
            _ => return,
        }
        // Write only the older slot. A torn write leaves the previous complete
        // breadcrumb readable; no queue or user-space buffer delays persistence.
        let Ok(mut file) = options.open(path) else {
            return;
        };
        if file.write_all(&line).and_then(|()| file.flush()).is_ok() {
            self.next_sequence = sequence.checked_add(1);
        }
    }
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
    // New slots use a monotonic persisted sequence (including across restarts),
    // not wall clock time. Legacy markers remain readable during upgrades.
    paths.sort_by(|left, right| right.0.cmp(&left.0));
    let mut latest: Option<(Option<u64>, Breadcrumb)> = None;
    for (_, path) in paths {
        let Some(candidate) = read_stage(&path) else {
            continue;
        };
        if latest
            .as_ref()
            .is_none_or(|current| candidate.0 > current.0)
        {
            latest = Some(candidate);
        }
    }
    latest.map(|(_, breadcrumb)| breadcrumb)
}

fn read_stage(path: &Path) -> Option<(Option<u64>, Breadcrumb)> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_STAGE_RECORD_BYTES as u64
    {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if let Ok(record) = serde_json::from_slice::<StoredStage<Breadcrumb>>(&bytes) {
        return sanitize_breadcrumb(record.breadcrumb).map(|value| (Some(record.sequence), value));
    }
    sanitize_breadcrumb(serde_json::from_slice::<Breadcrumb>(&bytes).ok()?)
        .map(|value| (None, value))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_reuse_two_files_resume_sequences_and_recover_a_torn_write() {
        let root = crate::diagnostics::tests::test_root("stage-slots");
        let mut writer = StageWriter::default();
        let mut value = Breadcrumb {
            timestamp: "2026-10-25T12:00:00.000Z".to_string(),
            operation: "synthetic-operation".to_string(),
            stage: String::new(),
            details: BTreeMap::new(),
        };
        for index in 1..=100 {
            value.stage = format!("stage_{index}");
            writer.write(&root, &value);
        }
        let directory = root.join("logs/operations");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        assert_eq!(read_latest_stage(&root).unwrap().stage, "stage_100");
        value.stage = "after_restart".to_string();
        StageWriter::default().write(&root, &value);
        assert_eq!(read_latest_stage(&root).unwrap().stage, "after_restart");
        fs::write(directory.join(STAGE_SLOTS[1]), b"{torn").unwrap();
        assert_eq!(read_latest_stage(&root).unwrap().stage, "stage_100");
        value.stage = "after_recovery".to_string();
        StageWriter::default().write(&root, &value);
        assert_eq!(read_latest_stage(&root).unwrap().stage, "after_recovery");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_marker_remains_readable_and_unsafe_slot_is_not_overwritten() {
        let root = crate::diagnostics::tests::test_root("stage-legacy");
        let directory = ensure_area(&root, OPERATIONS_DIRECTORY).unwrap();
        let value = Breadcrumb {
            timestamp: "2026-10-25T12:00:00.000Z".to_string(),
            operation: "synthetic-operation".to_string(),
            stage: "legacy".to_string(),
            details: BTreeMap::new(),
        };
        fs::write(
            directory.join("last-stage-20261025-120000-001-0001.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        fs::write(
            directory.join("last-stage-20261025-120000-002-0002.json"),
            b"{torn",
        )
        .unwrap();
        assert_eq!(read_latest_stage(&root).unwrap().stage, "legacy");
        fs::create_dir(directory.join(STAGE_SLOTS[1])).unwrap();
        StageWriter::default().write(&root, &value);
        assert!(directory.join(STAGE_SLOTS[1]).is_dir());
        assert_eq!(read_latest_stage(&root).unwrap().stage, "legacy");
        fs::remove_dir_all(root).unwrap();
    }
}
