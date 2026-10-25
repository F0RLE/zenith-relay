use super::*;

#[test]
fn retention_is_bounded_oldest_first_and_preserves_all_send_totals() {
    let root = std::env::temp_dir().join(format!("relay-retention-batch-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    for index in 0..600 {
        database
            .record(&aggregate_test_event(
                &format!("old-{index}"),
                1,
                10,
                0,
                None,
                2,
            ))
            .unwrap();
    }
    {
        let connection = database.connection.lock().unwrap();
        connection
            .execute(
                "UPDATE request_logs SET created_at = datetime('now', '-31 days')",
                [],
            )
            .unwrap();
        connection.execute("UPDATE request_logs SET created_at = datetime('now', '-32 days') WHERE request_id = 'old-599'", []).unwrap();
    }
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    {
        let connection = database.connection.lock().unwrap();
        assert_eq!(count(&connection, "SELECT COUNT(*) FROM request_logs"), 344);
        assert_eq!(
            count(
                &connection,
                "SELECT COUNT(*) FROM request_logs WHERE request_id = 'old-599'",
            ),
            0
        );
        // Maintenance must not wait on the writer's lock.
        assert_eq!(database.maintain_usage().unwrap(), 0);
    }
    assert_eq!(database.maintain_usage().unwrap(), 256);
    assert_eq!(database.maintain_usage().unwrap(), 88);
    assert_eq!(database.maintain_usage().unwrap(), 0);
    {
        let connection = database.connection.lock().unwrap();
        assert_eq!(count(&connection, "SELECT COUNT(*) FROM request_logs"), 0);
        assert_eq!(
            count(
                &connection,
                "SELECT SUM(input_tokens) FROM usage_candidate_rollups"
            ),
            6000
        );
        assert_eq!(
            count(
                &connection,
                "SELECT SUM(output_tokens) FROM usage_candidate_rollups"
            ),
            1200
        );
    }
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

fn count(connection: &Connection, sql: &str) -> i64 {
    connection.query_row(sql, [], |row| row.get(0)).unwrap()
}

#[test]
fn retention_failure_rolls_back_before_the_next_batch() {
    let root =
        std::env::temp_dir().join(format!("relay-retention-rollback-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    database
        .record(&aggregate_test_event("synthetic", 1, 10, 0, None, 2))
        .unwrap();
    {
        let connection = database.connection.lock().unwrap();
        // Simulate retained legacy sends not yet included in aggregates.
        connection
            .execute_batch(
                "UPDATE request_logs SET created_at = datetime('now', '-31 days'), usage_aggregate_recorded = 0;
                 UPDATE request_attempts SET usage_aggregate_recorded = 0;
                 DELETE FROM usage_candidate_rollups;
                 CREATE TEMP TRIGGER reject_retention BEFORE DELETE ON request_logs
                 BEGIN SELECT RAISE(ABORT, 'synthetic retention failure'); END;",
            )
            .unwrap();
    }
    assert!(database.maintain_usage().is_err());
    {
        let connection = database.connection.lock().unwrap();
        assert_eq!(count(&connection, "SELECT COUNT(*) FROM request_logs"), 1);
        assert_eq!(
            count(&connection, "SELECT COUNT(*) FROM usage_candidate_rollups"),
            0
        );
        connection
            .execute_batch("DROP TRIGGER reject_retention;")
            .unwrap();
    }
    assert_eq!(database.maintain_usage().unwrap(), 1);
    assert_eq!(database.maintain_usage().unwrap(), 0);
    {
        let connection = database.connection.lock().unwrap();
        assert_eq!(
            count(
                &connection,
                "SELECT SUM(input_tokens) FROM usage_candidate_rollups"
            ),
            10
        );
    }
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
