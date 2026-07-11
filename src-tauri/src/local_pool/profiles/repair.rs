use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const SNAPSHOT_VERSION: u32 = 1;
const PREVIEW_TTL_MS: u64 = 30 * 60 * 1_000;
const MAX_PROFILES: usize = 8;
const MAX_ROLLOUT_FILES: usize = 4_096;
const MAX_ROLLOUT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_ROLLOUT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetProvider {
    Openai,
    ZenithRelayLocal,
}

impl TargetProvider {
    fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::ZenithRelayLocal => "zenith_relay_local",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairPreview {
    pub session_id: String,
    pub target_provider: String,
    pub profile_count: usize,
    pub rollout_file_count: usize,
    pub rollout_record_count: usize,
    pub sqlite_row_count: usize,
    pub codex_running: bool,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairResult {
    pub backup_id: String,
    pub backup_path: String,
    pub rollout_records_changed: usize,
    pub sqlite_rows_changed: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackResult {
    pub backup_id: String,
    pub files_restored: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepairSnapshot {
    version: u32,
    session_id: String,
    target_provider: String,
    profile_roots: Vec<String>,
    rollout_files: Vec<RolloutSnapshot>,
    databases: Vec<DatabaseSnapshot>,
    created_at_ms: u64,
    expires_at_ms: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RolloutSnapshot {
    path: String,
    hash: String,
    records: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DatabaseSnapshot {
    path: String,
    hash: String,
    rows: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepairManifest {
    version: u32,
    backup_id: String,
    profile_roots: Vec<String>,
    entries: Vec<BackupEntry>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupEntry {
    original_path: String,
    backup_path: String,
    sqlite: bool,
}

pub fn preview(
    state_root: &Path,
    profile_roots: &[PathBuf],
    target_provider: TargetProvider,
    codex_running: bool,
) -> Result<RepairPreview, String> {
    if profile_roots.is_empty() || profile_roots.len() > MAX_PROFILES {
        return Err("repair must select between 1 and 8 profiles".to_string());
    }
    let roots = canonical_profile_roots(profile_roots)?;
    let target = target_provider.as_str();
    let mut rollout_files = Vec::new();
    let mut databases = Vec::new();
    let mut seen = HashSet::new();
    let mut total_bytes = 0_u64;
    for root in &roots {
        for directory in [root.join("sessions"), root.join("archived_sessions")] {
            collect_rollouts(
                &directory,
                root,
                target,
                0,
                &mut seen,
                &mut rollout_files,
                &mut total_bytes,
            )?;
        }
        for path in [
            root.join("state_5.sqlite"),
            root.join("sqlite").join("state_5.sqlite"),
        ] {
            if !path.is_file() {
                continue;
            }
            let path = canonical_child(root, &path)?;
            if !seen.insert(path.clone()) {
                continue;
            }
            let snapshot = scan_database(&path, target)?;
            if snapshot.rows > 0 {
                databases.push(snapshot);
            }
        }
    }
    let created_at_ms = now_ms();
    let session_id = format!("repair_{}", uuid::Uuid::new_v4().simple());
    let snapshot = RepairSnapshot {
        version: SNAPSHOT_VERSION,
        session_id: session_id.clone(),
        target_provider: target.to_string(),
        profile_roots: roots.iter().map(|path| path_string(path)).collect(),
        rollout_files,
        databases,
        created_at_ms,
        expires_at_ms: created_at_ms.saturating_add(PREVIEW_TTL_MS),
    };
    save_snapshot(state_root, &snapshot)?;
    Ok(preview_from_snapshot(&snapshot, codex_running))
}

pub fn apply(
    state_root: &Path,
    backup_root: &Path,
    session_id: &str,
) -> Result<RepairResult, String> {
    validate_id(session_id, "repair_")?;
    let snapshot = load_snapshot(state_root, session_id)?;
    if snapshot.version != SNAPSHOT_VERSION || snapshot.session_id != session_id {
        return Err("repair preview is invalid".to_string());
    }
    validate_target_provider(&snapshot.target_provider)?;
    if now_ms() > snapshot.expires_at_ms {
        return Err("repair preview expired".to_string());
    }
    validate_snapshot_paths(&snapshot)?;
    for expected in &snapshot.rollout_files {
        let current = scan_rollout(Path::new(&expected.path), &snapshot.target_provider)?;
        if current.hash != expected.hash || current.records != expected.records {
            return Err("Codex rollout files changed after repair preview".to_string());
        }
    }
    for expected in &snapshot.databases {
        let current = scan_database(Path::new(&expected.path), &snapshot.target_provider)?;
        if current.hash != expected.hash || current.rows != expected.rows {
            return Err("Codex history database changed after repair preview".to_string());
        }
    }

    let backup_id = format!("history_repair_{}", uuid::Uuid::new_v4().simple());
    let directory = backup_root.join(&backup_id);
    fs::create_dir_all(&directory).map_err(io_error)?;
    let manifest = create_backup(&directory, &backup_id, &snapshot)?;
    let result = apply_snapshot(&snapshot);
    if let Err(error) = result {
        let rollback = restore_manifest(&manifest, &directory);
        return Err(match rollback {
            Ok(_) => error,
            Err(rollback) => format!("{error}; automatic rollback failed: {rollback}"),
        });
    }
    let _ = fs::remove_file(snapshot_path(state_root, session_id)?);
    Ok(RepairResult {
        backup_id,
        backup_path: path_string(&directory),
        rollout_records_changed: snapshot.rollout_files.iter().map(|item| item.records).sum(),
        sqlite_rows_changed: snapshot.databases.iter().map(|item| item.rows).sum(),
    })
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
    validate_manifest_paths(&manifest, &directory)?;
    let files_restored = restore_manifest(&manifest, &directory)?;
    Ok(RollbackResult {
        backup_id: backup_id.to_string(),
        files_restored,
    })
}

fn preview_from_snapshot(snapshot: &RepairSnapshot, codex_running: bool) -> RepairPreview {
    RepairPreview {
        session_id: snapshot.session_id.clone(),
        target_provider: snapshot.target_provider.clone(),
        profile_count: snapshot.profile_roots.len(),
        rollout_file_count: snapshot.rollout_files.len(),
        rollout_record_count: snapshot.rollout_files.iter().map(|item| item.records).sum(),
        sqlite_row_count: snapshot.databases.iter().map(|item| item.rows).sum(),
        codex_running,
        expires_at_ms: snapshot.expires_at_ms,
    }
}

fn canonical_profile_roots(profile_roots: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
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

fn collect_rollouts(
    directory: &Path,
    root: &Path,
    target: &str,
    depth: usize,
    seen: &mut HashSet<PathBuf>,
    snapshots: &mut Vec<RolloutSnapshot>,
    total_bytes: &mut u64,
) -> Result<(), String> {
    if !directory.exists() {
        return Ok(());
    }
    if depth > 8 {
        return Err("Codex session directory is too deeply nested".to_string());
    }
    for entry in fs::read_dir(directory).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_rollouts(
                &entry.path(),
                root,
                target,
                depth + 1,
                seen,
                snapshots,
                total_bytes,
            )?;
            continue;
        }
        if entry.path().extension().and_then(|value| value.to_str()) != Some("jsonl") {
            continue;
        }
        if seen.len() >= MAX_ROLLOUT_FILES {
            return Err("repair rollout file limit exceeded".to_string());
        }
        if metadata.len() > MAX_ROLLOUT_BYTES {
            return Err("Codex rollout file is too large".to_string());
        }
        *total_bytes = total_bytes.saturating_add(metadata.len());
        if *total_bytes > MAX_TOTAL_ROLLOUT_BYTES {
            return Err("repair rollout data limit exceeded".to_string());
        }
        let path = canonical_child(root, &entry.path())?;
        if !seen.insert(path.clone()) {
            continue;
        }
        let snapshot = scan_rollout(&path, target)?;
        if snapshot.records > 0 {
            snapshots.push(snapshot);
        }
    }
    Ok(())
}

fn scan_rollout(path: &Path, target: &str) -> Result<RolloutSnapshot, String> {
    let bytes = fs::read(path).map_err(io_error)?;
    if bytes.len() as u64 > MAX_ROLLOUT_BYTES {
        return Err("Codex rollout file is too large".to_string());
    }
    let content =
        std::str::from_utf8(&bytes).map_err(|_| "Codex rollout is not UTF-8".to_string())?;
    let mut records = 0;
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line)
            .map_err(|_| "Codex rollout contains malformed JSON".to_string())?;
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            let provider = value
                .get("payload")
                .and_then(|payload| payload.get("model_provider"))
                .and_then(Value::as_str);
            if target.is_empty() || provider != Some(target) {
                records += 1;
            }
        }
    }
    Ok(RolloutSnapshot {
        path: path_string(path),
        hash: hex_hash(&bytes),
        records,
    })
}

fn scan_database(path: &Path, target: &str) -> Result<DatabaseSnapshot, String> {
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(db_error)?;
    let exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='threads')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(db_error)?;
    if !exists {
        return Ok(DatabaseSnapshot {
            path: path_string(path),
            hash: hex_hash(&[]),
            rows: 0,
        });
    }
    let mut statement = connection
        .prepare("SELECT id, model_provider, rollout_path FROM threads WHERE model_provider <> ?1 ORDER BY id")
        .map_err(db_error)?;
    let rows = statement
        .query_map([target], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    let mut hasher = Sha256::new();
    for (id, provider, rollout_path) in &rows {
        for value in [id, provider, rollout_path] {
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
    }
    Ok(DatabaseSnapshot {
        path: path_string(path),
        hash: format!("{:x}", hasher.finalize()),
        rows: rows.len(),
    })
}

fn save_snapshot(state_root: &Path, snapshot: &RepairSnapshot) -> Result<(), String> {
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

fn load_snapshot(state_root: &Path, session_id: &str) -> Result<RepairSnapshot, String> {
    let bytes = fs::read(snapshot_path(state_root, session_id)?).map_err(io_error)?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("repair preview is too large".to_string());
    }
    serde_json::from_slice(&bytes).map_err(|_| "repair preview is invalid".to_string())
}

fn snapshot_path(state_root: &Path, session_id: &str) -> Result<PathBuf, String> {
    validate_id(session_id, "repair_")?;
    Ok(state_root
        .join("repair_previews")
        .join(format!("{session_id}.json")))
}

fn validate_snapshot_paths(snapshot: &RepairSnapshot) -> Result<(), String> {
    let roots = snapshot
        .profile_roots
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    for path in snapshot
        .rollout_files
        .iter()
        .map(|item| &item.path)
        .chain(snapshot.databases.iter().map(|item| &item.path))
    {
        let canonical = fs::canonicalize(path).map_err(io_error)?;
        if !roots.iter().any(|root| canonical.starts_with(root)) {
            return Err("repair preview path escaped its profile".to_string());
        }
    }
    Ok(())
}

fn create_backup(
    directory: &Path,
    backup_id: &str,
    snapshot: &RepairSnapshot,
) -> Result<RepairManifest, String> {
    let mut entries = Vec::new();
    for (index, item) in snapshot.rollout_files.iter().enumerate() {
        let relative = PathBuf::from("rollouts").join(format!("{index}.jsonl"));
        let target = directory.join(&relative);
        fs::create_dir_all(target.parent().unwrap()).map_err(io_error)?;
        fs::copy(&item.path, &target).map_err(io_error)?;
        sync_file(&target)?;
        entries.push(BackupEntry {
            original_path: item.path.clone(),
            backup_path: path_string(&relative),
            sqlite: false,
        });
    }
    for (index, item) in snapshot.databases.iter().enumerate() {
        let relative = PathBuf::from("databases").join(format!("{index}.sqlite"));
        let target = directory.join(&relative);
        fs::create_dir_all(target.parent().unwrap()).map_err(io_error)?;
        Connection::open(&item.path)
            .map_err(db_error)?
            .backup(rusqlite::MAIN_DB, &target, None)
            .map_err(db_error)?;
        sync_file(&target)?;
        entries.push(BackupEntry {
            original_path: item.path.clone(),
            backup_path: path_string(&relative),
            sqlite: true,
        });
    }
    let manifest = RepairManifest {
        version: SNAPSHOT_VERSION,
        backup_id: backup_id.to_string(),
        profile_roots: snapshot.profile_roots.clone(),
        entries,
    };
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|_| "repair backup manifest serialization failed".to_string())?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join("manifest.json"))
        .map_err(io_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    Ok(manifest)
}

fn apply_snapshot(snapshot: &RepairSnapshot) -> Result<(), String> {
    for item in &snapshot.rollout_files {
        rewrite_rollout(
            Path::new(&item.path),
            &snapshot.target_provider,
            item.records,
        )?;
    }
    for item in &snapshot.databases {
        let mut connection = Connection::open(&item.path).map_err(db_error)?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let changed = transaction
            .execute(
                "UPDATE threads SET model_provider = ?1 WHERE model_provider <> ?1",
                [&snapshot.target_provider],
            )
            .map_err(db_error)?;
        if changed != item.rows {
            return Err("Codex history database changed during repair".to_string());
        }
        transaction.commit().map_err(db_error)?;
    }
    Ok(())
}

fn rewrite_rollout(path: &Path, target: &str, expected: usize) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(io_error)?;
    let mut changed = 0;
    let mut output = String::with_capacity(content.len());
    for line in content.lines() {
        if line.trim().is_empty() {
            output.push('\n');
            continue;
        }
        let mut value: Value = serde_json::from_str(line)
            .map_err(|_| "Codex rollout contains malformed JSON".to_string())?;
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            let provider = value
                .get("payload")
                .and_then(|payload| payload.get("model_provider"))
                .and_then(Value::as_str);
            if provider != Some(target) {
                let payload = value
                    .get_mut("payload")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(|| "Codex session metadata is invalid".to_string())?;
                payload.insert(
                    "model_provider".to_string(),
                    Value::String(target.to_string()),
                );
                output.push_str(
                    &serde_json::to_string(&value)
                        .map_err(|_| "Codex session serialization failed".to_string())?,
                );
                changed += 1;
            } else {
                output.push_str(line);
            }
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    if changed != expected {
        return Err("Codex rollout changed during repair".to_string());
    }
    replace_file(path, output.as_bytes(), false)
}

fn restore_manifest(manifest: &RepairManifest, directory: &Path) -> Result<usize, String> {
    validate_manifest_paths(manifest, directory)?;
    let directory = fs::canonicalize(directory).map_err(io_error)?;
    let mut restored = 0;
    for entry in &manifest.entries {
        let relative = Path::new(&entry.backup_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err("repair backup path is invalid".to_string());
        }
        let backup = canonical_child(&directory, &directory.join(relative))?;
        let bytes = fs::read(backup).map_err(io_error)?;
        replace_file(Path::new(&entry.original_path), &bytes, entry.sqlite)?;
        restored += 1;
    }
    Ok(restored)
}

fn validate_manifest_paths(manifest: &RepairManifest, directory: &Path) -> Result<(), String> {
    let directory = fs::canonicalize(directory).map_err(io_error)?;
    let roots = manifest
        .profile_roots
        .iter()
        .map(fs::canonicalize)
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_error)?;
    for entry in &manifest.entries {
        let original = fs::canonicalize(&entry.original_path).map_err(io_error)?;
        if !roots.iter().any(|root| original.starts_with(root)) {
            return Err("repair manifest path escaped its profile".to_string());
        }
        let relative = Path::new(&entry.backup_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err("repair backup path is invalid".to_string());
        }
        let backup = canonical_child(&directory, &directory.join(relative))?;
        if !backup.is_file() {
            return Err("repair backup file is missing".to_string());
        }
    }
    Ok(())
}

fn replace_file(path: &Path, bytes: &[u8], sqlite: bool) -> Result<(), String> {
    let temporary = sibling_path(
        path,
        &format!(".repair-{}.tmp", uuid::Uuid::new_v4().simple()),
    );
    let previous = sibling_path(path, ".repair-previous");
    if previous.exists() {
        fs::remove_file(&previous).map_err(io_error)?;
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    fs::rename(path, &previous).map_err(io_error)?;
    if sqlite {
        for suffix in ["-wal", "-shm"] {
            let sidecar = sibling_path(path, suffix);
            if sidecar.exists() {
                if let Err(error) = fs::remove_file(sidecar) {
                    let _ = fs::rename(&previous, path);
                    let _ = fs::remove_file(&temporary);
                    return Err(io_error(error));
                }
            }
        }
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::rename(&previous, path);
        let _ = fs::remove_file(&temporary);
        return Err(io_error(error));
    }
    fs::remove_file(previous).map_err(io_error)
}

fn canonical_child(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    if !canonical.starts_with(root) {
        return Err("repair path escaped its profile".to_string());
    }
    Ok(canonical)
}

fn validate_target_provider(value: &str) -> Result<(), String> {
    if matches!(value, "openai" | "zenith_relay_local") {
        Ok(())
    } else {
        Err("repair target provider is invalid".to_string())
    }
}

fn sync_file(path: &Path) -> Result<(), String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(io_error)
}

fn validate_id(value: &str, prefix: &str) -> Result<(), String> {
    if value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        Ok(())
    } else {
        Err("repair identifier is invalid".to_string())
    }
}

fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn hex_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

fn db_error(error: rusqlite::Error) -> String {
    format!("Codex history database operation failed: {error}")
}

fn io_error(error: std::io::Error) -> String {
    format!("Codex history repair I/O failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_apply_and_rollback_repair_only_provider_metadata() {
        let (root, state, backups, profile, rollout, database) = fixture("round-trip");
        let preview = preview(
            &state,
            std::slice::from_ref(&profile),
            TargetProvider::ZenithRelayLocal,
            true,
        )
        .unwrap();
        assert_eq!(preview.profile_count, 1);
        assert_eq!(preview.rollout_file_count, 1);
        assert_eq!(preview.rollout_record_count, 1);
        assert_eq!(preview.sqlite_row_count, 1);
        assert!(preview.codex_running);
        assert!(!serde_json::to_string(&preview)
            .unwrap()
            .contains("synthetic-private-prompt"));

        let applied = apply(&state, &backups, &preview.session_id).unwrap();
        assert_eq!(applied.rollout_records_changed, 1);
        assert_eq!(applied.sqlite_rows_changed, 1);
        assert_eq!(rollout_provider(&rollout), "zenith_relay_local");
        assert_eq!(database_provider(&database), "zenith_relay_local");
        assert!(fs::read_to_string(&rollout)
            .unwrap()
            .contains("synthetic-private-prompt"));

        let rolled_back = rollback(&backups, &applied.backup_id).unwrap();
        assert_eq!(rolled_back.files_restored, 2);
        assert_eq!(rollout_provider(&rollout), "openai");
        assert_eq!(database_provider(&database), "openai");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn changed_rollout_is_rejected_before_backup_or_write() {
        let (root, state, backups, profile, rollout, database) = fixture("changed");
        let preview = preview(&state, &[profile], TargetProvider::ZenithRelayLocal, false).unwrap();
        let mut file = OpenOptions::new().append(true).open(&rollout).unwrap();
        writeln!(file, "{{\"type\":\"event_msg\",\"payload\":{{}}}}").unwrap();

        let error = apply(&state, &backups, &preview.session_id).unwrap_err();
        assert!(error.contains("changed after repair preview"));
        assert_eq!(rollout_provider(&rollout), "openai");
        assert_eq!(database_provider(&database), "openai");
        assert!(!backups.exists());
        fs::remove_dir_all(root).unwrap();
    }

    fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-repair-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let state = root.join("state");
        let backups = root.join("backups");
        let profile = root.join("profile");
        let session = profile.join("sessions/2026/07/11");
        fs::create_dir_all(&session).unwrap();
        fs::create_dir_all(&state).unwrap();
        let rollout = session.join("rollout-test.jsonl");
        fs::write(
            &rollout,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"thread-test\",\"model_provider\":\"openai\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"content\":\"synthetic-private-prompt\"}}\n"
            ),
        )
        .unwrap();
        let database = profile.join("state_5.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads(id TEXT PRIMARY KEY, model_provider TEXT NOT NULL, rollout_path TEXT NOT NULL);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads(id, model_provider, rollout_path) VALUES ('thread-test', 'openai', ?1)",
                [path_string(&rollout)],
            )
            .unwrap();
        drop(connection);
        (root, state, backups, profile, rollout, database)
    }

    fn rollout_provider(path: &Path) -> String {
        let line = fs::read_to_string(path)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .to_string();
        let value: Value = serde_json::from_str(&line).unwrap();
        value["payload"]["model_provider"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn database_provider(path: &Path) -> String {
        Connection::open(path)
            .unwrap()
            .query_row(
                "SELECT model_provider FROM threads WHERE id='thread-test'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }
}
