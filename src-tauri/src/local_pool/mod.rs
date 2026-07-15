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

use std::{ffi::OsStr, fs, path::Path};

pub use state::DesktopState;

const STORAGE_LAYOUT_MOVES: [(&str, &str); 6] = [
    ("imports", "transient/imports"),
    ("oauth_pending", "transient/oauth_pending"),
    ("repair_previews", "transient/repair_previews"),
    ("locks", "transient/locks"),
    ("exports", "output/exports"),
    ("deployments", "output/deployments"),
];

pub fn initialize(app: &tauri::AppHandle) -> error::Result<DesktopState> {
    let root = crate::platform::local_pool_dir(app)
        .map_err(|message| error::LocalPoolError::new(error::ErrorCode::Io, message))?;
    migrate_storage_layout(&root)?;
    profiles::repair::migrate_history_repair_backups(
        &root.join("backups").join("profiles"),
        &root.join("backups").join("history-repair"),
    )
    .map_err(|message| error::LocalPoolError::new(error::ErrorCode::Io, message))?;
    store::secret_store::initialize(&root)?;
    let state = DesktopState::open(root)?;
    state.set_app_handle(app.clone());
    Ok(state)
}

fn migrate_storage_layout(root: &Path) -> error::Result<()> {
    for (legacy, target) in STORAGE_LAYOUT_MOVES {
        migrate_layout_directory(&root.join(legacy), &root.join(target))?;
    }
    Ok(())
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
    fn storage_layout_moves_transient_and_output_directories() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-layout-{}",
            uuid::Uuid::new_v4().simple()
        ));
        for (index, (legacy, _)) in STORAGE_LAYOUT_MOVES.iter().enumerate() {
            std::fs::create_dir_all(root.join(legacy)).unwrap();
            std::fs::write(root.join(legacy).join(format!("entry-{index}")), legacy).unwrap();
        }

        migrate_storage_layout(&root).unwrap();

        for (index, (legacy, target)) in STORAGE_LAYOUT_MOVES.iter().enumerate() {
            assert_eq!(
                std::fs::read_to_string(root.join(target).join(format!("entry-{index}"))).unwrap(),
                *legacy
            );
            assert!(!root.join(legacy).exists());
        }
        migrate_storage_layout(&root).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn storage_layout_preserves_both_files_when_target_already_exists() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-layout-conflict-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("exports")).unwrap();
        std::fs::create_dir_all(root.join("output/exports")).unwrap();
        std::fs::write(root.join("exports/report.json"), "legacy").unwrap();
        std::fs::write(root.join("output/exports/report.json"), "current").unwrap();

        migrate_storage_layout(&root).unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join("output/exports/report.json")).unwrap(),
            "current"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("output/exports/report.legacy-1.json")).unwrap(),
            "legacy"
        );
        assert!(!root.join("exports").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
