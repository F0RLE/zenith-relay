use super::settings_store::{load_json, save_json};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::{StoreMetadata, CURRENT_SCHEMA_VERSION},
};
use chrono::Utc;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn migrate(root: &Path) -> Result<StoreMetadata> {
    let metadata_path = root.join("metadata.json");
    let gateway_path = root.join("settings").join("gateway.json");
    let mut metadata = match load_json_or_quarantine::<StoreMetadata>(root, &metadata_path)? {
        Some(metadata) => metadata,
        None if gateway_path.exists() => StoreMetadata { schema_version: 0 },
        None => {
            let metadata = StoreMetadata::default();
            save_json(&metadata_path, &metadata)?;
            return Ok(metadata);
        }
    };

    if metadata.schema_version > CURRENT_SCHEMA_VERSION {
        return Err(LocalPoolError::new(
            ErrorCode::UnsupportedSchema,
            format!(
                "local pool schema {} is newer than supported schema {}",
                metadata.schema_version, CURRENT_SCHEMA_VERSION
            ),
        ));
    }

    let backup = (metadata.schema_version < CURRENT_SCHEMA_VERSION)
        .then(|| backup_settings(root, metadata.schema_version))
        .transpose()?;

    let result = (|| {
        while metadata.schema_version < CURRENT_SCHEMA_VERSION {
            match metadata.schema_version {
                0 => migrate_v0_to_v1(root, &gateway_path)?,
                version => {
                    return Err(LocalPoolError::new(
                        ErrorCode::UnsupportedSchema,
                        format!("no migration exists for local pool schema {version}"),
                    ));
                }
            }
            metadata.schema_version += 1;
            save_json(&metadata_path, &metadata)?;
        }
        Ok(metadata)
    })();

    if result.is_err() {
        if let Some(backup) = backup {
            restore_settings(root, &backup)?;
        }
    }
    result
}

fn migrate_v0_to_v1(root: &Path, path: &Path) -> Result<()> {
    let Some(mut gateway) = load_json_or_quarantine::<Value>(root, path)? else {
        return Ok(());
    };
    let object = gateway.as_object_mut().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "legacy gateway settings must be an object",
        )
    })?;
    object
        .entry("bindScope")
        .or_insert_with(|| Value::String("localhost".to_string()));
    object
        .entry("clientHost")
        .or_insert_with(|| Value::String("127.0.0.1".to_string()));
    save_json(path, &gateway)
}

fn load_json_or_quarantine<T: serde::de::DeserializeOwned>(
    root: &Path,
    path: &Path,
) -> Result<Option<T>> {
    match load_json(path) {
        Ok(value) => Ok(value),
        Err(error) if path.exists() => {
            let quarantined = super::quarantine::move_file(root, path)?;
            Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "invalid local pool data were moved to {}: {error}",
                    quarantined.display()
                ),
            ))
        }
        Err(error) => Err(error),
    }
}

fn backup_settings(root: &Path, schema_version: u32) -> Result<PathBuf> {
    let target = root.join("backups").join("migrations").join(format!(
        "v{}-{}",
        schema_version,
        Utc::now().format("%Y%m%d%H%M%S%3f")
    ));
    fs::create_dir_all(&target).map_err(io_error)?;
    copy_if_exists(&root.join("metadata.json"), &target.join("metadata.json"))?;
    copy_if_exists(
        &root.join("settings").join("gateway.json"),
        &target.join("gateway.json"),
    )?;
    Ok(target)
}

fn restore_settings(root: &Path, backup: &Path) -> Result<()> {
    copy_if_exists(&backup.join("metadata.json"), &root.join("metadata.json"))?;
    copy_if_exists(
        &backup.join("gateway.json"),
        &root.join("settings").join("gateway.json"),
    )
}

fn copy_if_exists(source: &Path, target: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    fs::copy(source, target).map(|_| ()).map_err(io_error)
}

fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;
    use std::{
        env,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        let path = env::temp_dir().join(format!(
            "zenith-relay-migration-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("settings")).unwrap();
        path
    }

    #[test]
    fn migrates_legacy_gateway_once_and_keeps_backup() {
        let root = temp_root();
        save_json(
            &root.join("settings").join("gateway.json"),
            &Value::Object(Map::from_iter([
                ("enabled".to_string(), Value::Bool(false)),
                ("port".to_string(), Value::from(14998)),
            ])),
        )
        .unwrap();

        assert_eq!(
            migrate(&root).unwrap().schema_version,
            CURRENT_SCHEMA_VERSION
        );
        assert_eq!(
            migrate(&root).unwrap().schema_version,
            CURRENT_SCHEMA_VERSION
        );
        let gateway: Value = load_json(&root.join("settings").join("gateway.json"))
            .unwrap()
            .unwrap();
        assert_eq!(gateway["clientHost"], "127.0.0.1");
        assert!(root
            .join("backups")
            .join("migrations")
            .read_dir()
            .unwrap()
            .next()
            .is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_newer_schema() {
        let root = temp_root();
        save_json(
            &root.join("metadata.json"),
            &StoreMetadata {
                schema_version: CURRENT_SCHEMA_VERSION + 1,
            },
        )
        .unwrap();
        let error = migrate(&root).unwrap_err();
        assert!(matches!(error.code, ErrorCode::UnsupportedSchema));
        fs::remove_dir_all(root).unwrap();
    }
}
