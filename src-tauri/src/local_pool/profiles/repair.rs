use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::io::BufRead;

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use zenith_relay_core::unix_time_ms as now_ms;

mod paths;
use paths::{
    canonical_child, db_error, io_error, path_string, portable_canonicalize, portable_path_value,
    sibling_path, sync_file, validate_id, validate_target_provider,
};

const SNAPSHOT_VERSION: u32 = 1;
const PREVIEW_TTL_MS: u64 = 30 * 60 * 1_000;
const MAX_PROFILES: usize = 8;
const MAX_ROLLOUT_FILES: usize = 4_096;
const MAX_ROLLOUT_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_TOTAL_REWRITE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ROLLOUT_HEADER_BYTES: usize = 1024 * 1024;
const MAX_DATABASE_FILES: usize = 64;
const MAX_REPAIR_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_HISTORY_REPAIR_BACKUPS: usize = 1;
const HISTORY_REPAIR_BACKUP_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

mod types;
use types::{
    BackupEntry, BackupTimestamp, DatabaseCatalogThreadSnapshot, DatabaseSnapshot,
    DatabaseThreadSnapshot, RepairManifest, RepairSnapshot, RolloutCollection, RolloutSnapshot,
    SessionMeta, SessionMetadata,
};
pub use types::{RepairPreview, RepairResult, RollbackResult, TargetProvider};

pub fn preview(
    state_root: &Path,
    profile_roots: &[PathBuf],
    target_provider: TargetProvider,
    codex_running: bool,
) -> Result<RepairPreview, String> {
    preview_with_rewrite_budget(
        state_root,
        profile_roots,
        target_provider,
        codex_running,
        MAX_TOTAL_REWRITE_BYTES,
    )
}

fn preview_with_rewrite_budget(
    state_root: &Path,
    profile_roots: &[PathBuf],
    target_provider: TargetProvider,
    codex_running: bool,
    mut remaining_rewrite_bytes: u64,
) -> Result<RepairPreview, String> {
    if profile_roots.is_empty() || profile_roots.len() > MAX_PROFILES {
        return Err("repair must select between 1 and 8 profiles".to_string());
    }
    let roots = scan::canonical_profile_roots(profile_roots)?;
    let target = target_provider.as_str();
    let mut rollout_files = Vec::new();
    let mut history_rollouts = Vec::new();
    let mut databases = Vec::new();
    let mut seen = HashSet::new();
    for root in &roots {
        let mut collected_rollouts = RolloutCollection::default();
        for directory in [root.join("sessions"), root.join("archived_sessions")] {
            scan::collect_rollouts(
                &directory,
                root,
                target,
                0,
                &mut seen,
                &mut collected_rollouts,
                &mut remaining_rewrite_bytes,
            )?;
        }
        let profile_rollouts = collected_rollouts.rewrites;
        let profile_history_rollouts = collected_rollouts.history;
        let eligible_rollout_paths = profile_history_rollouts
            .iter()
            .map(|rollout_file| rollout_file.path.clone())
            .collect::<HashSet<_>>();
        let mut eligible_thread_ids =
            scan::session_ids_from_rollouts(profile_history_rollouts.iter());
        let database_paths = scan::collect_history_databases(root, &mut seen)?;
        for path in &database_paths {
            let snapshot = scan::scan_database(
                root,
                path,
                target,
                &eligible_rollout_paths,
                &eligible_thread_ids,
            )?;
            eligible_thread_ids.extend(snapshot.threads.into_iter().map(|thread| thread.id));
        }
        for path in database_paths {
            let snapshot = scan::scan_database(
                root,
                &path,
                target,
                &eligible_rollout_paths,
                &eligible_thread_ids,
            )?;
            if snapshot.rows > 0 {
                databases.push(snapshot);
            }
        }
        rollout_files.extend(profile_rollouts);
        history_rollouts.extend(
            profile_history_rollouts
                .into_iter()
                .filter(|rollout| rollout.records == 0),
        );
    }
    let created_at_ms = now_ms();
    let session_id = format!("repair_{}", uuid::Uuid::new_v4().simple());
    let snapshot = RepairSnapshot {
        version: SNAPSHOT_VERSION,
        session_id,
        target_provider: target.to_string(),
        profile_roots: roots.iter().map(|path| path_string(path)).collect(),
        rollout_files,
        history_rollouts,
        databases,
        created_at_ms,
        expires_at_ms: created_at_ms.saturating_add(PREVIEW_TTL_MS),
    };
    snapshot::save_snapshot(state_root, &snapshot)?;
    Ok(snapshot::preview_from_snapshot(&snapshot, codex_running))
}

pub fn apply(
    state_root: &Path,
    backup_root: &Path,
    session_id: &str,
) -> Result<RepairResult, String> {
    validate_id(session_id, "repair_")?;
    let snapshot = snapshot::load_snapshot(state_root, session_id)?;
    if snapshot.version != SNAPSHOT_VERSION || snapshot.session_id != session_id {
        return Err("repair preview is invalid".to_string());
    }
    validate_target_provider(&snapshot.target_provider)?;
    if now_ms() > snapshot.expires_at_ms {
        return Err("repair preview expired".to_string());
    }
    snapshot::validate_snapshot_paths(&snapshot)?;
    let history_rollouts = snapshot
        .rollout_files
        .iter()
        .chain(snapshot.history_rollouts.iter())
        .collect::<Vec<_>>();
    for expected in &history_rollouts {
        let scanned_rollout =
            scan::scan_rollout(Path::new(&expected.path), &snapshot.target_provider)?;
        if scanned_rollout.hash != expected.hash || scanned_rollout.records != expected.records {
            return Err("ChatGPT rollout files changed after repair preview".to_string());
        }
    }
    let eligible_rollout_paths = history_rollouts
        .iter()
        .map(|rollout_file| rollout_file.path.clone())
        .collect::<HashSet<_>>();
    let mut eligible_thread_ids = scan::session_ids_from_rollouts(history_rollouts.iter().copied());
    for database in &snapshot.databases {
        eligible_thread_ids.extend(database.threads.iter().map(|thread| thread.id.clone()));
    }
    for expected in &snapshot.databases {
        let profile_root =
            scan::profile_root_for_path(&snapshot.profile_roots, Path::new(&expected.path))?;
        let scanned_database = scan::scan_database(
            &profile_root,
            Path::new(&expected.path),
            &snapshot.target_provider,
            &eligible_rollout_paths,
            &eligible_thread_ids,
        )?;
        if scanned_database.hash != expected.hash || scanned_database.rows != expected.rows {
            return Err("ChatGPT history database changed after repair preview".to_string());
        }
    }

    let backup_id = format!("history_repair_{}", uuid::Uuid::new_v4().simple());
    let directory = backup_root.join(&backup_id);
    fs::create_dir_all(&directory).map_err(io_error)?;
    let manifest = snapshot::create_backup(&directory, &backup_id, &snapshot)?;
    let apply_result = snapshot::apply_snapshot(&snapshot);
    if let Err(error) = apply_result {
        let rollback = snapshot::restore_manifest(&manifest, &directory);
        return Err(match rollback {
            Ok(_) => error,
            Err(rollback) => format!("{error}; automatic rollback failed: {rollback}"),
        });
    }
    let _ = fs::remove_file(snapshot::snapshot_path(state_root, session_id)?);
    let _ = fs::remove_dir(state_root.join("repair_previews"));
    let _ = cleanup_history_repair_backups_preserving(backup_root, Some(&backup_id));
    Ok(RepairResult {
        backup_id,
        backup_path: path_string(&directory),
        rollout_records_changed: snapshot
            .rollout_files
            .iter()
            .map(|rollout_file| rollout_file.records)
            .sum(),
        sqlite_rows_changed: snapshot
            .databases
            .iter()
            .map(|database_snapshot| database_snapshot.rows)
            .sum(),
    })
}

pub fn synchronize(
    state_root: &Path,
    backup_root: &Path,
    profile_root: &Path,
    target_provider: TargetProvider,
) -> Result<Option<RepairResult>, String> {
    if !profile_root.is_dir() {
        return Ok(None);
    }
    let preview = preview(
        state_root,
        &[profile_root.to_path_buf()],
        target_provider,
        false,
    )?;
    if preview.rollout_record_count == 0 && preview.sqlite_row_count == 0 {
        let _ = fs::remove_file(snapshot::snapshot_path(state_root, &preview.session_id)?);
        let _ = fs::remove_dir(state_root.join("repair_previews"));
        return Ok(None);
    }
    apply(state_root, backup_root, &preview.session_id).map(Some)
}

pub fn rollback(backup_root: &Path, backup_id: &str) -> Result<RollbackResult, String> {
    validate_id(backup_id, "history_repair_")?;
    let directory = backup_root.join(backup_id);
    let manifest_path = directory.join("manifest.json");
    let manifest: RepairManifest =
        serde_json::from_slice(&fs::read(&manifest_path).map_err(io_error)?)
            .map_err(|_| "repair backup manifest is invalid".to_string())?;
    if manifest.version != SNAPSHOT_VERSION || manifest.backup_id != backup_id {
        return Err("repair backup manifest is invalid".to_string());
    }
    snapshot::validate_manifest_paths(&manifest, &directory)?;
    let files_restored = snapshot::restore_manifest(&manifest, &directory)?;
    Ok(RollbackResult {
        backup_id: backup_id.to_string(),
        files_restored,
    })
}

pub fn discard(backup_root: &Path, backup_id: &str) -> Result<(), String> {
    validate_id(backup_id, "history_repair_")?;
    let directory = backup_root.join(backup_id);
    if directory.exists() {
        fs::remove_dir_all(directory).map_err(io_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

mod cleanup;
use cleanup::cleanup_history_repair_backups_preserving;
pub use cleanup::{cleanup_expired_previews, cleanup_history_repair_backups};
mod scan;
mod snapshot;
