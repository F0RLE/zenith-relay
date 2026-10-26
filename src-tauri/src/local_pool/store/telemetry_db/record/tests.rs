use super::*;
use crate::local_pool::store::telemetry_db::tests::aggregate_test_event;
use rusqlite::StatementStatus;

#[test]
fn cached_record_statements_reuse_without_retaining_bound_values() {
    let root = std::env::temp_dir().join(format!("relay-write-cache-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let event = aggregate_test_event("synthetic-cache", 1, 100, 0, None, 10);
    database.record(&event).unwrap();
    let statements = [
        UsageRecordKind::FinalRequest.previous_sql(),
        UsageRecordKind::FinalRequest.upsert_sql(),
        UsageRecordKind::AttemptHistory.previous_sql(),
        UsageRecordKind::AttemptHistory.upsert_sql(),
    ];
    let previous_runs = {
        let connection = database.connection.lock().unwrap();
        statements.map(|sql| {
            let runs = connection
                .prepare_cached(sql)
                .unwrap()
                .get_status(StatementStatus::Run);
            assert!(runs > 0, "record must return its statements to the cache");
            runs
        })
    };
    database.record(&event).unwrap();
    let connection = database.connection.lock().unwrap();
    for (sql, previous_runs) in statements.into_iter().zip(previous_runs) {
        let statement = connection.prepare_cached(sql).unwrap();
        // SQLite may also count an automatic reprepare as a run. The counter
        // must accumulate across records rather than reset to a fresh statement.
        assert!(statement.get_status(StatementStatus::Run) > previous_runs);
        assert!(
            !statement
                .expanded_sql()
                .unwrap()
                .contains("synthetic-cache")
        );
    }
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn attempt_lookup_uses_the_full_identity_instead_of_scanning_other_attempts() {
    let root = std::env::temp_dir().join(format!("relay-attempt-index-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut connection = database.connection.lock().unwrap();
    connection
        .execute_batch(
            "WITH RECURSIVE attempts(n) AS (
                 SELECT 1 UNION ALL SELECT n + 1 FROM attempts WHERE n < 1024
             ) INSERT INTO request_attempts(
                 request_id, attempt, local_key_id, source_id, wire_api,
                 success, http_status, latency_ms, usage_aggregate_recorded
             ) SELECT 'synthetic-many', n, 'key', 'source', 'responses', 1, 200, 1, 0
               FROM attempts;",
        )
        .unwrap();
    let transaction = connection.transaction().unwrap();
    let previous = previous_usage_log(&transaction, "synthetic-many", Some(1024))
        .unwrap()
        .unwrap();
    assert_eq!(previous.attempt, 1024);
    let statement = transaction
        .prepare_cached(UsageRecordKind::AttemptHistory.previous_sql())
        .unwrap();
    let steps = statement.get_status(StatementStatus::VmStep);
    assert!(steps < 100, "point lookup used {steps} VM steps");
    drop(statement);
    drop(transaction);
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
