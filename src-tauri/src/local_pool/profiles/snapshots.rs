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
};
use uuid::Uuid;
use zenith_relay_core::unix_time_ms as now_ms;

const SNAPSHOT_VERSION: u32 = 1;
const PAYLOAD_VERSION: u32 = 1;
const SNAPSHOT_DIR: &str = "snapshots";
const ORIGINAL_MARKER_FILE: &str = "original.snapshot.initialized";
const ORIGINAL_SNAPSHOT_NAME: &str = "Initial profile";
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
    pub is_original: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSnapshotList {
    pub snapshots: Vec<ProfileSnapshotSummary>,
    pub invalid_count: usize,
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
    #[serde(default)]
    is_original: bool,
    payload_secret_ref: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotPayload {
    version: u32,
    config: Option<String>,
    auth: Option<String>,
}

pub fn list(backup_root: &Path) -> Result<ProfileSnapshotList> {
    list_with(backup_root, &OsSnapshotSecrets)
}

fn list_with(backup_root: &Path, secrets: &impl SnapshotSecrets) -> Result<ProfileSnapshotList> {
    let root = snapshot_root(backup_root);
    if !root.exists() {
        return Ok(ProfileSnapshotList {
            snapshots: Vec::new(),
            invalid_count: 0,
        });
    }
    let mut snapshots = Vec::new();
    let mut invalid_count = 0;
    for entry in fs::read_dir(&root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if !entry.file_type().map_err(io_error)?.is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let path = entry.path();
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let record = read_record(&path).and_then(|record| {
            validate_record(&record, stem)?;
            let payload = load_payload(&record, secrets)?;
            if record.config_available != payload.config.is_some()
                || record.auth_available != payload.auth.is_some()
            {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "ChatGPT snapshot metadata does not match its encrypted payload",
                ));
            }
            Ok(record)
        });
        match record {
            Ok(record) => snapshots.push(summary(&record)),
            Err(_) => invalid_count += 1,
        }
    }
    snapshots.sort_by(|left, right| {
        right
            .is_original
            .cmp(&left.is_original)
            .then_with(|| right.created_at_ms.cmp(&left.created_at_ms))
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(ProfileSnapshotList {
        snapshots,
        invalid_count,
    })
}

pub fn create(codex_home: &Path, backup_root: &Path, name: &str) -> Result<ProfileSnapshotSummary> {
    create_with(codex_home, backup_root, name, false, &OsSnapshotSecrets)
}

/// Create the one-time first-launch restore point. The marker deliberately
/// survives deletion of the snapshot so a later startup cannot silently turn
/// a user-created profile into a new "original" state.
pub fn ensure_original(codex_home: &Path, backup_root: &Path) -> Result<()> {
    ensure_original_with(codex_home, backup_root, &OsSnapshotSecrets)
}

pub fn restore_full(codex_home: &Path, backup_root: &Path, id: &str) -> Result<()> {
    restore_full_with(codex_home, backup_root, id, &OsSnapshotSecrets)
}

pub fn delete(backup_root: &Path, id: &str) -> Result<()> {
    delete_with(backup_root, id, &OsSnapshotSecrets)
}

fn create_with(
    codex_home: &Path,
    backup_root: &Path,
    name: &str,
    is_original: bool,
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
        profile_dir: codex::portable_path_string(&profile_dir),
        created_at_ms: now_ms(),
        config_available,
        auth_available,
        is_original,
        payload_secret_ref: payload_secret_ref.clone(),
    };
    let metadata = serde_json::to_string_pretty(&record).map_err(invalid_data)?;
    let path = metadata_path(backup_root, &id)?;
    if let Err(error) = atomic_write(&path, &format!("{metadata}\n")).map_err(io_error_message) {
        return Err(with_cleanup(error, secrets.delete(&payload_secret_ref)));
    }
    Ok(summary(&record))
}

fn ensure_original_with(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SnapshotSecrets,
) -> Result<()> {
    let root = snapshot_root(backup_root);
    fs::create_dir_all(&root).map_err(io_error)?;
    let marker = root.join(ORIGINAL_MARKER_FILE);
    if marker.exists() {
        return Ok(());
    }

    if !list_with(backup_root, secrets)?
        .snapshots
        .iter()
        .any(|snapshot| snapshot.is_original)
    {
        create_with(
            codex_home,
            backup_root,
            ORIGINAL_SNAPSHOT_NAME,
            true,
            secrets,
        )?;
    }
    atomic_write(&marker, "v1\n").map_err(io_error_message)?;
    Ok(())
}

fn restore_full_with(
    codex_home: &Path,
    backup_root: &Path,
    id: &str,
    secrets: &impl SnapshotSecrets,
) -> Result<()> {
    let path = metadata_path(backup_root, id)?;
    let record = read_record(&path)?;
    validate_record(&record, id)?;
    let profile_dir = fs::canonicalize(codex_home).map_err(io_error)?;
    if codex::portable_path_value(&record.profile_dir) != codex::portable_path_string(&profile_dir)
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "ChatGPT snapshot belongs to another profile",
        ));
    }
    let payload = load_payload(&record, secrets)?;
    let snapshot = UserProfileSnapshot {
        config: payload.config,
        auth: payload.auth,
    };
    codex::restore_full_user_profile_snapshot(&profile_dir, backup_root, &snapshot)
}

fn delete_with(backup_root: &Path, id: &str, secrets: &impl SnapshotSecrets) -> Result<()> {
    let path = metadata_path(backup_root, id)?;
    let bytes = read_bounded(&path, MAX_METADATA_BYTES)?;
    let content = std::str::from_utf8(&bytes).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT snapshot metadata is not UTF-8",
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
            "ChatGPT snapshot payload is missing",
        )
    })?;
    let payload: SnapshotPayload = serde_json::from_str(&content).map_err(invalid_data)?;
    if payload.version != PAYLOAD_VERSION {
        return Err(LocalPoolError::new(
            ErrorCode::UnsupportedSchema,
            "ChatGPT snapshot payload uses an unsupported version",
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
            "ChatGPT snapshot metadata is not UTF-8",
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
            "ChatGPT snapshot metadata is invalid",
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
            "ChatGPT profile snapshot is too large",
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
            "ChatGPT snapshot name is invalid",
        ));
    }
    Ok(value.to_string())
}

fn summary(record: &SnapshotRecord) -> ProfileSnapshotSummary {
    ProfileSnapshotSummary {
        id: record.id.clone(),
        name: record.name.clone(),
        profile_dir: codex::portable_path_value(&record.profile_dir),
        created_at_ms: record.created_at_ms,
        config_available: record.config_available,
        auth_available: record.auth_available,
        is_original: record.is_original,
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
        LocalPoolError::new(ErrorCode::InvalidState, "ChatGPT snapshot ID is invalid")
    })?;
    let normalized = parsed.to_string();
    if normalized != id {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT snapshot ID is invalid",
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
            "ChatGPT snapshot metadata is invalid",
        ));
    }
    fs::read(path).map_err(io_error)
}

fn invalid_data(error: impl std::fmt::Display) -> LocalPoolError {
    let _ = error;
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "ChatGPT snapshot data is invalid",
    )
}

fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::Io,
        format!("ChatGPT snapshot I/O failed: {error}"),
    )
}

fn io_error_message(error: String) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, error)
}

fn snapshot_changed() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::ProfileRestoreBlocked,
        "ChatGPT snapshot changed while Relay was updating it",
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
    fn full_restore_replaces_the_profile_without_creating_an_extra_snapshot() {
        let root =
            std::env::temp_dir().join(format!("zenith-profile-snapshots-{}", Uuid::new_v4()));
        let profile = root.join("profile");
        let backups = root.join("backups");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join("config.toml"), "model = \"original-secret\"\n").unwrap();
        fs::write(profile.join("auth.json"), "{\"token\":\"auth-secret\"}").unwrap();
        let secrets = MemorySecrets::default();

        let first = create_with(&profile, &backups, "Original", false, &secrets).unwrap();
        let metadata = fs::read_to_string(metadata_path(&backups, &first.id).unwrap()).unwrap();
        assert!(!metadata.contains("original-secret"));
        assert!(!metadata.contains("auth-secret"));

        fs::write(profile.join("config.toml"), "model = \"changed\"\n").unwrap();
        fs::write(profile.join("auth.json"), "{\"token\":\"changed\"}").unwrap();
        restore_full_with(&profile, &backups, &first.id, &secrets).unwrap();

        assert_eq!(
            fs::read_to_string(profile.join("config.toml")).unwrap(),
            "model = \"original-secret\"\n"
        );
        assert_eq!(
            fs::read_to_string(profile.join("auth.json")).unwrap(),
            "{\"token\":\"auth-secret\"}"
        );
        assert_eq!(list_with(&backups, &secrets).unwrap().snapshots.len(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn full_restore_does_not_save_the_current_profile() {
        let root =
            std::env::temp_dir().join(format!("zenith-profile-snapshots-{}", Uuid::new_v4()));
        let profile = root.join("profile");
        let backups = root.join("backups");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join("config.toml"), "model = \"original\"\n").unwrap();
        let secrets = MemorySecrets::default();

        let original = create_with(&profile, &backups, "Original", false, &secrets).unwrap();
        fs::write(profile.join("config.toml"), "model = \"changed\"\n").unwrap();
        restore_full_with(&profile, &backups, &original.id, &secrets).unwrap();

        assert_eq!(
            fs::read_to_string(profile.join("config.toml")).unwrap(),
            "model = \"original\"\n"
        );
        assert_eq!(list_with(&backups, &secrets).unwrap().snapshots.len(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_list_keeps_valid_entries_when_one_payload_is_missing() {
        let root = std::env::temp_dir().join(format!(
            "zenith-profile-snapshot-missing-{}",
            Uuid::new_v4()
        ));
        let profile = root.join("profile");
        let backups = root.join("backups");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join("config.toml"), "model = \"test\"\n").unwrap();
        let secrets = MemorySecrets::default();
        let snapshot = create_with(&profile, &backups, "Missing", false, &secrets).unwrap();
        let valid = create_with(&profile, &backups, "Valid", false, &secrets).unwrap();

        secrets.delete(&payload_secret_ref(&snapshot.id)).unwrap();
        let list = list_with(&backups, &secrets).unwrap();

        assert_eq!(list.invalid_count, 1);
        assert_eq!(list.snapshots.len(), 1);
        assert_eq!(list.snapshots[0].id, valid.id);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn original_snapshot_is_created_once_and_sorted_first() {
        let root = std::env::temp_dir().join(format!("zenith-profile-original-{}", Uuid::new_v4()));
        let profile = root.join("profile");
        let backups = root.join("backups");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join("config.toml"), "model = \"initial\"\n").unwrap();
        let secrets = MemorySecrets::default();

        ensure_original_with(&profile, &backups, &secrets).unwrap();
        let original = list_with(&backups, &secrets).unwrap().snapshots;
        assert_eq!(original.len(), 1);
        assert!(original[0].is_original);
        create_with(&profile, &backups, "Later", false, &secrets).unwrap();
        ensure_original_with(&profile, &backups, &secrets).unwrap();
        let snapshots = list_with(&backups, &secrets).unwrap().snapshots;
        assert_eq!(snapshots.len(), 2);
        assert!(snapshots[0].is_original);
        assert_eq!(
            snapshots
                .iter()
                .filter(|snapshot| snapshot.is_original)
                .count(),
            1
        );

        fs::remove_dir_all(root).unwrap();
    }
}
