use super::codex::{self, UserProfileSnapshot};
use crate::{
    files::atomic_write,
    local_pool::error::{ErrorCode, LocalPoolError, Result},
};
use serde::Serialize;
use std::{fs, path::Path};
use uuid::Uuid;
use zenith_relay_core::unix_time_ms as now_ms;

pub(super) const SNAPSHOT_VERSION: u32 = 1;
pub(super) const PAYLOAD_VERSION: u32 = 1;
mod io;

use io::SnapshotSecrets;

mod record;
use record::{
    load_payload, normalize_name, read_record, summary, validate_profile_content, validate_record,
    SnapshotPayload, SnapshotRecord,
};

pub(super) const MAX_NAME_CHARS: usize = 80;
pub(super) const MAX_METADATA_BYTES: u64 = 16 * 1024;
pub(super) const MAX_PROFILE_FILE_BYTES: usize = 1024 * 1024;

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

pub fn list(backup_root: &Path) -> Result<ProfileSnapshotList> {
    list_with(backup_root, &io::OsSnapshotSecrets)
}

fn list_with(backup_root: &Path, secrets: &impl SnapshotSecrets) -> Result<ProfileSnapshotList> {
    let root = io::snapshot_root(backup_root);
    if !root.exists() {
        return Ok(ProfileSnapshotList {
            snapshots: Vec::new(),
            invalid_count: 0,
        });
    }
    let mut snapshots = Vec::new();
    let mut invalid_count = 0;
    for directory_entry in fs::read_dir(&root).map_err(io::io_error)? {
        let directory_entry = directory_entry.map_err(io::io_error)?;
        if !directory_entry.file_type().map_err(io::io_error)?.is_file()
            || directory_entry
                .path()
                .extension()
                .and_then(|file_extension| file_extension.to_str())
                != Some("json")
        {
            continue;
        }
        let path = directory_entry.path();
        let stem = path
            .file_stem()
            .and_then(|file_stem| file_stem.to_str())
            .unwrap_or_default();
        let snapshot_record = read_record(&path).and_then(|snapshot_record| {
            validate_record(&snapshot_record, stem)?;
            let snapshot_payload = load_payload(&snapshot_record, secrets)?;
            if snapshot_record.config_available != snapshot_payload.config.is_some()
                || snapshot_record.auth_available != snapshot_payload.auth.is_some()
            {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "ChatGPT snapshot metadata does not match its encrypted payload",
                ));
            }
            Ok(snapshot_record)
        });
        match snapshot_record {
            Ok(snapshot_record) => snapshots.push(summary(&snapshot_record)),
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

pub fn create(
    codex_home: &Path,
    backup_root: &Path,
    snapshot_name: &str,
) -> Result<ProfileSnapshotSummary> {
    create_with(
        codex_home,
        backup_root,
        snapshot_name,
        false,
        &io::OsSnapshotSecrets,
    )
}

pub fn restore_full(codex_home: &Path, backup_root: &Path, snapshot_id: &str) -> Result<()> {
    restore_full_with(codex_home, backup_root, snapshot_id, &io::OsSnapshotSecrets)
}

pub fn delete(backup_root: &Path, snapshot_id: &str) -> Result<()> {
    delete_with(backup_root, snapshot_id, &io::OsSnapshotSecrets)
}

fn create_with(
    codex_home: &Path,
    backup_root: &Path,
    requested_name: &str,
    is_original: bool,
    secrets: &impl SnapshotSecrets,
) -> Result<ProfileSnapshotSummary> {
    let snapshot_name = normalize_name(requested_name)?;
    fs::create_dir_all(codex_home).map_err(io::io_error)?;
    let profile_dir = fs::canonicalize(codex_home).map_err(io::io_error)?;
    let snapshot = codex::snapshot_user_profile(&profile_dir, backup_root)?;
    validate_profile_content(&snapshot)?;
    let snapshot_id = Uuid::new_v4().to_string();
    let payload_secret_ref = io::payload_secret_ref(&snapshot_id);
    let config_available = snapshot.config.is_some();
    let auth_available = snapshot.auth.is_some();
    let snapshot_payload_json = serde_json::to_string(&SnapshotPayload {
        version: PAYLOAD_VERSION,
        config: snapshot.config,
        auth: snapshot.auth,
    })
    .map_err(io::invalid_data)?;
    secrets.save(&payload_secret_ref, &snapshot_payload_json)?;

    let snapshot_record = SnapshotRecord {
        version: SNAPSHOT_VERSION,
        id: snapshot_id.clone(),
        name: snapshot_name,
        profile_dir: codex::portable_path_string(&profile_dir),
        created_at_ms: now_ms(),
        config_available,
        auth_available,
        is_original,
        payload_secret_ref: payload_secret_ref.clone(),
    };
    let metadata = serde_json::to_string_pretty(&snapshot_record).map_err(io::invalid_data)?;
    let path = io::metadata_path(backup_root, &snapshot_id)?;
    if let Err(error) =
        atomic_write(&path, &format!("{metadata}\n")).map_err(super::io_error_message)
    {
        return Err(io::with_cleanup(error, secrets.delete(&payload_secret_ref)));
    }
    Ok(summary(&snapshot_record))
}

fn restore_full_with(
    codex_home: &Path,
    backup_root: &Path,
    snapshot_id: &str,
    secrets: &impl SnapshotSecrets,
) -> Result<()> {
    let path = io::metadata_path(backup_root, snapshot_id)?;
    let snapshot_record = read_record(&path)?;
    validate_record(&snapshot_record, snapshot_id)?;
    // A profile directory may have been removed while the snapshot was kept.
    // Recreate it before canonicalizing so a valid snapshot can repair the
    // profile instead of failing with an I/O error on a missing path.
    fs::create_dir_all(codex_home).map_err(io::io_error)?;
    let profile_dir = fs::canonicalize(codex_home).map_err(io::io_error)?;
    if codex::portable_path_value(&snapshot_record.profile_dir)
        != codex::portable_path_string(&profile_dir)
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "ChatGPT snapshot belongs to another profile",
        ));
    }
    let snapshot_payload = load_payload(&snapshot_record, secrets)?;
    let snapshot = UserProfileSnapshot {
        config: snapshot_payload.config,
        auth: snapshot_payload.auth,
    };
    codex::restore_full_user_profile_snapshot(&profile_dir, backup_root, &snapshot)
}

fn delete_with(
    backup_root: &Path,
    snapshot_id: &str,
    secrets: &impl SnapshotSecrets,
) -> Result<()> {
    let path = io::metadata_path(backup_root, snapshot_id)?;
    let bytes = io::read_bounded(&path, MAX_METADATA_BYTES)?;
    let content = std::str::from_utf8(&bytes).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT snapshot metadata is not UTF-8",
        )
    })?;
    let snapshot_record: SnapshotRecord =
        serde_json::from_str(content).map_err(io::invalid_data)?;
    validate_record(&snapshot_record, snapshot_id)?;
    if fs::read(&path).map_err(io::io_error)? != bytes {
        return Err(io::snapshot_changed());
    }
    fs::remove_file(&path).map_err(io::io_error)?;
    if let Err(error) = secrets.delete(&snapshot_record.payload_secret_ref) {
        let rollback = atomic_write(&path, content).map_err(super::io_error_message);
        return Err(io::with_cleanup(error, rollback));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, String>>);

    impl SnapshotSecrets for MemorySecrets {
        fn save(&self, secret_ref: &str, secret_value: &str) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(secret_ref.to_string(), secret_value.to_string());
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
        let metadata = fs::read_to_string(io::metadata_path(&backups, &first.id).unwrap()).unwrap();
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
    fn full_restore_recreates_a_deleted_profile_directory() {
        let root = std::env::temp_dir().join(format!(
            "zenith-profile-snapshot-deleted-{}",
            Uuid::new_v4()
        ));
        let profile = root.join("profile");
        let backups = root.join("backups");
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join("config.toml"), "model = \"original\"\n").unwrap();
        fs::write(profile.join("auth.json"), "{\"token\":\"original\"}").unwrap();
        let secrets = MemorySecrets::default();

        let snapshot = create_with(&profile, &backups, "Original", false, &secrets).unwrap();
        fs::remove_dir_all(&profile).unwrap();

        restore_full_with(&profile, &backups, &snapshot.id, &secrets).unwrap();

        assert_eq!(
            fs::read_to_string(profile.join("config.toml")).unwrap(),
            "model = \"original\"\n"
        );
        assert_eq!(
            fs::read_to_string(profile.join("auth.json")).unwrap(),
            "{\"token\":\"original\"}"
        );

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

        secrets
            .delete(&io::payload_secret_ref(&snapshot.id))
            .unwrap();
        let list = list_with(&backups, &secrets).unwrap();

        assert_eq!(list.invalid_count, 1);
        assert_eq!(list.snapshots.len(), 1);
        assert_eq!(list.snapshots[0].id, valid.id);
        fs::remove_dir_all(root).unwrap();
    }
}
