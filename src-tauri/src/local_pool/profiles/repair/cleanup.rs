use super::*;

pub fn cleanup_history_repair_backups(backup_root: &Path) -> Result<usize, String> {
    cleanup_history_repair_backups_preserving(backup_root, None)
}

pub fn cleanup_expired_previews(state_root: &Path) -> Result<usize, String> {
    let directory = state_root.join("repair_previews");
    if !directory.exists() {
        return Ok(0);
    }
    let now = now_ms();
    let mut stale = Vec::new();
    for directory_entry in fs::read_dir(&directory).map_err(io_error)? {
        let directory_entry = directory_entry.map_err(io_error)?;
        let file_type = directory_entry.file_type().map_err(io_error)?;
        if !file_type.is_file() || file_type.is_symlink() {
            continue;
        }
        let path = directory_entry.path();
        let Some(session_id) = path.file_stem().and_then(|file_stem| file_stem.to_str()) else {
            continue;
        };
        if path
            .extension()
            .and_then(|file_extension| file_extension.to_str())
            != Some("json")
            || validate_id(session_id, "repair_").is_err()
            || directory_entry.metadata().map_err(io_error)?.len() > MAX_REPAIR_MANIFEST_BYTES
        {
            continue;
        }
        let Ok(snapshot) = fs::read(&path).map_err(io_error).and_then(|bytes| {
            serde_json::from_slice::<RepairSnapshot>(&bytes)
                .map_err(|_| "repair preview is invalid".to_string())
        }) else {
            continue;
        };
        if snapshot.version == SNAPSHOT_VERSION
            && snapshot.session_id == session_id
            && snapshot.expires_at_ms <= now
        {
            stale.push(path);
        }
    }
    for path in &stale {
        fs::remove_file(path).map_err(io_error)?;
    }
    Ok(stale.len())
}

pub(super) fn cleanup_history_repair_backups_preserving(
    backup_root: &Path,
    preserve_id: Option<&str>,
) -> Result<usize, String> {
    if !backup_root.exists() {
        return Ok(0);
    }
    let now = now_ms();
    let mut backups = Vec::new();
    for directory_entry in fs::read_dir(backup_root).map_err(io_error)? {
        let directory_entry = directory_entry.map_err(io_error)?;
        let file_type = directory_entry.file_type().map_err(io_error)?;
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let Some(name) = directory_entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if validate_id(&name, "history_repair_").is_err() {
            continue;
        }
        backups.push((
            backup_created_at_ms(&directory_entry.path()),
            name,
            directory_entry.path(),
        ));
    }
    backups.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    let mut keep = HashSet::new();
    if let Some(id) = preserve_id {
        if backups.iter().any(|(_, name, _)| name == id) {
            keep.insert(id.to_string());
        }
    }
    for (created_at_ms, name, _) in &backups {
        let expired = created_at_ms.saturating_add(HISTORY_REPAIR_BACKUP_TTL_MS) <= now;
        if !expired && keep.len() < MAX_HISTORY_REPAIR_BACKUPS {
            keep.insert(name.clone());
        }
    }
    let stale = backups
        .into_iter()
        .filter(|(_, name, _)| !keep.contains(name))
        .map(|(_, _, path)| path)
        .collect::<Vec<_>>();
    for path in &stale {
        fs::remove_dir_all(path).map_err(io_error)?;
    }
    Ok(stale.len())
}

fn backup_created_at_ms(directory: &Path) -> u64 {
    let manifest = directory.join("manifest.json");
    let declared = fs::metadata(&manifest)
        .ok()
        .filter(|metadata| metadata.is_file() && metadata.len() <= MAX_REPAIR_MANIFEST_BYTES)
        .and_then(|_| fs::read(manifest).ok())
        .and_then(|bytes| serde_json::from_slice::<BackupTimestamp>(&bytes).ok())
        .map(|timestamp| timestamp.created_at_ms)
        .filter(|timestamp| *timestamp > 0);
    declared.unwrap_or_else(|| {
        fs::metadata(directory)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default()
    })
}
