//! Read Codex rollout and history-database state for one repair preview.

use super::*;

mod database;
pub(super) use database::{
    collect_history_databases, profile_root_for_path, scan_database, table_columns,
};

pub(super) fn canonical_profile_roots(profile_roots: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::new();
    for root in profile_roots {
        let root = fs::canonicalize(root).map_err(io_error)?;
        if !root.is_dir() {
            return Err("repair profile path is not a directory".to_string());
        }
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    Ok(roots)
}

pub(super) fn collect_rollouts(
    directory: &Path,
    root: &Path,
    target: &str,
    depth: usize,
    seen: &mut HashSet<PathBuf>,
    collection: &mut RolloutCollection,
    remaining_rewrite_bytes: &mut u64,
) -> Result<(), String> {
    if !directory.exists() {
        return Ok(());
    }
    if depth > 8 {
        return Err("ChatGPT session directory is too deeply nested".to_string());
    }
    for directory_entry in fs::read_dir(directory).map_err(io_error)? {
        let directory_entry = directory_entry.map_err(io_error)?;
        let metadata = fs::symlink_metadata(directory_entry.path()).map_err(io_error)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_rollouts(
                &directory_entry.path(),
                root,
                target,
                depth + 1,
                seen,
                collection,
                remaining_rewrite_bytes,
            )?;
            continue;
        }
        if directory_entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("jsonl")
        {
            continue;
        }
        if seen.len() >= MAX_ROLLOUT_FILES {
            return Err("repair rollout file limit exceeded".to_string());
        }
        if metadata.len() > MAX_ROLLOUT_BYTES {
            return Err("ChatGPT rollout file is too large".to_string());
        }
        let path = canonical_child(root, &directory_entry.path())?;
        if !seen.insert(path.clone()) {
            continue;
        }
        let snapshot = scan_rollout(&path, target)?;
        if snapshot.session_meta_count > 0 {
            collection.history.push(snapshot.clone());
        }
        if snapshot.records > 0 {
            // This bounds backup and rewrite I/O, not the size of the user's
            // entire history. Matching files are checked for imported metadata
            // and database reconciliation, but are never copied or rewritten.
            // Charging them here used to block even a same-pool reconnect once
            // unrelated, already-correct history exceeded 4 GiB.
            *remaining_rewrite_bytes = remaining_rewrite_bytes
                .checked_sub(metadata.len())
                .ok_or_else(|| "repair rollout data limit exceeded".to_string())?;
            collection.rewrites.push(snapshot);
        }
    }
    Ok(())
}

pub(super) fn scan_rollout(path: &Path, target: &str) -> Result<RolloutSnapshot, String> {
    let file = File::open(path).map_err(io_error)?;
    if file.metadata().map_err(io_error)?.len() > MAX_ROLLOUT_BYTES {
        return Err("ChatGPT rollout file is too large".to_string());
    }
    let mut hasher = Sha256::new();
    let metadata = read_session_metadata_from(BufReader::new(file), Some(&mut hasher))?;
    let replacement_count = session_meta_replacements(&metadata, target).len();
    let mut session_ids = metadata
        .records
        .iter()
        .filter_map(|session_metadata| session_meta_thread_id(&session_metadata.session_record))
        .collect::<Vec<_>>();
    session_ids.sort();
    session_ids.dedup();
    let session_meta_count = metadata.records.len();
    Ok(RolloutSnapshot {
        path: path_string(path),
        hash: hex::encode(hasher.finalize()),
        records: replacement_count,
        session_ids,
        session_meta_count,
    })
}

pub(super) fn read_session_metadata(path: &Path) -> Result<SessionMetadata, String> {
    let file = File::open(path).map_err(io_error)?;
    if file.metadata().map_err(io_error)?.len() > MAX_ROLLOUT_BYTES {
        return Err("ChatGPT rollout file is too large".to_string());
    }
    read_session_metadata_from(BufReader::new(file), None)
}

pub(super) fn read_session_metadata_from(
    mut reader: impl Read,
    mut hasher: Option<&mut Sha256>,
) -> Result<SessionMetadata, String> {
    let mut metadata = SessionMetadata {
        records: Vec::new(),
    };
    let mut line = Vec::new();
    let mut line_too_large = false;
    let mut line_start = 0_u64;
    let mut offset = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        if let Some(hasher) = hasher.as_mut() {
            hasher.update(&buffer[..read]);
        }
        for byte in &buffer[..read] {
            if !line_too_large {
                if line.len() >= MAX_ROLLOUT_HEADER_BYTES {
                    line.clear();
                    line_too_large = true;
                } else {
                    line.push(*byte);
                }
            }
            offset += 1;
            if *byte != b'\n' {
                continue;
            }
            if line_too_large {
                if line_start == 0 {
                    return Err("ChatGPT rollout session metadata is too large".to_string());
                }
            } else {
                record_session_metadata(&mut metadata, &line, line_start, offset);
            }
            line.clear();
            line_too_large = false;
            line_start = offset;
        }
    }
    if line_start < offset {
        if line_too_large {
            if line_start == 0 {
                return Err("ChatGPT rollout session metadata is too large".to_string());
            }
        } else {
            record_session_metadata(&mut metadata, &line, line_start, offset);
        }
    }
    Ok(metadata)
}

pub(super) fn record_session_metadata(
    metadata: &mut SessionMetadata,
    line: &[u8],
    start: u64,
    end: u64,
) {
    let Some((session_record, separator)) = session_meta_value(line) else {
        return;
    };
    let session_metadata = SessionMeta {
        start,
        end,
        separator,
        session_record,
    };
    metadata.records.push(session_metadata);
}

#[cfg(test)]
pub(super) fn rollout_provider(first_line: &[u8]) -> Option<Option<String>> {
    session_meta_value(first_line)
        .map(|(session_record, _)| session_meta_provider(&session_record).map(str::to_string))
}

pub(super) fn session_meta_value(line: &[u8]) -> Option<(Value, Vec<u8>)> {
    let (line, separator) = if let Some(line) = line.strip_suffix(b"\r\n") {
        (line, b"\r\n".as_slice())
    } else if let Some(line) = line.strip_suffix(b"\n") {
        (line, b"\n".as_slice())
    } else {
        (line, b"".as_slice())
    };
    // Ignore message/tool payloads without constructing a JSON tree for every
    // event. Still inspect every record: imported files can contain more than
    // one session_meta, and JSON field order is not significant.
    #[derive(Deserialize)]
    struct RecordKind {
        #[serde(rename = "type")]
        kind: Kind,
    }
    #[derive(Deserialize)]
    enum Kind {
        #[serde(rename = "session_meta")]
        SessionMeta,
        #[serde(other)]
        Other,
    }
    let record_kind: RecordKind = serde_json::from_slice(line).ok()?;
    match record_kind.kind {
        Kind::SessionMeta => Some((serde_json::from_slice(line).ok()?, separator.to_vec())),
        Kind::Other => None,
    }
}

pub(super) fn session_meta_provider(session_record: &Value) -> Option<&str> {
    session_record
        .get("payload")
        .and_then(|session_payload| session_payload.get("model_provider"))
        .and_then(Value::as_str)
}

pub(super) fn session_meta_thread_id(session_record: &Value) -> Option<String> {
    session_record
        .get("payload")
        .and_then(|session_payload| {
            session_payload
                .get("id")
                .or_else(|| session_payload.get("session_id"))
        })
        .or_else(|| {
            session_record
                .get("id")
                .or_else(|| session_record.get("session_id"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|thread_id| !thread_id.is_empty())
        .map(str::to_string)
}

pub(super) fn session_ids_from_rollouts<'a>(
    rollouts: impl IntoIterator<Item = &'a RolloutSnapshot>,
) -> HashSet<String> {
    rollouts
        .into_iter()
        .flat_map(|rollout| rollout.session_ids.iter().cloned())
        .collect()
}

pub(super) fn session_meta_replacements(
    metadata: &SessionMetadata,
    target: &str,
) -> Vec<SessionMeta> {
    metadata
        .records
        .iter()
        .filter(|session_metadata| {
            session_meta_provider(&session_metadata.session_record) != Some(target)
        })
        .cloned()
        .collect()
}
