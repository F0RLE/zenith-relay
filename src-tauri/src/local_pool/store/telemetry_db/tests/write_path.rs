use super::*;

#[test]
fn unchanged_consumption_updates_history_without_rewriting_rollups() {
    let root = std::env::temp_dir().join(format!("relay-noop-rollup-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut event = aggregate_test_event("synthetic-noop", 1, 100, 0, None, 10);
    database.record(&event).unwrap();
    let before = database.connection.lock().unwrap().total_changes();
    event.latency_ms = 200;
    database.record(&event).unwrap();
    assert_eq!(
        database.connection.lock().unwrap().total_changes(),
        before + 2
    );
    let page = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(page.totals.total_tokens, 110);
    assert_eq!(page.totals.latency_ms, 200);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_rollup_update_rolls_back_history_and_cached_totals() {
    let root = std::env::temp_dir().join(format!("relay-rollup-rollback-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut event = aggregate_test_event("synthetic-rollback", 1, 100, 0, None, 10);
    database.record(&event).unwrap();
    let initial = database.usage_page(&UsageQuery::default()).unwrap().totals;
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_rollup BEFORE INSERT ON usage_candidate_rollups
         BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        )
        .unwrap();
    event.input_tokens = Some(200);
    event.total_tokens = Some(210);
    assert!(database.record(&event).is_err());
    let after = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(after.events[0].total_tokens, Some(110));
    assert_eq!(
        serde_json::to_value(after.totals).unwrap(),
        serde_json::to_value(initial).unwrap()
    );
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_rollup;")
        .unwrap();
    database.record(&event).unwrap();
    assert_eq!(
        database
            .usage_page(&UsageQuery::default())
            .unwrap()
            .totals
            .total_tokens,
        210
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_history_clear_preserves_all_tables_and_cached_totals() {
    let root = std::env::temp_dir().join(format!("relay-clear-rollback-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    database
        .record(&aggregate_test_event(
            "synthetic-clear",
            1,
            100,
            0,
            None,
            10,
        ))
        .unwrap();
    let initial = database.usage_page(&UsageQuery::default()).unwrap().totals;
    let revision = database.weekly_capacity_revision();
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO weekly_capacity_anchors VALUES ('synthetic', 1, '{}');
             INSERT INTO weekly_capacity_history VALUES ('synthetic', 1, '{}');
             CREATE TEMP TRIGGER fail_clear BEFORE DELETE ON weekly_capacity_history
             BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        )
        .unwrap();
    let counts = || {
        database
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT (SELECT COUNT(*) FROM request_logs),
                        (SELECT COUNT(*) FROM request_attempts),
                        (SELECT COUNT(*) FROM usage_candidate_rollups),
                        (SELECT COUNT(*) FROM weekly_capacity_anchors),
                        (SELECT COUNT(*) FROM weekly_capacity_history)",
                [],
                |row| {
                    Ok([
                        row.get::<_, i64>(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ])
                },
            )
            .unwrap()
    };
    let before = counts();
    assert!(before.iter().all(|count| *count > 0));
    assert!(database.clear().is_err());
    assert_eq!(counts(), before);
    assert_eq!(database.weekly_capacity_revision(), revision);
    assert_eq!(
        serde_json::to_value(database.usage_page(&UsageQuery::default()).unwrap().totals).unwrap(),
        serde_json::to_value(initial).unwrap()
    );
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_clear")
        .unwrap();
    database.clear().unwrap();
    assert_eq!(counts(), [0; 5]);
    assert_eq!(database.weekly_capacity_revision(), revision + 1);
    assert_eq!(
        database.usage_page(&UsageQuery::default()).unwrap().total,
        0
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unchanged_state_skips_updates_and_mixed_batches_keep_last_value() {
    let root = std::env::temp_dir().join(format!("relay-state-writes-{}", uuid::Uuid::new_v4()));
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    let initial = [("stable", "{}".into()), ("changed", "0".into())];
    database.replace_state_json(&initial).unwrap();
    let before = database.connection.lock().unwrap().total_changes();
    let wal_path = path.with_extension("sqlite-wal");
    let wal_bytes = std::fs::metadata(&wal_path).unwrap().len();
    for _ in 0..16 {
        database.replace_state_json(&initial).unwrap();
    }
    assert_eq!(database.connection.lock().unwrap().total_changes(), before);
    assert_eq!(std::fs::metadata(&wal_path).unwrap().len(), wal_bytes);
    database
        .replace_state_json(&[
            ("stable", "{}".into()),
            ("changed", "1".into()),
            ("new", "2".into()),
            ("new", "3".into()),
        ])
        .unwrap();
    assert_eq!(
        database.connection.lock().unwrap().total_changes(),
        before + 3
    );
    drop(database);
    let reopened = TelemetryDb::open(&path).unwrap();
    assert_eq!(
        reopened.state_json("stable").unwrap().as_deref(),
        Some("{}")
    );
    assert_eq!(
        reopened.state_json("changed").unwrap().as_deref(),
        Some("1")
    );
    assert_eq!(reopened.state_json("new").unwrap().as_deref(), Some("3"));
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_state_statement_rolls_back_batch_and_cached_statement_is_reusable() {
    let root = std::env::temp_dir().join(format!("relay-state-rollback-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    database
        .replace_state_json(&[("first", "0".into())])
        .unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_state_write BEFORE INSERT ON app_state
         WHEN NEW.key = 'fail' BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        )
        .unwrap();
    assert!(
        database
            .replace_state_json(&[("first", "1".into()), ("fail", "2".into())])
            .is_err()
    );
    assert_eq!(database.state_json("first").unwrap().as_deref(), Some("0"));
    assert_eq!(database.state_json("fail").unwrap(), None);
    database
        .replace_state_json(&[("first", "3".into())])
        .unwrap();
    assert_eq!(database.state_json("first").unwrap().as_deref(), Some("3"));
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_purge_rolls_back_state_and_attempt_deletion() {
    let root = std::env::temp_dir().join(format!("relay-purge-rollback-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    database
        .replace_state_json(&[("accounts", "[1]".into())])
        .unwrap();
    let mut event = aggregate_test_event("synthetic-purge", 1, 100, 0, None, 10);
    event.account_id = Some("synthetic-account".into());
    database.record(&event).unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_purge BEFORE DELETE ON request_logs
         BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        )
        .unwrap();
    assert!(
        database
            .replace_state_json_and_delete_account_data(
                &[("accounts", "[]".into())],
                "synthetic-account",
            )
            .is_err()
    );
    assert_eq!(
        database.state_json("accounts").unwrap().as_deref(),
        Some("[1]")
    );
    let rows: (i64, i64) = database
        .connection
        .lock()
        .unwrap()
        .query_row(
            "SELECT (SELECT COUNT(*) FROM request_logs), (SELECT COUNT(*) FROM request_attempts)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(rows, (1, 1));
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "synthetic durable write timing; same workload/profile for before-after comparison"]
fn measure_usage_record_writes() {
    let mut events = Vec::new();
    for request in 0..128 {
        let mut first = aggregate_test_event(&format!("synthetic-{request}"), 1, 100, 0, None, 10);
        first.account_id = Some(format!("synthetic-account-{}", request % 16));
        first.resolved_model = Some(format!("synthetic-model-{}", request % 4));
        let mut second = first.clone();
        second.attempt = 2;
        second.input_tokens = Some(200);
        second.output_tokens = Some(20);
        second.total_tokens = Some(220);
        let mut correction = first.clone();
        correction.input_tokens = Some(150);
        correction.total_tokens = Some(160);
        let mut completion = second.clone();
        completion.routing = Some(
            serde_json::from_value(serde_json::json!({
                "reason":"only_eligible", "eligibleCandidates":1,
                "quotaRemainingBasisPoints":null, "inFlightBefore":0, "dispatchesBefore":0,
                "attemptOutcome":"completed",
                "requestTiming":{"durationMs":500,"ttftMs":null,"projectionOnly":true}
            }))
            .unwrap(),
        );
        events.extend([first, second, correction, completion]);
    }
    let mut elapsed_ms = Vec::new();
    for _ in 0..3 {
        let root = std::env::temp_dir().join(format!("relay-write-bench-{}", uuid::Uuid::new_v4()));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        let started = Instant::now();
        for event in &events {
            database.record(event).unwrap();
        }
        elapsed_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        let page = database.usage_page(&UsageQuery::default()).unwrap();
        assert_eq!(
            (page.totals.requests, page.totals.total_tokens),
            (128, 48_640)
        );
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }
    elapsed_ms.sort_by(f64::total_cmp);
    eprintln!(
        "usage_record WAL/FULL: events={}, median_ms={:.3}, samples_ms={elapsed_ms:?}",
        events.len(),
        elapsed_ms[1]
    );
}
