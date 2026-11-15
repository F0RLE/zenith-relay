use super::*;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
};

#[tokio::test(flavor = "current_thread")]
async fn disabled_performance_collection_validates_without_waiting_for_storage() {
    let root = crate::files::test_temp_root("relay-disabled-metrics");
    let database = Arc::new(TelemetryDb::open(&root.join("usage.sqlite")).unwrap());
    {
        let _connection = database.connection.lock().unwrap();
        let mut write = Box::pin(database.record_performance_async(
            "page_open".into(),
            12.0,
            Some("usage".into()),
            || false,
        ));
        let result = write.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        assert!(matches!(result, Poll::Ready(Ok(()))));
    }
    assert_eq!(
        database
            .record_performance_async("unknown_metric".into(), 1.0, None, || false,)
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidState
    );
    assert_eq!(
        database
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM performance_samples", [], |row| row
                .get::<_, i64>(0),)
            .unwrap(),
        0
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn invalidated_performance_job_does_not_write_after_enqueue() {
    let root = crate::files::test_temp_root("relay-invalidated-metrics");
    let database = Arc::new(TelemetryDb::open(&root.join("usage.sqlite")).unwrap());
    let observations = Arc::new(AtomicUsize::new(0));
    let gate = observations.clone();
    database
        .record_performance_async("page_open".into(), 1.0, None, move || {
            gate.fetch_add(1, Ordering::AcqRel) == 0
        })
        .await
        .unwrap();
    assert_eq!(observations.load(Ordering::Acquire), 2);
    assert_eq!(
        database
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM performance_samples", [], |row| row
                .get::<_, i64>(0),)
            .unwrap(),
        0
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
