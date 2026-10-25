use super::*;
use std::{
    sync::{mpsc, Arc},
    time::Duration,
};

#[test]
fn report_snapshot_does_not_block_recording_and_keeps_one_revision() {
    let root = std::env::temp_dir().join(format!("relay-reader-{}", uuid::Uuid::new_v4()));
    let database = Arc::new(TelemetryDb::open(&root.join("usage.sqlite")).unwrap());
    database
        .record(&aggregate_test_event("before", 1, 10, 0, None, 1))
        .unwrap();
    // Seed the incrementally maintained all-time totals too.
    assert_eq!(
        database
            .usage_page(&UsageQuery::default())
            .unwrap()
            .totals
            .requests,
        1
    );
    let read = database.lock_report_connection().unwrap();
    let revision = read.revision;
    let (sent, received) = mpsc::channel();
    let writer = database.clone();
    let task = std::thread::spawn(move || {
        sent.send(writer.record(&aggregate_test_event("during", 1, 20, 0, None, 2)))
            .unwrap();
    });
    let written = received.recv_timeout(Duration::from_secs(5));
    let count: i64 = read
        .query_row("SELECT COUNT(*) FROM request_logs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(read.totals.as_ref().unwrap().requests, 1);
    // Releasing the read even on timeout allows the worker to finish cleanly.
    drop(read);
    task.join().unwrap();
    written
        .expect("recording waited for the report snapshot")
        .unwrap();
    let next = database.lock_report_connection().unwrap();
    assert!(next.revision > revision);
    assert_eq!(next.totals.as_ref().unwrap().requests, 2);
    assert_eq!(
        next.query_row("SELECT COUNT(*) FROM request_logs", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert!(next.execute("DELETE FROM request_logs", []).is_err());
    drop(next);
    assert_eq!(database.memory_cache_budget(), Some(32 * 1024 * 1024));
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_report_releases_read_transaction() {
    let root = std::env::temp_dir().join(format!("relay-reader-error-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    {
        let read = database.lock_report_connection().unwrap();
        assert!(read
            .prepare("SELECT * FROM synthetic_missing_table")
            .is_err());
    }
    database
        .record(&aggregate_test_event("after-error", 1, 10, 0, None, 1))
        .unwrap();
    assert_eq!(
        database
            .usage_page(&UsageQuery::default())
            .unwrap()
            .totals
            .requests,
        1
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cold_report_does_not_publish_totals_after_a_concurrent_write() {
    let root = std::env::temp_dir().join(format!("relay-reader-cold-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    database
        .record(&aggregate_test_event("before", 1, 10, 0, None, 1))
        .unwrap();
    let read = database.lock_report_connection().unwrap();
    assert!(read.totals.is_none());
    database
        .record(&aggregate_test_event("during", 1, 20, 0, None, 2))
        .unwrap();
    let totals = database
        .cached_usage_totals(&read, &UsageQuery::default(), "", &[])
        .unwrap();
    assert_eq!(totals.requests, 1);
    assert!(database.usage_totals_cache.lock().unwrap().is_none());
    drop(read);
    assert_eq!(
        database
            .usage_page(&UsageQuery::default())
            .unwrap()
            .totals
            .requests,
        2
    );
    assert_eq!(
        database
            .usage_totals_cache
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .requests,
        2
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
