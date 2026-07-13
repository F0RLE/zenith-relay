use super::settings_store::{load_json, save_json};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::{AutomationRecords, StoreMetadata, CURRENT_SCHEMA_VERSION},
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
            migrate_v1_to_v2(root)?;
            migrate_v3_to_v4(root)?;
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
                1 => migrate_v1_to_v2(root)?,
                2 => migrate_v2_to_v3(root, &gateway_path)?,
                3 => migrate_v3_to_v4(root)?,
                4 => migrate_v4_to_v5(root, &gateway_path)?,
                5 => migrate_v5_to_v6(root, &gateway_path)?,
                6 => migrate_v6_to_v7(root, &gateway_path)?,
                7 => migrate_v7_to_v8(root, &gateway_path)?,
                8 => migrate_v8_to_v9(root)?,
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

fn migrate_v8_to_v9(root: &Path) -> Result<()> {
    let path = root.join("records").join("accounts.json");
    let mut records =
        load_json_or_quarantine::<Value>(root, &path)?.unwrap_or_else(|| Value::Array(Vec::new()));
    let records = records.as_array_mut().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "accounts.json must contain an array",
        )
    })?;
    for record in records.iter_mut() {
        let record = record.as_object_mut().ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "accounts.json must contain objects",
            )
        })?;
        record.insert("cooldowns".to_string(), Value::Object(Default::default()));
        record.insert("consecutiveFailures".to_string(), Value::from(0));
    }
    save_json(&path, records)
}

fn migrate_v7_to_v8(root: &Path, gateway_path: &Path) -> Result<()> {
    let mut gateway = load_json_or_quarantine::<Value>(root, gateway_path)?.ok_or_else(|| {
        LocalPoolError::new(ErrorCode::InvalidState, "gateway settings are missing")
    })?;
    let gateway = gateway.as_object_mut().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "gateway settings must be an object",
        )
    })?;
    gateway
        .entry("useFreeAccounts")
        .or_insert_with(|| Value::Bool(false));
    save_json(gateway_path, gateway)
}

fn migrate_v6_to_v7(root: &Path, gateway_path: &Path) -> Result<()> {
    let mut gateway = load_json_or_quarantine::<Value>(root, gateway_path)?.ok_or_else(|| {
        LocalPoolError::new(ErrorCode::InvalidState, "gateway settings are missing")
    })?;
    let gateway = gateway.as_object_mut().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "gateway settings must be an object",
        )
    })?;
    gateway
        .entry("hiddenModels")
        .or_insert_with(|| Value::Array(Vec::new()));
    save_json(gateway_path, gateway)
}

fn migrate_v5_to_v6(root: &Path, gateway_path: &Path) -> Result<()> {
    migrate_record_defaults(root, "sources.json", &[("inPool", Value::Bool(false))])?;
    let path = root.join("records").join("accounts.json");
    let mut records =
        load_json_or_quarantine::<Value>(root, &path)?.unwrap_or_else(|| Value::Array(Vec::new()));
    let records = records.as_array_mut().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "accounts.json must contain an array",
        )
    })?;
    for record in records.iter_mut() {
        let account = record
            .get_mut("account")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "accounts.json records must contain an account object",
                )
            })?;
        account
            .entry("inPool")
            .or_insert_with(|| Value::Bool(false));
    }
    save_json(&path, &records)?;
    let mut gateway = load_json_or_quarantine::<Value>(root, gateway_path)?.ok_or_else(|| {
        LocalPoolError::new(ErrorCode::InvalidState, "gateway settings are missing")
    })?;
    let gateway = gateway.as_object_mut().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "gateway settings must be an object",
        )
    })?;
    gateway
        .entry("quotaRefreshIntervalSeconds")
        .or_insert_with(|| Value::from(300));
    gateway
        .entry("quotaRequestTimeoutSeconds")
        .or_insert_with(|| Value::from(20));
    save_json(gateway_path, gateway)
}

fn migrate_v4_to_v5(root: &Path, gateway_path: &Path) -> Result<()> {
    let mut gateway = load_json_or_quarantine::<Value>(root, gateway_path)?.ok_or_else(|| {
        LocalPoolError::new(ErrorCode::InvalidState, "gateway settings are missing")
    })?;
    let gateway = gateway.as_object_mut().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "gateway settings must be an object",
        )
    })?;
    gateway
        .entry("commonProxyConfigured")
        .or_insert_with(|| Value::Bool(false));
    save_json(gateway_path, gateway)
}

fn migrate_v3_to_v4(root: &Path) -> Result<()> {
    let records = root.join("records");
    fs::create_dir_all(&records).map_err(io_error)?;
    let accounts_path = records.join("accounts.json");
    if !accounts_path.exists() {
        save_json(&accounts_path, &Vec::<Value>::new())?;
    }
    let automations_path = records.join("automations.json");
    if !automations_path.exists() {
        save_json(&automations_path, &AutomationRecords::default())?;
    }
    migrate_record_defaults(root, "keys.json", &[("accountIds", Value::Null)])
}

fn migrate_v2_to_v3(root: &Path, gateway_path: &Path) -> Result<()> {
    let mut gateway = load_json_or_quarantine::<Value>(root, gateway_path)?.ok_or_else(|| {
        LocalPoolError::new(ErrorCode::InvalidState, "gateway settings are missing")
    })?;
    let gateway = gateway.as_object_mut().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "gateway settings must be an object",
        )
    })?;
    gateway
        .entry("maxRetryCandidates")
        .or_insert_with(|| Value::from(3));
    gateway
        .entry("sessionAffinity")
        .or_insert_with(|| Value::Bool(true));
    gateway
        .entry("sessionAffinityTtlSeconds")
        .or_insert_with(|| Value::from(3_600));
    save_json(gateway_path, &gateway)?;

    migrate_record_defaults(
        root,
        "sources.json",
        &[
            ("draining", Value::Bool(false)),
            ("allowedModels", Value::Array(Vec::new())),
            ("excludedModels", Value::Array(Vec::new())),
            ("priority", Value::from(0)),
            ("weight", Value::from(1)),
            ("lastUsedAt", Value::Null),
        ],
    )?;
    migrate_record_defaults(
        root,
        "keys.json",
        &[
            ("sourceIds", Value::Null),
            ("allowedModels", Value::Array(Vec::new())),
            ("excludedModels", Value::Array(Vec::new())),
            ("modelPrefix", Value::Null),
        ],
    )
}

fn migrate_record_defaults(root: &Path, name: &str, defaults: &[(&str, Value)]) -> Result<()> {
    let path = root.join("records").join(name);
    let mut records =
        load_json_or_quarantine::<Value>(root, &path)?.unwrap_or_else(|| Value::Array(Vec::new()));
    let records = records.as_array_mut().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("{name} must contain an array"),
        )
    })?;
    for record in records.iter_mut() {
        let record = record.as_object_mut().ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("{name} must contain objects"),
            )
        })?;
        for (field, value) in defaults {
            record
                .entry((*field).to_string())
                .or_insert_with(|| value.clone());
        }
    }
    save_json(&path, &records)
}

fn migrate_v1_to_v2(root: &Path) -> Result<()> {
    let records = root.join("records");
    fs::create_dir_all(&records).map_err(io_error)?;
    fs::create_dir_all(root.join("telemetry")).map_err(io_error)?;
    for name in ["sources.json", "keys.json"] {
        let path = records.join(name);
        if !path.exists() {
            save_json(&path, &Vec::<Value>::new())?;
        }
    }
    Ok(())
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
    for (source, name) in [
        (root.join("metadata.json"), "metadata.json"),
        (root.join("settings").join("gateway.json"), "gateway.json"),
        (root.join("records").join("sources.json"), "sources.json"),
        (root.join("records").join("keys.json"), "keys.json"),
        (root.join("records").join("accounts.json"), "accounts.json"),
        (
            root.join("records").join("automations.json"),
            "automations.json",
        ),
    ] {
        backup_file(
            &source,
            &target.join(name),
            &target.join(format!("{name}.missing")),
        )?;
    }
    Ok(target)
}

fn restore_settings(root: &Path, backup: &Path) -> Result<()> {
    for (target, name) in [
        (root.join("metadata.json"), "metadata.json"),
        (root.join("settings").join("gateway.json"), "gateway.json"),
        (root.join("records").join("sources.json"), "sources.json"),
        (root.join("records").join("keys.json"), "keys.json"),
        (root.join("records").join("accounts.json"), "accounts.json"),
        (
            root.join("records").join("automations.json"),
            "automations.json",
        ),
    ] {
        restore_file(
            &backup.join(name),
            &backup.join(format!("{name}.missing")),
            &target,
        )?;
    }
    Ok(())
}

fn backup_file(source: &Path, target: &Path, missing_marker: &Path) -> Result<()> {
    if !source.exists() {
        fs::write(missing_marker, b"").map_err(io_error)?;
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    fs::copy(source, target).map(|_| ()).map_err(io_error)
}

fn restore_file(source: &Path, missing_marker: &Path, target: &Path) -> Result<()> {
    if missing_marker.exists() {
        return match fs::remove_file(target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        };
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
        assert!(root.join("records").join("sources.json").exists());
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

    #[test]
    fn failed_migration_restores_absent_metadata_and_legacy_gateway() {
        let root = temp_root();
        let gateway_path = root.join("settings").join("gateway.json");
        save_json(
            &gateway_path,
            &Value::Object(Map::from_iter([
                ("enabled".to_string(), Value::Bool(false)),
                ("port".to_string(), Value::from(14998)),
            ])),
        )
        .unwrap();
        fs::write(root.join("records"), "blocks records directory").unwrap();

        assert!(migrate(&root).is_err());
        assert!(!root.join("metadata.json").exists());
        let gateway: Value = load_json(&gateway_path).unwrap().unwrap();
        assert!(gateway.get("clientHost").is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_v2_records_and_gateway_with_p2_defaults() {
        let root = temp_root();
        write_v2_store(&root);

        assert_eq!(
            migrate(&root).unwrap().schema_version,
            CURRENT_SCHEMA_VERSION
        );
        let gateway: Value = load_json(&root.join("settings").join("gateway.json"))
            .unwrap()
            .unwrap();
        let sources: Value = load_json(&root.join("records").join("sources.json"))
            .unwrap()
            .unwrap();
        let keys: Value = load_json(&root.join("records").join("keys.json"))
            .unwrap()
            .unwrap();
        assert_eq!(gateway["maxRetryCandidates"], 3);
        assert_eq!(sources[0]["weight"], 1);
        assert_eq!(sources[0]["draining"], false);
        assert!(sources[0]["lastUsedAt"].is_null());
        assert!(keys[0]["sourceIds"].is_null());
        assert!(keys[0]["accountIds"].is_null());
        assert!(root.join("records").join("accounts.json").exists());
        assert!(root.join("records").join("automations.json").exists());
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
    fn migrates_v3_store_with_account_and_automation_records() {
        let root = temp_root();
        write_v3_store(&root);

        assert_eq!(
            migrate(&root).unwrap().schema_version,
            CURRENT_SCHEMA_VERSION
        );
        let keys: Value = load_json(&root.join("records").join("keys.json"))
            .unwrap()
            .unwrap();
        let automations: AutomationRecords =
            load_json(&root.join("records").join("automations.json"))
                .unwrap()
                .unwrap();
        assert!(keys[0]["accountIds"].is_null());
        assert!(root.join("records").join("accounts.json").exists());
        assert!(automations.tasks.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_v4_gateway_with_common_proxy_disabled() {
        let root = temp_root();
        write_v3_store(&root);
        migrate_v3_to_v4(&root).unwrap();
        save_json(
            &root.join("metadata.json"),
            &StoreMetadata { schema_version: 4 },
        )
        .unwrap();

        assert_eq!(
            migrate(&root).unwrap().schema_version,
            CURRENT_SCHEMA_VERSION
        );
        let gateway: Value = load_json(&root.join("settings").join("gateway.json"))
            .unwrap()
            .unwrap();
        assert_eq!(gateway["commonProxyConfigured"], false);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_v5_connections_outside_the_pool() {
        let root = temp_root();
        write_v3_store(&root);
        migrate_v3_to_v4(&root).unwrap();
        let gateway_path = root.join("settings").join("gateway.json");
        migrate_v4_to_v5(&root, &gateway_path).unwrap();
        save_json(
            &root.join("records").join("accounts.json"),
            &vec![serde_json::json!({"account": {"id": "account_1", "label": "Preserved"}})],
        )
        .unwrap();
        save_json(
            &root.join("metadata.json"),
            &StoreMetadata { schema_version: 5 },
        )
        .unwrap();

        assert_eq!(
            migrate(&root).unwrap().schema_version,
            CURRENT_SCHEMA_VERSION
        );
        let sources: Value = load_json(&root.join("records").join("sources.json"))
            .unwrap()
            .unwrap();
        let accounts: Value = load_json(&root.join("records").join("accounts.json"))
            .unwrap()
            .unwrap();
        assert_eq!(sources[0]["inPool"], false);
        assert_eq!(accounts[0]["account"]["inPool"], false);
        assert_eq!(accounts[0]["account"]["label"], "Preserved");
        let gateway: Value = load_json(&gateway_path).unwrap().unwrap();
        assert_eq!(gateway["hiddenModels"], Value::Array(Vec::new()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_v7_with_free_accounts_excluded_by_default() {
        let root = temp_root();
        let gateway_path = root.join("settings").join("gateway.json");
        save_json(
            &gateway_path,
            &serde_json::to_value(crate::local_pool::models::GatewaySettings::default()).unwrap(),
        )
        .unwrap();
        let mut gateway: Value = load_json(&gateway_path).unwrap().unwrap();
        gateway.as_object_mut().unwrap().remove("useFreeAccounts");
        save_json(&gateway_path, &gateway).unwrap();
        save_json(
            &root.join("metadata.json"),
            &StoreMetadata { schema_version: 7 },
        )
        .unwrap();

        assert_eq!(
            migrate(&root).unwrap().schema_version,
            CURRENT_SCHEMA_VERSION
        );
        let gateway: Value = load_json(&gateway_path).unwrap().unwrap();
        assert_eq!(gateway["useFreeAccounts"], false);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_v8_by_clearing_legacy_long_cooldowns() {
        let root = temp_root();
        let records = root.join("records");
        fs::create_dir_all(&records).unwrap();
        save_json(
            &root.join("settings").join("gateway.json"),
            &serde_json::to_value(crate::local_pool::models::GatewaySettings::default()).unwrap(),
        )
        .unwrap();
        save_json(
            &records.join("accounts.json"),
            &vec![serde_json::json!({
                "account": {"id": "account_1"},
                "cooldowns": {"*": 1_784_000_000_000_u64, "gpt-5.6-luna": 1_784_001_013_965_u64},
                "consecutiveFailures": 7
            })],
        )
        .unwrap();
        save_json(
            &root.join("metadata.json"),
            &StoreMetadata { schema_version: 8 },
        )
        .unwrap();

        assert_eq!(
            migrate(&root).unwrap().schema_version,
            CURRENT_SCHEMA_VERSION
        );
        let accounts: Value = load_json(&records.join("accounts.json")).unwrap().unwrap();
        assert_eq!(accounts[0]["cooldowns"], serde_json::json!({}));
        assert_eq!(accounts[0]["consecutiveFailures"], 0);
        assert_eq!(accounts[0]["account"]["id"], "account_1");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_v4_write_restores_every_v3_file_and_removes_new_files() {
        let root = temp_root();
        write_v3_store(&root);
        let paths = [
            root.join("metadata.json"),
            root.join("settings").join("gateway.json"),
            root.join("records").join("sources.json"),
            root.join("records").join("keys.json"),
        ];
        let before = paths
            .iter()
            .map(fs::read)
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        fs::create_dir(root.join("records").join("automations.tmp")).unwrap();

        assert!(migrate(&root).is_err());
        for (path, expected) in paths.iter().zip(before) {
            assert_eq!(
                fs::read(path).unwrap(),
                expected,
                "{} changed",
                path.display()
            );
        }
        assert!(!root.join("records").join("accounts.json").exists());
        assert!(!root.join("records").join("automations.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_v3_key_write_restores_every_v2_file() {
        let root = temp_root();
        write_v2_store(&root);
        let paths = [
            root.join("metadata.json"),
            root.join("settings").join("gateway.json"),
            root.join("records").join("sources.json"),
            root.join("records").join("keys.json"),
        ];
        let before = paths
            .iter()
            .map(fs::read)
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        fs::create_dir(root.join("records").join("keys.tmp")).unwrap();

        assert!(migrate(&root).is_err());
        for (path, expected) in paths.iter().zip(before) {
            assert_eq!(
                fs::read(path).unwrap(),
                expected,
                "{} changed",
                path.display()
            );
        }
        let metadata: StoreMetadata = load_json(&paths[0]).unwrap().unwrap();
        let gateway: Value = load_json(&paths[1]).unwrap().unwrap();
        let sources: Value = load_json(&paths[2]).unwrap().unwrap();
        let keys: Value = load_json(&paths[3]).unwrap().unwrap();
        assert_eq!(metadata.schema_version, 2);
        assert!(gateway.get("maxRetryCandidates").is_none());
        assert!(sources[0].get("draining").is_none());
        assert!(keys[0].get("sourceIds").is_none());
        fs::remove_dir_all(root).unwrap();
    }

    fn write_v2_store(root: &Path) {
        fs::create_dir_all(root.join("records")).unwrap();
        save_json(
            &root.join("metadata.json"),
            &StoreMetadata { schema_version: 2 },
        )
        .unwrap();
        save_json(
            &root.join("settings").join("gateway.json"),
            &serde_json::json!({
                "enabled": false,
                "bindScope": "localhost",
                "port": 14998,
                "clientHost": "127.0.0.1"
            }),
        )
        .unwrap();
        save_json(
            &root.join("records").join("sources.json"),
            &vec![serde_json::json!({
                "id": "source_1",
                "name": "Synthetic",
                "enabled": true,
                "baseUrl": "https://example.test/v1",
                "secretRef": "source:source_1",
                "wireApi": "responses",
                "models": ["gpt-test"],
                "lastTestAt": null,
                "lastTestStatus": null,
                "lastError": null
            })],
        )
        .unwrap();
        save_json(
            &root.join("records").join("keys.json"),
            &vec![serde_json::json!({
                "id": "key_1",
                "label": "Default",
                "enabled": true,
                "secretRef": "key:key_1",
                "createdAt": "2026-07-10T00:00:00Z",
                "lastUsedAt": null
            })],
        )
        .unwrap();
    }

    fn write_v3_store(root: &Path) {
        write_v2_store(root);
        let gateway_path = root.join("settings").join("gateway.json");
        migrate_v2_to_v3(root, &gateway_path).unwrap();
        save_json(
            &root.join("metadata.json"),
            &StoreMetadata { schema_version: 3 },
        )
        .unwrap();
    }
}
