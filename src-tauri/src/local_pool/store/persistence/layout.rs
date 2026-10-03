use super::{PersistedState, LEGACY_STATE_FILES, MAX_LEGACY_JSON_BYTES, SQLITE_SIDECAR_SUFFIXES};
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use crate::local_pool::models::CURRENT_SCHEMA_VERSION;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use zenith_relay_core::path_with_suffix as companion_path;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyMetadata {
    schema_version: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyRemoteTargets {
    active: Option<crate::local_pool::models::RemoteTargetRecord>,
}

pub(super) fn load_legacy_state(root: &Path) -> Result<Option<PersistedState>> {
    let metadata_path = root.join("metadata.json");
    if !metadata_path.exists() {
        if LEGACY_STATE_FILES
            .iter()
            .skip(1)
            .any(|name| root.join(name).exists())
        {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "legacy local data exist but metadata.json is missing",
            ));
        }
        return Ok(None);
    }
    let metadata: LegacyMetadata = read_legacy_json(&metadata_path)?;
    if metadata.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(LocalPoolError::new(
            ErrorCode::UnsupportedSchema,
            format!(
                "legacy local data schema {} is unsupported; expected {CURRENT_SCHEMA_VERSION}",
                metadata.schema_version
            ),
        ));
    }
    let remote_target = if root.join("remote-target.json").exists() {
        read_legacy_json::<LegacyRemoteTargets>(&root.join("remote-target.json"))?.active
    } else {
        None
    };
    Ok(Some(PersistedState {
        gateway: read_legacy_json(&root.join("settings.json"))?,
        sources: read_legacy_json(&root.join("connections.json"))?,
        accounts: read_legacy_json(&root.join("accounts.json"))?,
        keys: read_legacy_json(&root.join("pool-keys.json"))?,
        automations: read_legacy_json(&root.join("automations.json"))?,
        remote_target,
        ownership_operation: None,
        refresh_revisions: None,
        source_refresh_revisions: None,
    }))
}

fn read_legacy_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let metadata = fs::symlink_metadata(path).map_err(legacy_io_error)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_LEGACY_JSON_BYTES
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("legacy local state file is unsafe: {}", path.display()),
        ));
    }
    let content = fs::read_to_string(path).map_err(legacy_io_error)?;
    serde_json::from_str(&content).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "legacy local state is invalid in {}: {error}",
                path.display()
            ),
        )
    })
}

pub(in crate::local_pool::store) fn cleanup_legacy_state_files(root: &Path) -> Result<()> {
    for name in LEGACY_STATE_FILES {
        let path = root.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                fs::remove_file(&path).map_err(legacy_io_error)?;
            }
            Ok(_) => {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("legacy local state path is unsafe: {}", path.display()),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(legacy_io_error(error)),
        }
    }
    Ok(())
}

pub(super) fn ensure_storage_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to create local pool store: {error}"),
        )
    })?;
    let metadata = fs::symlink_metadata(path).map_err(legacy_io_error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("local pool storage directory is unsafe: {}", path.display()),
        ));
    }
    Ok(())
}

/// The database used to live next to catalogs and vault files. Move it before
/// opening SQLite so its sidecars stay with the database and an interrupted
/// migration never selects one of two competing copies.
pub(in crate::local_pool::store) fn migrate_database_file(
    data_root: &Path,
    database_root: &Path,
) -> Result<()> {
    let target = database_root.join("relay.sqlite");
    let legacy_paths = [
        data_root.join("relay.sqlite"),
        data_root.join("usage.sqlite"),
    ];
    let sources = legacy_paths
        .iter()
        .filter_map(|path| regular_file_if_present(path, "legacy Relay database").transpose())
        .collect::<Result<Vec<_>>>()?;
    if sources.len() > 1 {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "multiple legacy Relay databases exist; recovery is required before migration",
        ));
    }
    let target_exists = regular_file_if_present(&target, "categorized Relay database")?.is_some();
    match (sources.first(), target_exists) {
        (Some(source), true) => Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "both legacy and categorized Relay databases exist: {} and {}",
                source.display(),
                target.display()
            ),
        )),
        (Some(source), false) => move_database_and_sidecars(source, &legacy_paths, &target),
        (None, true) => complete_database_sidecar_move(&legacy_paths, &target),
        (None, false) => {
            if legacy_sidecar_exists(&legacy_paths)? {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "legacy Relay database sidecars exist without a database file",
                ));
            }
            Ok(())
        }
    }
}

fn move_database_and_sidecars(
    source: &Path,
    legacy_paths: &[PathBuf],
    target: &Path,
) -> Result<()> {
    for other in legacy_paths.iter().filter(|path| path.as_path() != source) {
        for suffix in SQLITE_SIDECAR_SUFFIXES {
            let sidecar = companion_path(other, suffix);
            if regular_file_if_present(&sidecar, "legacy Relay database sidecar")?.is_some() {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!(
                        "legacy Relay database sidecar belongs to a different database: {}",
                        sidecar.display()
                    ),
                ));
            }
        }
    }
    for suffix in SQLITE_SIDECAR_SUFFIXES {
        let source_companion = companion_path(source, suffix);
        let target_companion = companion_path(target, suffix);
        regular_file_if_present(&source_companion, "legacy Relay database sidecar")?;
        if regular_file_if_present(&target_companion, "categorized Relay database sidecar")?
            .is_some()
        {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "categorized Relay database sidecar exists before its database: {}",
                    target_companion.display()
                ),
            ));
        }
    }
    fs::rename(source, target).map_err(legacy_io_error)?;
    for suffix in SQLITE_SIDECAR_SUFFIXES {
        let source_companion = companion_path(source, suffix);
        if regular_file_if_present(&source_companion, "legacy Relay database sidecar")?.is_some() {
            fs::rename(&source_companion, companion_path(target, suffix))
                .map_err(legacy_io_error)?;
        }
    }
    Ok(())
}

/// A crash after moving the database but before its sidecars leaves a
/// recoverable split layout. Finish only that exact move; a second copy is a
/// conflict, never a reason to discard either sidecar.
fn complete_database_sidecar_move(legacy_paths: &[PathBuf], target: &Path) -> Result<()> {
    for suffix in SQLITE_SIDECAR_SUFFIXES {
        let candidates = legacy_paths
            .iter()
            .map(|path| companion_path(path, suffix))
            .filter_map(|path| {
                regular_file_if_present(&path, "legacy Relay database sidecar").transpose()
            })
            .collect::<Result<Vec<_>>>()?;
        if candidates.len() > 1 {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "multiple legacy Relay database sidecars exist; recovery is required",
            ));
        }
        let Some(source) = candidates.first() else {
            continue;
        };
        let destination = companion_path(target, suffix);
        if regular_file_if_present(&destination, "categorized Relay database sidecar")?.is_some() {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "both legacy and categorized Relay database sidecars exist: {} and {}",
                    source.display(),
                    destination.display()
                ),
            ));
        }
        fs::rename(source, destination).map_err(legacy_io_error)?;
    }
    Ok(())
}

fn legacy_sidecar_exists(legacy_paths: &[PathBuf]) -> Result<bool> {
    for path in legacy_paths {
        for suffix in SQLITE_SIDECAR_SUFFIXES {
            if regular_file_if_present(
                &companion_path(path, suffix),
                "legacy Relay database sidecar",
            )?
            .is_some()
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn regular_file_if_present(path: &Path, description: &str) -> Result<Option<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            Ok(Some(path.to_path_buf()))
        }
        Ok(_) => Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("{description} is unsafe: {}", path.display()),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(legacy_io_error(error)),
    }
}

fn legacy_io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::Io,
        format!("local data migration failed: {error}"),
    )
}
