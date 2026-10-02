use super::*;
use crate::state::ServerAccountRecord;
use crate::store::sqlite::Store;
use crate::store::test_support::test_root;

fn account_record(id: &str) -> ServerAccountRecord {
    crate::test_fixtures::synthetic_server_account(id)
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
fn rotation_migration_removes_only_obsolete_metadata() {
    let mut connection = Connection::open_in_memory().unwrap();
    apply_migrations_through(&mut connection, 38);
    for (key, value) in [
        ("routing_strategy", "quota_highest"),
        ("subscription_plan_order", "not-json"),
        ("cooldown_after_failures", "0"),
        ("keep_last_candidate_available", "false"),
        ("max_retry_candidates", "8"),
        ("gateway_enabled", "true"),
    ] {
        connection.execute(
            "INSERT INTO metadata(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        ).unwrap();
    }
    apply_migrations(&mut connection, 38).unwrap();
    validate_migration_ledger(&connection).unwrap();
    for key in [
        "routing_strategy",
        "subscription_plan_order",
        "cooldown_after_failures",
        "keep_last_candidate_available",
    ] {
        let found: Option<String> = connection
            .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .unwrap();
        assert!(found.is_none(), "{key} must be retired");
    }
    for (key, expected) in [("max_retry_candidates", "8"), ("gateway_enabled", "true")] {
        let found: String = connection
            .query_row("SELECT value FROM metadata WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(found, expected);
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
    let root = std::env::temp_dir().join(format!("zenith-relay-store-{}", uuid::Uuid::new_v4()));
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
            (35, "035_cache_write_ttl".to_string()),
            (36, "036_upstream_error_details".to_string()),
            (37, "037_account_refresh_revisions".to_string()),
            (38, "038_source_refresh_revisions".to_string()),
            (39, "039_remove_v1_routing_options".to_string())
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
        .execute_batch(include_str!("../../../migrations/001_init.sql"))
        .unwrap();
    connection
        .execute(
            "INSERT INTO metadata(key, value) VALUES ('server_id', ?1)",
            [server_id],
        )
        .unwrap();
}
