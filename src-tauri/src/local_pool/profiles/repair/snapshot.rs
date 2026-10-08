//! Persist a repair preview and write or restore its files.

use super::*;

pub(super) fn preview_from_snapshot(
    snapshot: &RepairSnapshot,
    codex_running: bool,
) -> RepairPreview {
    RepairPreview {
        session_id: snapshot.session_id.clone(),
        target_provider: snapshot.target_provider.clone(),
        profile_count: snapshot.profile_roots.len(),
        rollout_file_count: snapshot.rollout_files.len(),
        rollout_record_count: snapshot
            .rollout_files
            .iter()
            .map(|rollout_file| rollout_file.records)
            .sum(),
        sqlite_row_count: snapshot
            .databases
            .iter()
            .map(|database_snapshot| database_snapshot.rows)
            .sum(),
        codex_running,
        expires_at_ms: snapshot.expires_at_ms,
    }
}

pub(super) fn save_snapshot(state_root: &Path, snapshot: &RepairSnapshot) -> Result<(), String> {
    let _ = cleanup_expired_previews(state_root);
    let directory = state_root.join("repair_previews");
    fs::create_dir_all(&directory).map_err(io_error)?;
    let path = snapshot_path(state_root, &snapshot.session_id)?;
    let bytes = serde_json::to_vec(snapshot)
        .map_err(|_| "repair preview serialization failed".to_string())?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

pub(super) fn load_snapshot(state_root: &Path, session_id: &str) -> Result<RepairSnapshot, String> {
    let bytes = fs::read(snapshot_path(state_root, session_id)?).map_err(io_error)?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("repair preview is too large".to_string());
    }
    serde_json::from_slice(&bytes).map_err(|_| "repair preview is invalid".to_string())
}

pub(super) fn snapshot_path(state_root: &Path, session_id: &str) -> Result<PathBuf, String> {
    validate_id(session_id, "repair_")?;
    Ok(state_root
        .join("repair_previews")
        .join(format!("{session_id}.json")))
}

pub(super) fn validate_snapshot_paths(snapshot: &RepairSnapshot) -> Result<(), String> {
    let roots = snapshot
        .profile_roots
        .iter()
        .map(|path| PathBuf::from(portable_path_value(path)))
        .collect::<Vec<_>>();
    for path in snapshot
        .rollout_files
        .iter()
        .map(|rollout_file| &rollout_file.path)
        .chain(
            snapshot
                .history_rollouts
                .iter()
                .map(|history_rollout| &history_rollout.path),
        )
        .chain(
            snapshot
                .databases
                .iter()
                .map(|database_snapshot| &database_snapshot.path),
        )
    {
        let canonical = portable_canonicalize(Path::new(path))?;
        if !roots.iter().any(|root| canonical.starts_with(root)) {
            return Err("repair preview path escaped its profile".to_string());
        }
    }
    Ok(())
}
mod apply;

#[cfg(test)]
pub(super) use apply::rewrite_rollout;
pub(super) use apply::{apply_snapshot, create_backup, restore_manifest, validate_manifest_paths};
