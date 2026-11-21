use super::{performance_samples::with_database, *};
use std::time::Duration;

fn assert_ten_minute_deadline(database: &TelemetryDb) {
    let delay = database.next_maintenance_delay().unwrap();
    assert!(delay >= Duration::from_secs(9 * 60), "{delay:?}");
    assert!(delay <= Duration::from_secs(11 * 60), "{delay:?}");
}

#[test]
fn quiet_database_uses_bounded_idle_delay_and_new_usage_changes_revision() {
    with_database(|database| {
        assert_eq!(
            database.next_maintenance_delay().unwrap(),
            Duration::from_secs(3600)
        );
        let revision = database.maintenance_revision();
        database
            .record(&aggregate_test_event(
                "synthetic-schedule",
                1,
                100,
                0,
                None,
                20,
            ))
            .unwrap();
        assert_ne!(database.maintenance_revision(), revision);
        assert_eq!(
            database.next_maintenance_delay().unwrap(),
            Duration::from_secs(3600)
        );
        let locked = database.connection.lock().unwrap();
        assert_eq!(
            database.next_maintenance_delay().unwrap(),
            Duration::from_secs(60)
        );
        drop(locked);
    });
}

#[test]
fn retention_deadlines_include_usage_diagnostics_and_legacy_inline_work() {
    with_database(|database| {
        let mut event = aggregate_test_event("synthetic-deadline", 1, 100, 0, None, 20);
        event.routing = Some(
            serde_json::from_value(serde_json::json!({
                "reason": "fair_rotation", "eligibleCandidates": 1,
                "quotaRemainingBasisPoints": null, "inFlightBefore": 0, "dispatchesBefore": 0,
            }))
            .unwrap(),
        );
        database.record(&event).unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute_batch(
                "UPDATE request_logs SET created_at = datetime('now', '-7 days', '+10 minutes');
             UPDATE request_attempts SET created_at = datetime('now', '-7 days', '+10 minutes');",
            )
            .unwrap();
        assert_ten_minute_deadline(database);
        database.connection.lock().unwrap().execute_batch(
            "UPDATE request_logs SET diagnostics_pruned = 1, created_at = datetime('now', '-30 days', '+10 minutes');
             UPDATE request_attempts SET diagnostics_pruned = 1;"
        ).unwrap();
        assert_ten_minute_deadline(database);
        database
            .connection
            .lock()
            .unwrap()
            .execute_batch("UPDATE request_attempts SET routing_json = '{}';")
            .unwrap();
        assert_eq!(
            database.next_maintenance_delay().unwrap(),
            Duration::from_secs(60)
        );
    });
}

#[test]
fn capacity_deadlines_expire_without_new_usage() {
    for (table, days) in [
        ("weekly_capacity_history", 30),
        ("weekly_capacity_anchors", 3),
    ] {
        with_database(|database| {
            let due = i64::try_from(zenith_relay_core::unix_time_ms()).unwrap() - days * 86_400_000
                + 600_000;
            database.connection.lock().unwrap().execute(
                &format!("INSERT INTO {table}(account_id, observed_at_ms, payload_json) VALUES ('synthetic', ?1, '{{}}')"),
                [due],
            ).unwrap();
            assert_ten_minute_deadline(database);
            database
                .connection
                .lock()
                .unwrap()
                .execute(
                    &format!("UPDATE {table} SET observed_at_ms = ?1"),
                    [due - 1_200_000],
                )
                .unwrap();
            assert_eq!(
                database.next_maintenance_delay().unwrap(),
                Duration::from_secs(60)
            );
            database.maintain_usage().unwrap();
            assert_eq!(
                database.next_maintenance_delay().unwrap(),
                Duration::from_secs(3600)
            );
        });
    }
}
