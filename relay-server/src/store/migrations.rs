use super::sqlite::{db_error, io_error, unix_time_ms};
use crate::state::SERVER_SCHEMA_VERSION;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "001_init",
        sql: include_str!("../../migrations/001_init.sql"),
    },
    Migration {
        version: 2,
        name: "002_migration_ledger",
        sql: include_str!("../../migrations/002_migration_ledger.sql"),
    },
    Migration {
        version: 3,
        name: "003_usage_query_indexes",
        sql: include_str!("../../migrations/003_usage_query_indexes.sql"),
    },
    Migration {
        version: 4,
        name: "004_account_proxies",
        sql: include_str!("../../migrations/004_account_proxies.sql"),
    },
    Migration {
        version: 5,
        name: "005_pool_membership",
        sql: include_str!("../../migrations/005_pool_membership.sql"),
    },
    Migration {
        version: 6,
        name: "006_model_rules",
        sql: include_str!("../../migrations/006_model_rules.sql"),
    },
    Migration {
        version: 7,
        name: "007_cached_input_tokens",
        sql: include_str!("../../migrations/007_cached_input_tokens.sql"),
    },
    Migration {
        version: 8,
        name: "008_reasoning_tokens",
        sql: include_str!("../../migrations/008_reasoning_tokens.sql"),
    },
    Migration {
        version: 9,
        name: "009_ttft_ms",
        sql: include_str!("../../migrations/009_ttft_ms.sql"),
    },
    Migration {
        version: 10,
        name: "010_reset_legacy_cooldowns",
        sql: include_str!("../../migrations/010_reset_legacy_cooldowns.sql"),
    },
    Migration {
        version: 11,
        name: "011_request_rotation_default",
        sql: include_str!("../../migrations/011_request_rotation_default.sql"),
    },
    Migration {
        version: 12,
        name: "012_routing_diagnostics",
        sql: include_str!("../../migrations/012_routing_diagnostics.sql"),
    },
    Migration {
        version: 13,
        name: "013_routing_strategy",
        sql: include_str!("../../migrations/013_routing_strategy.sql"),
    },
    Migration {
        version: 14,
        name: "014_default_service_tier",
        sql: include_str!("../../migrations/014_default_service_tier.sql"),
    },
    Migration {
        version: 15,
        name: "015_cache_write_input_tokens",
        sql: include_str!("../../migrations/015_cache_write_input_tokens.sql"),
    },
    Migration {
        version: 16,
        name: "016_response_affinity",
        sql: include_str!("../../migrations/016_response_affinity.sql"),
    },
    Migration {
        version: 17,
        name: "017_generation_ms",
        sql: include_str!("../../migrations/017_generation_ms.sql"),
    },
    Migration {
        version: 18,
        name: "018_image_base_model",
        sql: include_str!("../../migrations/018_image_base_model.sql"),
    },
    Migration {
        version: 19,
        name: "019_remove_cache_write_input_tokens",
        sql: include_str!("../../migrations/019_remove_cache_write_input_tokens.sql"),
    },
    Migration {
        version: 20,
        name: "020_cache_write_input_tokens",
        sql: include_str!("../../migrations/020_cache_write_input_tokens.sql"),
    },
    Migration {
        version: 21,
        name: "021_terminal_usage_per_request",
        sql: include_str!("../../migrations/021_terminal_usage_per_request.sql"),
    },
    Migration {
        version: 22,
        name: "022_usage_retention_rollups",
        sql: include_str!("../../migrations/022_usage_retention_rollups.sql"),
    },
    Migration {
        version: 23,
        name: "023_server_proxy_objects",
        sql: include_str!("../../migrations/023_server_proxy_objects.sql"),
    },
    Migration {
        version: 24,
        name: "024_remove_free_account_policy",
        sql: include_str!("../../migrations/024_remove_free_account_policy.sql"),
    },
    Migration {
        version: 25,
        name: "025_usage_effective_credits",
        sql: include_str!("../../migrations/025_usage_effective_credits.sql"),
    },
    Migration {
        version: 26,
        name: "026_remove_effective_credits",
        sql: include_str!("../../migrations/026_remove_effective_credits.sql"),
    },
    Migration {
        version: 27,
        name: "027_remove_quota_refresh_interval",
        sql: include_str!("../../migrations/027_remove_quota_refresh_interval.sql"),
    },
    Migration {
        version: 28,
        name: "028_applied_service_tier",
        sql: include_str!("../../migrations/028_applied_service_tier.sql"),
    },
    Migration {
        version: 29,
        name: "029_candidate_usage_rollups",
        sql: include_str!("../../migrations/029_candidate_usage_rollups.sql"),
    },
    Migration {
        version: 30,
        name: "030_source_priced_key_rollups",
        sql: include_str!("../../migrations/030_source_priced_key_rollups.sql"),
    },
    Migration {
        version: 31,
        name: "031_tool_use_diagnostics",
        sql: include_str!("../../migrations/031_tool_use_diagnostics.sql"),
    },
    Migration {
        version: 32,
        name: "032_error_origin",
        sql: include_str!("../../migrations/032_error_origin.sql"),
    },
    Migration {
        version: 33,
        name: "033_reasoning_effort",
        sql: include_str!("../../migrations/033_reasoning_effort.sql"),
    },
    Migration {
        version: 34,
        name: "034_account_purchase_cost",
        sql: include_str!("../../migrations/034_account_purchase_cost.sql"),
    },
    Migration {
        version: 35,
        name: "035_cache_write_ttl",
        sql: include_str!("../../migrations/035_cache_write_ttl.sql"),
    },
];

struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
}

pub(super) fn read_schema_version(connection: &Connection) -> Result<u32, String> {
    let has_metadata = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'metadata')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(db_error)?;
    if !has_metadata {
        return Ok(0);
    }
    let value = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    value
        .as_deref()
        .unwrap_or("0")
        .parse::<u32>()
        .map_err(|_| "database schema version is invalid".to_string())
}

pub(super) fn apply_migrations(
    connection: &mut Connection,
    current_version: u32,
) -> Result<(), String> {
    if MIGRATIONS.last().map(|migration| migration.version) != Some(SERVER_SCHEMA_VERSION)
        || !MIGRATIONS
            .iter()
            .enumerate()
            .all(|(index, migration)| migration.version == index as u32 + 1)
    {
        return Err("database migration registry is invalid".to_string());
    }
    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version > current_version)
    {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        transaction.execute_batch(migration.sql).map_err(db_error)?;
        if migration.version >= 2 {
            transaction
                .execute(
                    "INSERT INTO schema_migrations(version, name, applied_at_ms) VALUES (?1, ?2, ?3)",
                    params![
                        i64::from(migration.version),
                        migration.name,
                        unix_time_ms().min(i64::MAX as u64) as i64
                    ],
                )
                .map_err(db_error)?;
        }
        transaction
            .execute(
                "INSERT INTO metadata(key, value) VALUES ('schema_version', ?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [migration.version.to_string()],
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
    }
    Ok(())
}

pub(super) fn validate_migration_ledger(connection: &Connection) -> Result<(), String> {
    let mut statement = connection
        .prepare("SELECT version, name FROM schema_migrations ORDER BY version")
        .map_err(|_| "database migration ledger is missing".to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    let expected = MIGRATIONS
        .iter()
        .map(|migration| (migration.version, migration.name.to_string()))
        .collect::<Vec<_>>();
    if rows != expected {
        return Err("database migration ledger is invalid".to_string());
    }
    Ok(())
}

pub(super) fn prepare_migration_backup(
    connection: &Connection,
    path: &Path,
    from_version: u32,
    to_version: u32,
) -> Result<(), String> {
    let backup_path = sibling_path(path, ".pre-migration");
    if backup_path.exists() {
        fs::remove_file(&backup_path).map_err(io_error)?;
    }
    connection
        .backup(rusqlite::MAIN_DB, &backup_path, None)
        .map_err(db_error)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&backup_path)
        .and_then(|file| file.sync_all())
        .map_err(io_error)?;
    if validate_database_file(&backup_path)? != from_version {
        return Err("pre-migration backup version is invalid".to_string());
    }

    let marker_path = sibling_path(path, ".migration-in-progress");
    let mut marker = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(marker_path)
        .map_err(io_error)?;
    writeln!(marker, "{from_version}:{to_version}").map_err(io_error)?;
    marker.sync_all().map_err(io_error)
}

pub(super) fn finish_migration(path: &Path) -> Result<(), String> {
    let marker_path = sibling_path(path, ".migration-in-progress");
    if marker_path.exists() {
        fs::remove_file(marker_path).map_err(io_error)?;
    }
    Ok(())
}

pub(super) fn recover_interrupted_migration(path: &Path) -> Result<(), String> {
    let marker_path = sibling_path(path, ".migration-in-progress");
    if !marker_path.exists() {
        return Ok(());
    }
    let backup_path = sibling_path(path, ".pre-migration");
    if !backup_path.is_file() {
        return Err("interrupted database migration backup is missing".to_string());
    }
    let marker = fs::read_to_string(&marker_path).map_err(io_error)?;
    let (from_version, to_version) = marker
        .trim()
        .split_once(':')
        .and_then(|(from, to)| Some((from.parse::<u32>().ok()?, to.parse::<u32>().ok()?)))
        .ok_or_else(|| "database migration marker is invalid".to_string())?;
    if to_version != SERVER_SCHEMA_VERSION
        || from_version >= to_version
        || validate_database_file(&backup_path)? != from_version
    {
        return Err("database migration recovery metadata is invalid".to_string());
    }
    restore_database_file(&backup_path, path)?;
    fs::remove_file(marker_path).map_err(io_error)
}

fn validate_database_file(path: &Path) -> Result<u32, String> {
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(db_error)?;
    let integrity = connection
        .query_row("PRAGMA integrity_check(1)", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(db_error)?;
    if integrity != "ok" {
        return Err("database backup integrity check failed".to_string());
    }
    let version = read_schema_version(&connection)?;
    if version > SERVER_SCHEMA_VERSION {
        return Err("database backup schema is newer than this server".to_string());
    }
    Ok(version)
}

fn restore_database_file(source: &Path, target: &Path) -> Result<(), String> {
    let temporary = sibling_path(target, ".migration-restore.tmp");
    let previous = sibling_path(target, ".failed-migration");
    for path in [&temporary, &previous] {
        if path.exists() {
            fs::remove_file(path).map_err(io_error)?;
        }
    }
    fs::copy(source, &temporary).map_err(io_error)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temporary)
        .and_then(|file| file.sync_all())
        .map_err(io_error)?;
    for suffix in ["-wal", "-shm"] {
        let path = sibling_path(target, suffix);
        if path.exists() {
            fs::remove_file(path).map_err(io_error)?;
        }
    }
    if target.exists() {
        fs::rename(target, &previous).map_err(io_error)?;
    }
    if let Err(error) = fs::rename(&temporary, target) {
        if previous.exists() {
            let _ = fs::rename(&previous, target);
        }
        return Err(io_error(error));
    }
    if previous.exists() {
        fs::remove_file(previous).map_err(io_error)?;
    }
    Ok(())
}

pub(super) fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ServerAccountRecord;
    use crate::store::sqlite::Store;
    use crate::store::test_support::test_root;
    use zenith_relay_core::accounts::{AccountAuthState, AccountHealthState};

    fn account_record(id: &str) -> ServerAccountRecord {
        ServerAccountRecord {
            id: id.to_string(),
            label: id.to_string(),
            identity_hint: id.to_string(),
            enabled: true,
            in_pool: true,
            draining: false,
            source_id: "openai_codex".to_string(),
            secret_ref: format!("account:{id}"),
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            models: vec!["gpt-test".to_string()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            subscription: Default::default(),
            quota: Default::default(),
            purchase_cost_micro_usd: None,
            cooldowns: Default::default(),
            consecutive_failures: 0,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
            proxy_id: None,
            bypass_common_proxy: false,
        }
    }

    fn apply_migrations_through(connection: &mut Connection, target_version: u32) {
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= target_version)
        {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            transaction.execute_batch(migration.sql).unwrap();
            if migration.version >= 2 {
                transaction
                    .execute(
                        "INSERT INTO schema_migrations(version, name, applied_at_ms) VALUES (?1, ?2, ?3)",
                        params![i64::from(migration.version), migration.name, 0_i64],
                    )
                    .unwrap();
            }
            transaction
                .execute(
                    "INSERT INTO metadata(key, value) VALUES ('schema_version', ?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    [migration.version.to_string()],
                )
                .unwrap();
            transaction.commit().unwrap();
        }
    }

    #[test]
    fn pool_membership_migration_defaults_existing_records_outside_pool() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(MIGRATIONS[0].sql).unwrap();
        connection
            .execute(
                "INSERT INTO sources(id, data_json, secret_ref) VALUES ('source_1', '{\"id\":\"source_1\",\"name\":\"Preserved\"}', 'source:1')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO accounts(id, data_json, secret_ref) VALUES ('account_1', '{\"id\":\"account_1\",\"label\":\"Preserved\"}', 'account:1')",
                [],
            )
            .unwrap();
        connection.execute_batch(MIGRATIONS[4].sql).unwrap();
        let source: bool = connection
            .query_row(
                "SELECT json_extract(data_json, '$.inPool') FROM sources WHERE id = 'source_1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let account: bool = connection
            .query_row(
                "SELECT json_extract(data_json, '$.inPool') FROM accounts WHERE id = 'account_1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!source);
        assert!(!account);
    }

    #[test]
    fn cooldown_migration_clears_legacy_long_delays() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(MIGRATIONS[0].sql).unwrap();
        connection
            .execute(
                "INSERT INTO accounts(id, data_json, secret_ref) VALUES ('account_1', '{\"id\":\"account_1\",\"cooldowns\":{\"*\":1784000000000,\"gpt-5.6-luna\":1784001013965},\"consecutiveFailures\":7}', 'account:1')",
                [],
            )
            .unwrap();
        connection.execute_batch(MIGRATIONS[9].sql).unwrap();
        let (cooldowns, failures): (String, u32) = connection
            .query_row(
                "SELECT json_extract(data_json, '$.cooldowns'), json_extract(data_json, '$.consecutiveFailures') FROM accounts WHERE id = 'account_1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cooldowns, "{}");
        assert_eq!(failures, 0);
    }

    #[test]
    fn account_purchase_cost_migration_preserves_direct_values_and_removes_legacy_economics() {
        let root = test_root("account-purchase-cost-migration");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("relay.sqlite");
        let mut connection = Connection::open(&path).unwrap();
        apply_migrations_through(&mut connection, 33);

        let mut direct = serde_json::to_value(account_record("account_direct")).unwrap();
        direct["purchaseCostMicroUsd"] = serde_json::json!(42_000_000_u64);
        direct["economics"] = serde_json::json!({
            "purchaseCostMicroUsd": 13_000_000_u64,
            "sampleCount": 8
        });
        let mut legacy = serde_json::to_value(account_record("account_legacy")).unwrap();
        legacy["economics"] = serde_json::json!({
            "purchaseCostMicroUsd": 21_000_000_u64,
            "sampleCount": 3
        });
        let mut no_cost = serde_json::to_value(account_record("account_without_cost")).unwrap();
        no_cost["economics"] = serde_json::json!({ "sampleCount": 1 });
        for (id, value) in [
            ("account_direct", direct),
            ("account_legacy", legacy),
            ("account_without_cost", no_cost),
        ] {
            connection
                .execute(
                    "INSERT INTO accounts(id, data_json, secret_ref) VALUES (?1, ?2, ?3)",
                    params![id, value.to_string(), format!("account:{id}")],
                )
                .unwrap();
        }
        drop(connection);

        let store = Store::open(path.clone()).unwrap();
        assert_eq!(
            store
                .account("account_direct")
                .unwrap()
                .unwrap()
                .purchase_cost_micro_usd,
            Some(42_000_000)
        );
        assert_eq!(
            store
                .account("account_legacy")
                .unwrap()
                .unwrap()
                .purchase_cost_micro_usd,
            Some(21_000_000)
        );
        assert_eq!(
            store
                .account("account_without_cost")
                .unwrap()
                .unwrap()
                .purchase_cost_micro_usd,
            None
        );
        {
            let connection = store.lock().unwrap();
            let legacy_objects: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM accounts WHERE json_type(data_json, '$.economics') IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(legacy_objects, 0);
        }
        drop(store);

        let reopened = Store::open(path).unwrap();
        assert_eq!(
            reopened
                .account("account_legacy")
                .unwrap()
                .unwrap()
                .purchase_cost_micro_usd,
            Some(21_000_000)
        );
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn store_migrates_and_preserves_server_identity() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-store-{}", uuid::Uuid::new_v4()));
        let path = root.join("relay.sqlite");
        let first = Store::open(path.clone()).unwrap();
        let server_id = first.server_id().unwrap();
        assert!(first.gateway_enabled().unwrap());
        assert!(!first.common_proxy_configured().unwrap());
        assert!(!first.account_proxy_required().unwrap());
        first.set_common_proxy_configured(true).unwrap();
        first.set_account_proxy_required(true).unwrap();
        assert_eq!(
            first.metadata("schema_version").unwrap(),
            Some(SERVER_SCHEMA_VERSION.to_string())
        );
        drop(first);
        let second = Store::open(path).unwrap();
        assert_eq!(second.server_id().unwrap(), server_id);
        assert!(second.common_proxy_configured().unwrap());
        assert!(second.account_proxy_required().unwrap());
        drop(second);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn v1_migration_creates_backup_and_ordered_ledger() {
        let root = test_root("v1-migration");
        let path = root.join("relay.sqlite");
        create_v1_database(&path, "stable-server-id");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO metadata(key, value) VALUES ('use_free_accounts', 'true')",
                [],
            )
            .unwrap();
        drop(connection);

        let store = Store::open(path.clone()).unwrap();
        assert_eq!(store.server_id().unwrap(), "stable-server-id");
        assert_eq!(store.metadata("use_free_accounts").unwrap(), None);
        assert_eq!(
            store.metadata("schema_version").unwrap(),
            Some(SERVER_SCHEMA_VERSION.to_string())
        );
        let columns = {
            let connection = store.lock().unwrap();
            let mut statement = connection
                .prepare("SELECT name FROM pragma_table_info('usage_events')")
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(columns
            .iter()
            .any(|column| column == "requested_reasoning_effort"));
        assert!(columns
            .iter()
            .any(|column| column == "effective_reasoning_effort"));
        let ledger = {
            let connection = store.lock().unwrap();
            let mut statement = connection
                .prepare("SELECT version, name FROM schema_migrations ORDER BY version")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            ledger,
            vec![
                (1, "001_init".to_string()),
                (2, "002_migration_ledger".to_string()),
                (3, "003_usage_query_indexes".to_string()),
                (4, "004_account_proxies".to_string()),
                (5, "005_pool_membership".to_string()),
                (6, "006_model_rules".to_string()),
                (7, "007_cached_input_tokens".to_string()),
                (8, "008_reasoning_tokens".to_string()),
                (9, "009_ttft_ms".to_string()),
                (10, "010_reset_legacy_cooldowns".to_string()),
                (11, "011_request_rotation_default".to_string()),
                (12, "012_routing_diagnostics".to_string()),
                (13, "013_routing_strategy".to_string()),
                (14, "014_default_service_tier".to_string()),
                (15, "015_cache_write_input_tokens".to_string()),
                (16, "016_response_affinity".to_string()),
                (17, "017_generation_ms".to_string()),
                (18, "018_image_base_model".to_string()),
                (19, "019_remove_cache_write_input_tokens".to_string()),
                (20, "020_cache_write_input_tokens".to_string()),
                (21, "021_terminal_usage_per_request".to_string()),
                (22, "022_usage_retention_rollups".to_string()),
                (23, "023_server_proxy_objects".to_string()),
                (24, "024_remove_free_account_policy".to_string()),
                (25, "025_usage_effective_credits".to_string()),
                (26, "026_remove_effective_credits".to_string()),
                (27, "027_remove_quota_refresh_interval".to_string()),
                (28, "028_applied_service_tier".to_string()),
                (29, "029_candidate_usage_rollups".to_string()),
                (30, "030_source_priced_key_rollups".to_string()),
                (31, "031_tool_use_diagnostics".to_string()),
                (32, "032_error_origin".to_string()),
                (33, "033_reasoning_effort".to_string()),
                (34, "034_account_purchase_cost".to_string()),
                (35, "035_cache_write_ttl".to_string())
            ]
        );
        drop(store);

        let backup = Connection::open(sibling_path(&path, ".pre-migration")).unwrap();
        assert_eq!(read_schema_version(&backup).unwrap(), 1);
        drop(backup);
        assert!(!sibling_path(&path, ".migration-in-progress").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn newer_schema_is_rejected_without_rewriting_database() {
        let root = test_root("newer-schema");
        let path = root.join("relay.sqlite");
        create_v1_database(&path, "future-server-id");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE metadata SET value = '99' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();

        let error = Store::open(path.clone()).err().unwrap();
        assert!(error.contains("newer than supported"));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!sibling_path(&path, ".pre-migration").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_migration_restores_backup_before_retry() {
        let root = test_root("interrupted-migration");
        let path = root.join("relay.sqlite");
        create_v1_database(&path, "original-server-id");
        let connection = Connection::open(&path).unwrap();
        connection
            .backup(
                rusqlite::MAIN_DB,
                sibling_path(&path, ".pre-migration"),
                None,
            )
            .unwrap();
        connection
            .execute(
                "UPDATE metadata SET value = 'corrupted-server-id' WHERE key = 'server_id'",
                [],
            )
            .unwrap();
        drop(connection);
        fs::write(
            sibling_path(&path, ".migration-in-progress"),
            format!("1:{SERVER_SCHEMA_VERSION}\n"),
        )
        .unwrap();

        let store = Store::open(path.clone()).unwrap();
        assert_eq!(store.server_id().unwrap(), "original-server-id");
        assert_eq!(
            store.metadata("schema_version").unwrap(),
            Some(SERVER_SCHEMA_VERSION.to_string())
        );
        assert!(!sibling_path(&path, ".migration-in-progress").exists());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_interrupted_backup_never_replaces_live_database() {
        let root = test_root("corrupt-migration-backup");
        let path = root.join("relay.sqlite");
        create_v1_database(&path, "live-server-id");
        let before = fs::read(&path).unwrap();
        fs::write(sibling_path(&path, ".pre-migration"), b"not a database").unwrap();
        fs::write(sibling_path(&path, ".migration-in-progress"), b"1:3\n").unwrap();

        assert!(Store::open(path.clone()).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    fn create_v1_database(path: &Path, server_id: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(include_str!("../../migrations/001_init.sql"))
            .unwrap();
        connection
            .execute(
                "INSERT INTO metadata(key, value) VALUES ('server_id', ?1)",
                [server_id],
            )
            .unwrap();
    }
}
