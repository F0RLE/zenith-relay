use super::*;
use serde_json::json;

fn with_database(name: &str, check: impl FnOnce(&TelemetryDb)) {
    let root = crate::files::test_temp_root(name);
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    check(&database);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

fn event(request: &str) -> UsageEvent {
    let mut event = aggregate_test_event(request, 1, 1000, 0, None, 20);
    event.routing = Some(
        serde_json::from_value(json!({
            "reason":"fair_rotation", "eligibleCandidates":1,
            "quotaRemainingBasisPoints":9000, "inFlightBefore":0, "dispatchesBefore":0,
            "endpointKind":"responses", "attemptOutcome":"completed",
            "requestTiming":{"durationMs":10,"ttftMs":2,"projectionOnly":false}
        }))
        .unwrap(),
    );
    event.tool_use.client_tool_count = 2;
    event.tool_use.client_schema_bytes = Some(100);
    event.tool_use.client_context = Some(zenith_relay_core::usage::RequestContextSize {
        body_bytes: 120,
        conversation_bytes: 20,
        tool_catalog_bytes: 100,
        ..Default::default()
    });
    event
}

fn dictionary_count(database: &TelemetryDb) -> i64 {
    database
        .connection
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM usage_diagnostics", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn diagnostics_share_immutable_snapshots_and_collect_unreferenced_corrections() {
    with_database("relay-diagnostic-dedup", |database| {
        let mut observation = event("synthetic");
        database.record(&observation).unwrap();
        assert_eq!(dictionary_count(database), 2);
        {
            let connection = database.connection.lock().unwrap();
            assert_eq!(
                connection
                    .query_row("SELECT min(refs) FROM usage_diagnostics", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                2
            );
            assert!(
                connection
                    .query_row(
                        "SELECT routing_json IS NULL AND tool_use_json IS NULL FROM request_logs",
                        [],
                        |r| r.get::<_, bool>(0)
                    )
                    .unwrap()
            );
        }
        let before = serde_json::to_value(database.list(10).unwrap()).unwrap();
        database.record(&observation).unwrap();
        assert_eq!(
            serde_json::to_value(database.list(10).unwrap()).unwrap(),
            before
        );
        observation.tool_use.client_schema_bytes = Some(200);
        database.record(&observation).unwrap();
        assert_eq!(dictionary_count(database), 2);
        assert_eq!(
            database.list(10).unwrap()[0]
                .tool_use
                .as_ref()
                .unwrap()
                .client_schema_bytes,
            Some(200)
        );
        database.clear().unwrap();
        assert_eq!(dictionary_count(database), 0);
    });
}

#[test]
fn seven_day_retention_preserves_usage_and_a_recent_shared_attempt() {
    with_database("relay-diagnostic-expiry", |database| {
        database.record(&event("synthetic")).unwrap();
        let before = database.usage_page(&UsageQuery::default()).unwrap().totals;
        database
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE request_logs SET created_at = datetime('now', '-8 days')",
                [],
            )
            .unwrap();
        assert_eq!(database.maintain_usage().unwrap(), 1);
        let old = database.list(10).unwrap();
        let tools = old[0].tool_use.as_ref().unwrap();
        assert_eq!(tools.client_tool_count, 2);
        assert!(tools.client_context.is_none());
        assert!(tools.client_schema_bytes.is_none());
        assert_eq!(
            old[0].routing.as_ref().unwrap().endpoint_kind.as_deref(),
            Some("responses")
        );
        let recent = database.usage_page(&UsageQuery::default()).unwrap();
        assert!(
            recent.events[0]
                .tool_use
                .as_ref()
                .unwrap()
                .client_context
                .is_some()
        );
        assert_eq!(
            serde_json::to_value(recent.totals).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        database
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE request_attempts SET created_at = datetime('now', '-8 days')",
                [],
            )
            .unwrap();
        assert_eq!(database.maintain_usage().unwrap(), 1);
        assert_eq!(database.maintain_usage().unwrap(), 0);
        assert_eq!(dictionary_count(database), 2);
        assert!(
            database.usage_page(&UsageQuery::default()).unwrap().events[0]
                .tool_use
                .as_ref()
                .unwrap()
                .client_context
                .is_none()
        );
    });
}

#[test]
fn normalized_projection_keeps_logical_completion_separate_from_late_send_usage() {
    with_database("relay-diagnostic-completion", |database| {
        let mut send = event("synthetic");
        database.record(&send).unwrap();
        let mut completion = send.clone();
        completion.success = false;
        completion.http_status = 499;
        completion.error_category = Some("client_cancelled".into());
        let routing = completion.routing.as_mut().unwrap();
        routing.attempt_outcome = Some(zenith_relay_core::AttemptOutcome::ClientDisconnected);
        routing.request_timing.as_mut().unwrap().projection_only = true;
        database.record(&completion).unwrap();
        let expected = serde_json::to_value(database.list(10).unwrap()).unwrap();
        send.input_tokens = Some(2000);
        send.tool_use.client_schema_bytes = Some(300);
        database.record(&send).unwrap();
        assert_eq!(
            serde_json::to_value(database.list(10).unwrap()).unwrap(),
            expected
        );
        let sends = database.usage_page(&UsageQuery::default()).unwrap();
        assert_eq!(sends.events[0].input_tokens, Some(2000));
        assert_eq!(
            sends.events[0]
                .tool_use
                .as_ref()
                .unwrap()
                .client_schema_bytes,
            Some(300)
        );
        assert_eq!(sends.totals.requests, 1);
        assert!(database.request_diagnostics("synthetic").is_ok());
    });
}

#[test]
fn inline_compaction_is_bounded_and_rolls_back_with_its_dictionary() {
    with_database("relay-diagnostic-batch", |database| {
        let observation = event("synthetic");
        let routing = serde_json::to_string(observation.routing.as_ref().unwrap()).unwrap();
        let tools = serde_json::to_string(&observation.tool_use).unwrap();
        {
            let connection = database.connection.lock().unwrap();
            connection.execute_batch(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<300)
                 INSERT INTO request_logs(request_id, local_key_id, source_id, wire_api, success, http_status, latency_ms)
                 SELECT 'synthetic-'||i, 'key', 'source', 'responses', 1, 200, 10 FROM n;",
            ).unwrap();
            connection
                .execute(
                    "UPDATE request_logs SET routing_json = ?1, tool_use_json = ?2",
                    params![routing, tools],
                )
                .unwrap();
            connection.execute_batch(
                "CREATE TEMP TRIGGER reject_compaction BEFORE UPDATE OF routing_diagnostics_id ON request_logs
                 BEGIN SELECT RAISE(ABORT, 'synthetic compaction failure'); END;",
            ).unwrap();
        }
        assert!(database.maintain_usage().is_err());
        assert_eq!(dictionary_count(database), 0);
        database
            .connection
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_compaction")
            .unwrap();
        assert_eq!(database.maintain_usage().unwrap(), 256);
        assert_eq!(database.maintain_usage().unwrap(), 44);
        assert_eq!(database.maintain_usage().unwrap(), 0);
        assert_eq!(dictionary_count(database), 2);
        assert_eq!(database.list(500).unwrap().len(), 300);
    });
}

#[test]
fn diagnostic_schema_upgrades_v45_without_rewriting_history() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(
        "CREATE TABLE request_logs(id INTEGER PRIMARY KEY, created_at TEXT, routing_json TEXT, tool_use_json TEXT);
         CREATE TABLE request_attempts AS SELECT * FROM request_logs;
         INSERT INTO request_logs VALUES(1, CURRENT_TIMESTAMP, '{\"endpointKind\":\"responses\"}', NULL);
         PRAGMA user_version=45;",
    ).unwrap();
    migrate_046(&mut connection).unwrap();
    migrate_046(&mut connection).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        LOCAL_DATABASE_SCHEMA_VERSION
    );
    assert_eq!(
        connection
            .query_row("SELECT routing_json FROM request_logs", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "{\"endpointKind\":\"responses\"}"
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM usage_diagnostics", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
