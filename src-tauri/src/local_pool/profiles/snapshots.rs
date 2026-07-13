use super::codex::{self, UserProfileSnapshot};
use crate::{
    files::atomic_write,
    local_pool::{
        error::{ErrorCode, LocalPoolError, Result},
        store::secret_store,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const SNAPSHOT_VERSION: u32 = 1;
const PAYLOAD_VERSION: u32 = 1;
const SNAPSHOT_DIR: &str = "snapshots";
const MAX_NAME_CHARS: usize = 80;
const MAX_METADATA_BYTES: u64 = 16 * 1024;
const MAX_PROFILE_FILE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSnapshotSummary {
    pub id: String,
    pub name: String,
    pub profile_dir: String,
    pub created_at_ms: u64,
    pub config_available: bool,
    pub auth_available: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotRecord {
    version: u32,
    id: String,
    name: String,
    profile_dir: String,
    created_at_ms: u64,
    config_available: bool,
    auth_available: bool,
    payload_secret_ref: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotPayload {
    version: u32,
    config: Option<String>,
    auth: Option<String>,
}

pub fn list(backup_root: &Path) -> Result<Vec<ProfileSnapshotSummary>> {
    let root = snapshot_root(backup_root);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(&root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if !entry.file_type().map_err(io_error)?.is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let path = entry.path();
        let record = read_record(&path)?;
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        validate_record(&record, stem)?;
        snapshots.push(summary(&record));
    }
    snapshots.sort_by(|left, right| {
        right
            .created_at_ms
            .cmp(&left.created_at_ms)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(snapshots)
}

pub fn create(codex_home: &Path, backup_root: &Path, name: &str) -> Result<ProfileSnapshotSummary> {
    create_with(codex_home, backup_root, name, &OsSnapshotSecrets)
}

pub fn restore(
    codex_home: &Path,
    backup_root: &Path,
    id: &str,
    safety_name: &str,
) -> Result<ProfileSnapshotSummary> {
    restore_with(codex_home, backup_root, id, safety_name, &OsSnapshotSecrets)
}

pub fn delete(backup_root: &Path, id: &str) -> Result<()> {
    delete_with(backup_root, id, &OsSnapshotSecrets)
}

fn create_with(
    codex_home: &Path,
    backup_root: &Path,
    name: &str,
    secrets: &impl SnapshotSecrets,
) -> Result<ProfileSnapshotSummary> {
    let name = normalize_name(name)?;
    fs::create_dir_all(codex_home).map_err(io_error)?;
    let profile_dir = fs::canonicalize(codex_home).map_err(io_error)?;
    let snapshot = codex::snapshot_user_profile(&profile_dir, backup_root)?;
    validate_profile_content(&snapshot)?;
    let id = Uuid::new_v4().to_string();
    let payload_secret_ref = payload_secret_ref(&id);
    let config_available = snapshot.config.is_some();
    let auth_available = snapshot.auth.is_some();
    let payload = serde_json::to_string(&SnapshotPayload {
        version: PAYLOAD_VERSION,
        config: snapshot.config,
        auth: snapshot.auth,
    })
    .map_err(invalid_data)?;
    secrets.save(&payload_secret_ref, &payload)?;

    let record = SnapshotRecord {
        version: SNAPSHOT_VERSION,
        id: id.clone(),
        name,
        profile_dir: profile_dir.to_string_lossy().into_owned(),
        created_at_ms: now_ms(),
        config_available,
        auth_available,
        payload_secret_ref: payload_secret_ref.clone(),
    };
    let metadata = serde_json::to_string_pretty(&record).map_err(invalid_data)?;
    let path = metadata_path(backup_root, &id)?;
    if let Err(error) = atomic_write(&path, &format!("{metadata}\n")).map_err(io_error_message) {
        return Err(with_cleanup(error, secrets.delete(&payload_secret_ref)));
    }
    Ok(summary(&record))
}

fn restore_with(
    codex_home: &Path,
    backup_root: &Path,
    id: &str,
    safety_name: &str,
    secrets: &impl SnapshotSecrets,
) -> Result<ProfileSnapshotSummary> {
    let path = metadata_path(backup_root, id)?;
    let record = read_record(&path)?;
    validate_record(&record, id)?;
    let profile_dir = fs::canonicalize(codex_home).map_err(io_error)?;
    if record.profile_dir != profile_dir.to_string_lossy() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "Codex snapshot belongs to another profile",
        ));
    }
    let payload = load_payload(&record, secrets)?;
    let safety = create_with(&profile_dir, backup_root, safety_name, secrets)?;
    codex::restore_user_profile_snapshot(
        &profile_dir,
        backup_root,
        &UserProfileSnapshot {
            config: payload.config,
            auth: payload.auth,
        },
    )?;
    Ok(safety)
}

fn delete_with(backup_root: &Path, id: &str, secrets: &impl SnapshotSecrets) -> Result<()> {
    let path = metadata_path(backup_root, id)?;
    let bytes = read_bounded(&path, MAX_METADATA_BYTES)?;
    let content = std::str::from_utf8(&bytes).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Codex snapshot metadata is not UTF-8",
        )
    })?;
    let record: SnapshotRecord = serde_json::from_str(content).map_err(invalid_data)?;
    validate_record(&record, id)?;
    if fs::read(&path).map_err(io_error)? != bytes {
        return Err(snapshot_changed());
    }
    fs::remove_file(&path).map_err(io_error)?;
    if let Err(error) = secrets.delete(&record.payload_secret_ref) {
        let rollback = atomic_write(&path, content).map_err(io_error_message);
        return Err(with_cleanup(error, rollback));
    }
    Ok(())
}

fn load_payload(
    record: &SnapshotRecord,
    secrets: &impl SnapshotSecrets,
) -> Result<SnapshotPayload> {
    let content = secrets.load(&record.payload_secret_ref)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Codex snapshot payload is missing",
        )
    })?;
    let payload: SnapshotPayload = serde_json::from_str(&content).map_err(invalid_data)?;
    if payload.version != PAYLOAD_VERSION {
        return Err(LocalPoolError::new(
            ErrorCode::UnsupportedSchema,
            "Codex snapshot payload uses an unsupported version",
        ));
    }
    validate_profile_content(&UserProfileSnapshot {
        config: payload.config.clone(),
        auth: payload.auth.clone(),
    })?;
    Ok(payload)
}

fn read_record(path: &Path) -> Result<SnapshotRecord> {
    let bytes = read_bounded(path, MAX_METADATA_BYTES)?;
    let content = std::str::from_utf8(&bytes).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Codex snapshot metadata is not UTF-8",
        )
    })?;
    serde_json::from_str(content).map_err(invalid_data)
}

fn validate_record(record: &SnapshotRecord, expected_id: &str) -> Result<()> {
    let id = parse_id(&record.id)?;
    if record.version != SNAPSHOT_VERSION
        || id != expected_id
        || record.name != normalize_name(&record.name)?
        || record.profile_dir.trim().is_empty()
        || !Path::new(&record.profile_dir).is_absolute()
        || record.profile_dir.chars().any(char::is_control)
        || record.created_at_ms == 0
        || record.payload_secret_ref != payload_secret_ref(&id)
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Codex snapshot metadata is invalid",
        ));
    }
    Ok(())
}

fn validate_profile_content(snapshot: &UserProfileSnapshot) -> Result<()> {
    if snapshot
        .config
        .as_ref()
        .is_some_and(|value| value.len() > MAX_PROFILE_FILE_BYTES)
        || snapshot
            .auth
            .as_ref()
            .is_some_and(|value| value.len() > MAX_PROFILE_FILE_BYTES)
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "Codex profile snapshot is too large",
        ));
    }
    Ok(())
}

fn normalize_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_NAME_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "Codex snapshot name is invalid",
        ));
    }
    Ok(value.to_string())
}

fn summary(record: &SnapshotRecord) -> ProfileSnapshotSummary {
    ProfileSnapshotSummary {
        id: record.id.clone(),
        name: record.name.clone(),
        profile_dir: record.profile_dir.clone(),
        created_at_ms: record.created_at_ms,
        config_available: record.config_available,
        auth_available: record.auth_available,
    }
}

fn snapshot_root(backup_root: &Path) -> PathBuf {
    backup_root.join(SNAPSHOT_DIR)
}

fn metadata_path(backup_root: &Path, id: &str) -> Result<PathBuf> {
    let id = parse_id(id)?;
    Ok(snapshot_root(backup_root).join(format!("{id}.json")))
}

fn parse_id(id: &str) -> Result<String> {
    let parsed = Uuid::parse_str(id.trim()).map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "Codex snapshot ID is invalid")
    })?;
    let normalized = parsed.to_string();
    if normalized != id {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "Codex snapshot ID is invalid",
        ));
    }
    Ok(normalized)
}

fn payload_secret_ref(id: &str) -> String {
    format!("profile:snapshot:{id}:payload")
}

fn read_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > max_bytes {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Codex snapshot metadata is invalid",
        ));
    }
    fs::read(path).map_err(io_error)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn invalid_data(error: impl std::fmt::Display) -> LocalPoolError {
    let _ = error;
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "Codex snapshot data is invalid",
    )
}

fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, format!("Codex snapshot I/O failed: {error}"))
}

fn io_error_message(error: String) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, error)
}

fn snapshot_changed() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::ProfileRestoreBlocked,
        "Codex snapshot changed while Relay was updating it",
    )
}

fn with_cleanup(error: LocalPoolError, cleanup: Result<()>) -> LocalPoolError {
    match cleanup {
        Ok(()) => error,
        Err(cleanup) => LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; snapshot cleanup failed: {}",
                error.message, cleanup.message
            ),
        ),
    }
}

trait SnapshotSecrets {
    fn save(&self, secret_ref: &str, value: &str) -> Result<()>;
    fn load(&self, secret_ref: &str) -> Result<Option<String>>;
    fn delete(&self, secret_ref: &str) -> Result<()>;
}

struct OsSnapshotSecrets;

impl SnapshotSecrets for OsSnapshotSecrets {
    fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
        secret_store::save(secret_ref, value)
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        secret_store::load(secret_ref)
    }

    fn delete(&self, secret_ref: &str) -> Result<()> {
        secret_store::delete(secret_ref)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, String>>);

    impl SnapshotSecrets for MemorySecrets {
        fn save(&self, secret_ref: &str, value: &str) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(secret_ref.to_string(), value.to_string());
            Ok(())
        }

        fn load(&self, secret_ref: &str) -> Result<Option<String>> {
            Ok(self.0.lock().unwrap().get(secret_ref).cloned())
        }

        fn delete(&self, secret_ref: &str) -> Result<()> {
            self.0.lock().unwrap().remove(secret_ref);
            Ok(())
        }
    }

    #[test]
    fn named_snapshots_encrypt_payload_restore_and_keep_a_safety_copy() {
        let root =
            std::env::temp_dir().join(format!("zenith-profile-snapshots-{}", Uuid::new_v4()));
        let profile = root.join("profile");
        let backups = root.join("backups");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join("config.toml"), "model = \"original-secret\"\n").unwrap();
        fs::write(profile.join("auth.json"), "{\"token\":\"auth-secret\"}").unwrap();
        let secrets = MemorySecrets::default();

        let first = create_with(&profile, &backups, "Original", &secrets).unwrap();
        let metadata = fs::read_to_string(metadata_path(&backups, &first.id).unwrap()).unwrap();
        assert!(!metadata.contains("original-secret"));
        assert!(!metadata.contains("auth-secret"));

        fs::write(profile.join("config.toml"), "model = \"changed\"\n").unwrap();
        fs::write(profile.join("auth.json"), "{\"token\":\"changed\"}").unwrap();
        create_with(&profile, &backups, "Changed", &secrets).unwrap();
        let safety =
            restore_with(&profile, &backups, &first.id, "Before restore", &secrets).unwrap();

        assert_eq!(
            fs::read_to_string(profile.join("config.toml")).unwrap(),
            "model = \"original-secret\"\n"
        );
        assert_eq!(
            fs::read_to_string(profile.join("auth.json")).unwrap(),
            "{\"token\":\"auth-secret\"}"
        );
        assert_eq!(list(&backups).unwrap().len(), 3);
        delete_with(&backups, &safety.id, &secrets).unwrap();
        assert_eq!(list(&backups).unwrap().len(), 2);

        fs::remove_dir_all(root).unwrap();
    }
}
