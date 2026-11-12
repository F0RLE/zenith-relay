use super::performance_samples::with_database;
use super::*;

fn sample_count(database: &TelemetryDb, name: &str) -> i64 {
    database
        .connection
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM performance_samples WHERE name = ?1",
            [name],
            |row| row.get(0),
        )
        .unwrap()
}

fn flood_snapshots(database: &TelemetryDb) {
    let samples = (0..1024)
        .flat_map(|index| {
            ["full_snapshot", "full_snapshot_native", "snapshot_stage"].map(|name| {
                PerformanceSample {
                    name,
                    duration_ms: f64::from(index),
                    context: Some("input"),
                }
            })
        })
        .collect::<Vec<_>>();
    database.record_performance_batch(&samples).unwrap();
}

#[test]
fn performance_retention_preserves_rare_metrics_when_snapshots_are_frequent() {
    with_database(|database| {
        database
            .record_performance("native_startup", 42.0, None)
            .unwrap();
        database
            .record_performance("command_save", 12.0, Some("save_settings:ok"))
            .unwrap();
        flood_snapshots(database);
        assert_eq!(sample_count(database, "native_startup"), 1);
        assert_eq!(sample_count(database, "command_save"), 1);
        for name in ["full_snapshot", "full_snapshot_native", "snapshot_stage"] {
            let expected = if name == "snapshot_stage" { 256 } else { 128 };
            assert_eq!(sample_count(database, name), expected);
            let minimum: f64 = database
                .connection
                .lock()
                .unwrap()
                .query_row(
                    "SELECT MIN(duration_ms) FROM performance_samples WHERE name = ?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(minimum, 1024.0 - expected as f64);
        }
    });
}

#[test]
fn performance_retention_enforces_age_and_global_row_count_after_id_gaps() {
    with_database(|database| {
        database
            .record_performance("native_startup", 1.0, None)
            .unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute_batch(
                "UPDATE performance_samples SET created_at = datetime('now', '-31 days');
                 WITH RECURSIVE samples(n) AS (
                    VALUES(1) UNION ALL SELECT n + 1 FROM samples WHERE n < 2200
                 ) INSERT INTO performance_samples(name, duration_ms)
                   SELECT printf('synthetic_%d', n), 1.0 FROM samples;",
            )
            .unwrap();
        assert_eq!(sample_count(database, "native_startup"), 0);
        let count: i64 = database
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM performance_samples", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 2048);
        assert_eq!(sample_count(database, "synthetic_152"), 0);
        assert_eq!(sample_count(database, "synthetic_153"), 1);
    });
}

#[test]
fn performance_retention_reserves_capacity_for_all_current_metrics() {
    let names = [
        "native_startup",
        "vault",
        "sqlite",
        "window",
        "first_frame",
        "interactive",
        "full_snapshot",
        "full_snapshot_native",
        "snapshot_stage",
        "snapshot_lock_wait",
        "mode_switch",
        "page_open",
        "command_save",
        "command_switch",
        "command_refresh",
    ];
    with_database(|database| {
        for name in names {
            let context = match name {
                "command_save" => Some("save_settings:ok"),
                "command_switch" => Some("switch_pool:ok"),
                "command_refresh" => Some("refresh_catalog:ok"),
                "snapshot_lock_wait" => Some("store"),
                _ => None,
            };
            let samples = vec![
                PerformanceSample {
                    name,
                    duration_ms: 1.0,
                    context,
                };
                256
            ];
            database.record_performance_batch(&samples).unwrap();
        }
        flood_snapshots(database);
        let total = names
            .iter()
            .map(|name| {
                let count = sample_count(database, name);
                assert_eq!(count, if *name == "snapshot_stage" { 256 } else { 128 });
                count
            })
            .sum::<i64>();
        assert_eq!(total, 2048);
    });
}

#[test]
fn performance_retention_migrates_transactionally_and_survives_reopen() {
    let root = crate::files::test_temp_root("relay-performance-retention");
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    database
        .record(&aggregate_test_event("retained-request", 1, 11, 0, None, 7))
        .unwrap();
    {
        let connection = database.connection.lock().unwrap();
        connection
            .execute_batch(
                "DROP TRIGGER performance_samples_retention;
                 DROP INDEX performance_samples_name_id;
                 CREATE TRIGGER performance_samples_retention
                 AFTER INSERT ON performance_samples BEGIN
                    DELETE FROM performance_samples WHERE created_at < datetime('now', '-30 days');
                    DELETE FROM performance_samples WHERE id <= NEW.id - 2048;
                 END;
                 PRAGMA user_version = 44;
                 INSERT INTO performance_samples(name, duration_ms) VALUES ('native_startup', 42.0);
                 WITH RECURSIVE samples(n) AS (
                    VALUES(1) UNION ALL SELECT n + 1 FROM samples WHERE n < 512
                 ) INSERT INTO performance_samples(name, duration_ms)
                   SELECT 'full_snapshot_native', n FROM samples;",
            )
            .unwrap();
    }
    assert_eq!(sample_count(&database, "full_snapshot_native"), 512);
    drop(database);
    {
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_performance_cleanup BEFORE DELETE ON performance_samples
                 BEGIN SELECT RAISE(ABORT, 'synthetic migration failure'); END;",
            )
            .unwrap();
    }
    assert!(TelemetryDb::open(&path).is_err());
    {
        let connection = Connection::open(&path).unwrap();
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 44);
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM performance_samples", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 513);
        let index_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'performance_samples_name_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 0);
        connection
            .execute_batch("DROP TRIGGER fail_performance_cleanup;")
            .unwrap();
    }
    let database = TelemetryDb::open(&path).unwrap();
    assert_eq!(sample_count(&database, "native_startup"), 1);
    assert_eq!(sample_count(&database, "full_snapshot_native"), 128);
    assert_eq!(database.list(10).unwrap().len(), 1);
    flood_snapshots(&database);
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    assert_eq!(sample_count(&database, "native_startup"), 1);
    assert_eq!(sample_count(&database, "full_snapshot_native"), 128);
    assert_eq!(sample_count(&database, "snapshot_stage"), 256);
    assert_eq!(database.list(10).unwrap().len(), 1);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn performance_retention_uses_the_metric_index() {
    with_database(|database| {
        let connection = database.connection.lock().unwrap();
        let mut statement = connection
            .prepare(
                "EXPLAIN QUERY PLAN SELECT id FROM performance_samples WHERE name = ?1
             ORDER BY id DESC LIMIT -1 OFFSET 128",
            )
            .unwrap();
        let plan = statement
            .query_map(["full_snapshot_native"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(plan.contains("performance_samples_name_id"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    });
}
