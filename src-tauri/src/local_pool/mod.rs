mod accounts;
pub(crate) mod background;
pub mod commands;
mod error;
mod host;
mod models;
mod profiles;
mod remote;
mod state;
mod store;

use std::{ffi::OsStr, fs, io::ErrorKind, path::Path};

pub use state::DesktopState;

const LEGACY_LAYOUT_MOVES: [(&str, &str); 20] = [
    ("metadata.json", "data/metadata.json"),
    ("settings", "data/settings"),
    ("records", "data/records"),
    ("telemetry", "data/telemetry"),
    ("vault", "data/vault"),
    ("backups/migrations", "recovery/migrations"),
    ("backups/profiles", "recovery/profiles"),
    ("backups/history-repair", "recovery/history-repair"),
    ("backups/ready-api", "recovery/client-config"),
    ("transient/imports", "cache/imports"),
    ("transient/oauth_pending", "cache/oauth_pending"),
    ("transient/repair_previews", "cache/repair_previews"),
    ("transient/locks", "cache/locks"),
    ("output/exports", "recovery/exports"),
    ("output/deployments", "cache/deployments"),
    ("imports", "cache/imports"),
    ("oauth_pending", "cache/oauth_pending"),
    ("repair_previews", "cache/repair_previews"),
    ("locks", "cache/locks"),
    ("quarantine", "recovery/quarantine"),
];

pub fn initialize(app: &tauri::AppHandle) -> error::Result<DesktopState> {
    let root = crate::platform::relay_dir(app)
        .map_err(|message| error::LocalPoolError::new(error::ErrorCode::Io, message))?;
    migrate_storage_layout(app, &root)?;
    profiles::repair::migrate_history_repair_backups(
        &root.join("recovery").join("profiles"),
        &root.join("recovery").join("history-repair"),
    )
    .map_err(|message| error::LocalPoolError::new(error::ErrorCode::Io, message))?;
    store::secret_store::initialize(&root.join("data"))?;
    let state = DesktopState::open(root)?;
    state.set_app_handle(app.clone());
    Ok(state)
}

fn migrate_storage_layout(app: &tauri::AppHandle, root: &Path) -> error::Result<()> {
    create_storage_directory(root)?;
    let roaming = crate::platform::legacy_roaming_local_pool_dir(app).map_err(layout_error)?;
    let local = crate::platform::legacy_local_pool_dir(app).map_err(layout_error)?;
    for legacy in [&roaming, &local] {
        if legacy != root {
            migrate_legacy_store(legacy, root)?;
        }
    }

    if !local.exists() {
        let legacy_cache = crate::platform::legacy_app_local_dir(app).map_err(layout_error)?;
        let cache = root.join("cache").join("com.zenith.codex");
        if legacy_cache != root && legacy_cache.exists() {
            migrate_layout_directory(&legacy_cache, &cache)?;
        }
    }
    Ok(())
}

fn migrate_legacy_store(legacy_root: &Path, root: &Path) -> error::Result<()> {
    if !legacy_root.exists() {
        return Ok(());
    }
    for (legacy, target) in LEGACY_LAYOUT_MOVES {
        migrate_layout_path(&legacy_root.join(legacy), &root.join(target))?;
    }
    remove_empty_directories(legacy_root)?;
    if legacy_root.exists() {
        migrate_layout_directory(legacy_root, &root.join("recovery").join("legacy"))?;
    }
    Ok(())
}

fn create_storage_directory(path: &Path) -> error::Result<()> {
    fs::create_dir_all(path).map_err(|error| {
        layout_error(format!(
            "failed to create relay data directory {}: {error}",
            path.display()
        ))
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|error| layout_error(error.to_string()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(layout_error(format!(
            "relay data path must be a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn migrate_layout_path(legacy: &Path, target: &Path) -> error::Result<()> {
    let metadata = match fs::symlink_metadata(legacy) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(layout_error(error.to_string())),
    };
    if metadata.file_type().is_symlink() {
        return Err(layout_error(format!(
            "legacy data entry must not be a symbolic link: {}",
            legacy.display()
        )));
    }
    if metadata.is_dir() {
        return migrate_layout_directory(legacy, target);
    }
    let parent = target
        .parent()
        .ok_or_else(|| layout_error(format!("data target has no parent: {}", target.display())))?;
    fs::create_dir_all(parent).map_err(|error| layout_error(error.to_string()))?;
    let destination = if target.exists() {
        let name = target
            .file_name()
            .ok_or_else(|| layout_error("data target has no file name"))?;
        next_legacy_destination(parent, name)
    } else {
        target.to_path_buf()
    };
    fs::rename(legacy, &destination).map_err(|error| {
        layout_error(format!(
            "failed to move legacy data from {} to {}: {error}",
            legacy.display(),
            destination.display()
        ))
    })
}

fn remove_empty_directories(path: &Path) -> error::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(layout_error(error.to_string())),
    };
    if metadata.file_type().is_symlink() {
        return Err(layout_error(format!(
            "legacy data entry must not be a symbolic link: {}",
            path.display()
        )));
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(|error| layout_error(error.to_string()))? {
        remove_empty_directories(
            &entry
                .map_err(|error| layout_error(error.to_string()))?
                .path(),
        )?;
    }
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::DirectoryNotEmpty => Ok(()),
        Err(error) => Err(layout_error(error.to_string())),
    }
}

fn migrate_layout_directory(legacy: &Path, target: &Path) -> error::Result<()> {
    match crate::platform::migrate_directory(legacy, target).map_err(layout_error)? {
        crate::platform::StorageMigration::Current | crate::platform::StorageMigration::Moved => {
            Ok(())
        }
        crate::platform::StorageMigration::Conflict => merge_layout_directories(legacy, target),
    }
}

fn merge_layout_directories(legacy: &Path, target: &Path) -> error::Result<()> {
    let entries = fs::read_dir(legacy).map_err(|error| {
        layout_error(format!(
            "failed to read legacy data directory {}: {error}",
            legacy.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| layout_error(error.to_string()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| layout_error(error.to_string()))?;
        if file_type.is_symlink() {
            return Err(layout_error(format!(
                "legacy data entry must not be a symbolic link: {}",
                entry.path().display()
            )));
        }
        let name = entry.file_name();
        let destination = if target.join(&name).exists() {
            next_legacy_destination(target, &name)
        } else {
            target.join(&name)
        };
        fs::rename(entry.path(), &destination).map_err(|error| {
            layout_error(format!(
                "failed to move legacy data from {} to {}: {error}",
                entry.path().display(),
                destination.display()
            ))
        })?;
    }
    fs::remove_dir(legacy).map_err(|error| {
        layout_error(format!(
            "failed to remove empty legacy data directory {}: {error}",
            legacy.display()
        ))
    })
}

fn next_legacy_destination(directory: &Path, name: &OsStr) -> std::path::PathBuf {
    let path = Path::new(name);
    let stem = path.file_stem().and_then(OsStr::to_str);
    let extension = path.extension().and_then(OsStr::to_str);
    for index in 1_u32.. {
        let candidate = match (stem, extension) {
            (Some(stem), Some(extension)) => {
                directory.join(format!("{stem}.legacy-{index}.{extension}"))
            }
            _ => {
                let mut candidate = name.to_os_string();
                candidate.push(format!(".legacy-{index}"));
                directory.join(candidate)
            }
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("u32 legacy suffix space exhausted")
}

fn layout_error(message: impl Into<String>) -> error::LocalPoolError {
    error::LocalPoolError::new(error::ErrorCode::Io, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_layout_moves_legacy_store_into_named_zones() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-layout-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let legacy_root = root.join("old/local-pool");
        let target_root = root.join("Zenith Relay");
        for (index, (legacy, _)) in LEGACY_LAYOUT_MOVES.iter().enumerate() {
            let path = legacy_root.join(legacy);
            if *legacy == "metadata.json" {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, legacy).unwrap();
            } else {
                std::fs::create_dir_all(&path).unwrap();
                std::fs::write(path.join(format!("entry-{index}")), legacy).unwrap();
            }
        }

        migrate_legacy_store(&legacy_root, &target_root).unwrap();

        for (index, (legacy, target)) in LEGACY_LAYOUT_MOVES.iter().enumerate() {
            let target = target_root.join(target);
            let migrated = if *legacy == "metadata.json" {
                target
            } else {
                target.join(format!("entry-{index}"))
            };
            assert_eq!(std::fs::read_to_string(migrated).unwrap(), *legacy);
        }
        assert!(!legacy_root.exists());
        migrate_legacy_store(&legacy_root, &target_root).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn storage_layout_preserves_both_files_when_target_already_exists() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-layout-conflict-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let legacy_root = root.join("old/local-pool");
        let target_root = root.join("Zenith Relay");
        std::fs::create_dir_all(legacy_root.join("records")).unwrap();
        std::fs::create_dir_all(target_root.join("data/records")).unwrap();
        std::fs::write(legacy_root.join("records/accounts.json"), "legacy").unwrap();
        std::fs::write(target_root.join("data/records/accounts.json"), "current").unwrap();

        migrate_legacy_store(&legacy_root, &target_root).unwrap();

        assert_eq!(
            std::fs::read_to_string(target_root.join("data/records/accounts.json")).unwrap(),
            "current"
        );
        assert_eq!(
            std::fs::read_to_string(target_root.join("data/records/accounts.legacy-1.json"))
                .unwrap(),
            "legacy"
        );
        assert!(!legacy_root.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
