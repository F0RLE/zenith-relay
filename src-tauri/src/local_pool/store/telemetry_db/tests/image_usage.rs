use super::*;

#[test]
fn image_usage_survives_failure_reopen_and_duplicate_observation() {
    let root = std::env::temp_dir().join(format!("relay-image-usage-{}", uuid::Uuid::new_v4()));
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    let mut event = aggregate_test_event("image-request", 1, 10, 0, None, 5);
    event.requested_model = Some("gpt-image-test".into());
    event.resolved_model = event.requested_model.clone();
    event.success = false;
    event.http_status = 502;
    event.error_category = Some("client_cancelled".into());
    event.image_usage = Some(zenith_relay_core::usage::ImageUsage {
        input_text_tokens: Some(3),
        input_image_tokens: Some(7),
        output_image_tokens: Some(5),
        input_details_consistent: Some(true),
        generated_images: 1,
        ..Default::default()
    });
    database.record(&event).unwrap();
    database.record(&event).unwrap();
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    let rows = database.list(10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].image_usage, event.image_usage);
    assert_eq!(rows[0].total_tokens, Some(15));
    let report = database.request_diagnostics("image-request").unwrap();
    assert_eq!(
        report["finalRequest"]["usage"]["imageUsage"]["inputImageTokens"],
        7
    );
    assert_eq!(
        report["summary"]["consumption"]["totalTokens"]["observedTotal"],
        15
    );
    assert_eq!(report["summary"]["includedSends"], 1);
    let connection = database.connection.lock().unwrap();
    let sends: i64 = connection
        .query_row("SELECT COUNT(*) FROM usage_sends", [], |row| row.get(0))
        .unwrap();
    assert_eq!(sends, 1);
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn image_usage_migration_preserves_legacy_rows_and_send_view() {
    let root = std::env::temp_dir().join(format!("relay-image-migration-{}", uuid::Uuid::new_v4()));
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    database
        .record(&aggregate_test_event("legacy", 1, 2, 0, None, 3))
        .unwrap();
    database.connection.lock().unwrap().execute_batch(
        "DROP VIEW usage_sends;
         ALTER TABLE request_logs DROP COLUMN image_usage_json;
         ALTER TABLE request_attempts DROP COLUMN image_usage_json;
         CREATE VIEW usage_sends AS SELECT * FROM request_attempts UNION ALL
             SELECT * FROM request_logs WHERE NOT EXISTS (SELECT 1 FROM request_attempts
                 WHERE request_attempts.request_id=request_logs.request_id AND request_attempts.attempt=request_logs.attempt);
         PRAGMA user_version=37;"
    ).unwrap();
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    let rows = database.list(10).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].image_usage.is_none());
    assert_eq!(rows[0].total_tokens, Some(5));
    let connection = database.connection.lock().unwrap();
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, LOCAL_DATABASE_SCHEMA_VERSION);
    let count: u32 = connection
        .query_row("SELECT COUNT(*) FROM usage_sends", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
