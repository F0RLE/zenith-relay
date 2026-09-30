use super::*;

#[test]
fn usage_survives_database_reopen() {
    let root = std::env::temp_dir().join(format!("zenith-relay-usage-{}", uuid::Uuid::new_v4()));
    let path = root.join("usage.sqlite");
    let event = UsageEvent {
        request_id: "req_1".into(),
        attempt: 1,
        local_key_id: "key_1".into(),
        source_id: "source_1".into(),
        candidate_id: Some("account_1".into()),
        account_id: Some("account_1".into()),
        account_token_generation: None,
        client_context_id: Some("client_0123456789ab".into()),
        routing: Some(RoutingDiagnostics {
            reason: SelectionReason::QuotaHeadroom,
            eligible_candidates: 4,
            quota_remaining_basis_points: Some(6_300),
            in_flight_before: 0,
            dispatches_before: 3,
            endpoint_kind: None,
        }),
        requested_model: Some("gpt-5.4".into()),
        resolved_model: Some("gpt-5.4".into()),
        requested_reasoning_effort: Some("max".into()),
        effective_reasoning_effort: Some("low".into()),
        wire_api: WireApi::Responses,
        service_tier: DefaultServiceTier::Fast,
        applied_service_tier: Some("flex".into()),
        success: true,
        http_status: 200,
        error_category: None,
        tool_use: ToolUseDiagnostics {
            client_tool_count: 73,
            forwarded_tool_count: 73,
            tool_choice: ToolChoiceMode::Auto,
            tool_call_count: 1,
            text_output: false,
            terminal_output: TerminalOutputKind::ToolCall,
            client_schema_bytes: Some(12345),
            forwarded_schema_bytes: Some(12345),
            filtered_tool_count: 0,
            policy_mode: Some(zenith_relay_core::ToolPolicyMode::Automatic),
            policy_outcome: Some(zenith_relay_core::ToolPolicyOutcome::Deferred),
            policy_fallback: false,
            deferred_tool_search: true,
        },
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: Some(0),
        latency_ms: 12,
        ttft_ms: Some(4),
        generation_ms: Some(8),
        input_tokens: Some(2),
        cached_input_tokens: Some(1),
        cache_write_input_tokens: Some(1),
        cache_write_ttl: None,
        reasoning_tokens: Some(2),
        output_tokens: Some(3),
        total_tokens: Some(5),
        upstream_error: None,
        quota_snapshot: None,
    };
    TelemetryDb::open(&path).unwrap().record(&event).unwrap();
    let database = TelemetryDb::open(&path).unwrap();
    let logs = database.list(10).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].tool_use.as_ref(), Some(&event.tool_use));
    assert!(logs[0].created_at.ends_with('Z'));
    assert_eq!(logs[0].candidate_id.as_deref(), Some("account_1"));
    assert_eq!(
        logs[0].client_context_id.as_deref(),
        Some("client_0123456789ab")
    );
    assert_eq!(logs[0].ttft_ms, Some(4));
    assert_eq!(logs[0].cached_input_tokens, Some(1));
    assert_eq!(logs[0].cache_write_input_tokens, Some(1));
    assert_eq!(logs[0].reasoning_tokens, Some(2));
    assert_eq!(logs[0].requested_reasoning_effort.as_deref(), Some("max"));
    assert_eq!(logs[0].effective_reasoning_effort.as_deref(), Some("low"));
    assert_eq!(
        logs[0]
            .tool_use
            .as_ref()
            .map(|tool_use| tool_use.tool_call_count),
        Some(1)
    );
    assert_eq!(logs[0].service_tier, DefaultServiceTier::Fast);
    assert_eq!(logs[0].applied_service_tier, Some("flex".into()));
    assert_eq!(
        logs[0].routing.as_ref().map(|routing| routing.reason),
        Some(SelectionReason::QuotaHeadroom)
    );
    let page = database.usage_page(&UsageQuery::default()).unwrap();
    // The event carries a measured value and the totals are that same value
    // merged once, so the relation holds regardless of catalog prices.
    assert!(page.events[0].api_equivalent.micro_usd > 0);
    assert_eq!(page.totals.api_equivalent, page.events[0].api_equivalent);
    let default_page = database
        .usage_page(&UsageQuery {
            page: 0,
            page_size: 0,
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(default_page.page, 1);
    assert_eq!(default_page.page_size, 50);
    assert_eq!(default_page.total_pages, 1);
    let cached = database.api_equivalents().unwrap();
    assert_eq!(
        database.api_equivalents().unwrap().accounts,
        cached.accounts
    );
    let mut second = event;
    second.request_id = "req_2".into();
    second.input_tokens = Some(20);
    second.total_tokens = Some(23);
    database.record(&second).unwrap();
    assert!(
        database.api_equivalents().unwrap().accounts["account_1"].micro_usd
            > cached.accounts["account_1"].micro_usd
    );
    database
        .record_performance("first_frame", 12.5, Some("startup"))
        .unwrap();
    assert!(database
        .record_performance("unknown_metric", 1.0, None)
        .is_err());
    let performance_samples: i64 = database
        .connection
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM performance_samples", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(performance_samples, 1);
    database.clear().unwrap();
    assert!(database.list(10).unwrap().is_empty());
    assert_eq!(logs[0].total_tokens, Some(5));
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_aggregate_replaces_only_the_accepted_attempt() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-usage-aggregate-{}",
        uuid::Uuid::new_v4()
    ));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    database
        .record(&aggregate_test_event(
            "request-aggregate",
            1,
            10,
            5,
            Some("5m".to_string()),
            4,
        ))
        .unwrap();
    database
        .record(&aggregate_test_event(
            "request-aggregate",
            2,
            20,
            7,
            Some("1h".to_string()),
            8,
        ))
        .unwrap();
    database
        .record(&aggregate_test_event(
            "request-aggregate",
            1,
            90,
            11,
            None,
            9,
        ))
        .unwrap();

    let row = database
        .connection
        .lock()
        .unwrap()
        .query_row(
            "SELECT input_tokens, input_samples, cache_write_input_tokens,
                    cache_write_input_samples, cache_write_5m_tokens, cache_write_1h_tokens,
                    unknown_cache_write_tokens, output_tokens, output_samples,
                    total_tokens, total_samples
                 FROM usage_candidate_rollups
                 WHERE candidate_kind = 'source' AND candidate_id = 'source' AND model = 'model'",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row, (20, 1, 7, 1, 0, 7, 0, 8, 1, 28, 1));

    database
        .record(&aggregate_test_event(
            "request-aggregate",
            2,
            30,
            9,
            None,
            12,
        ))
        .unwrap();
    let row: (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) = database
        .connection
        .lock()
        .unwrap()
        .query_row(
            "SELECT input_tokens, input_samples, cache_write_input_tokens,
                    cache_write_input_samples, cache_write_5m_tokens, cache_write_1h_tokens,
                    unknown_cache_write_tokens, output_tokens, output_samples,
                    total_tokens, total_samples
                 FROM usage_candidate_rollups
                 WHERE candidate_kind = 'source' AND candidate_id = 'source' AND model = 'model'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row, (30, 1, 9, 1, 0, 0, 9, 12, 1, 42, 1));
    assert_eq!(database.list(10).unwrap().len(), 1);
    database.clear().unwrap();
    assert!(
        database
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM usage_candidate_rollups", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
            == 0
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_totals_cache_tracks_new_and_replaced_events() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-usage-totals-cache-{}",
        uuid::Uuid::new_v4()
    ));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut first = aggregate_test_event("cached-first", 1, 10, 0, None, 4);
    first.latency_ms = 100;
    first.ttft_ms = Some(20);
    first.generation_ms = Some(60);
    first.reasoning_tokens = Some(1);
    first.total_tokens = Some(14);
    database.record(&first).unwrap();

    let initial = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(initial.totals.requests, 1);
    assert!(database.usage_totals_cache.lock().unwrap().is_some());

    let mut second = aggregate_test_event("cached-second", 1, 20, 0, None, 8);
    second.latency_ms = 200;
    second.ttft_ms = Some(30);
    second.generation_ms = Some(100);
    second.reasoning_tokens = Some(2);
    second.total_tokens = Some(28);
    database.record(&second).unwrap();

    let appended = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(appended.totals.requests, 2);
    assert_eq!(appended.totals.successful_requests, 2);
    assert_eq!(appended.totals.latency_ms, 300);
    assert_eq!(appended.totals.ttft_ms, 50);
    assert_eq!(appended.totals.ttft_samples, 2);
    assert_eq!(appended.totals.generation_ms, 160);
    assert_eq!(appended.totals.generation_samples, 2);
    assert_eq!(appended.totals.generation_output_tokens, 7);
    assert_eq!(appended.totals.input_tokens, 30);
    assert_eq!(appended.totals.reasoning_tokens, 3);
    assert_eq!(appended.totals.output_tokens, 12);
    assert_eq!(appended.totals.total_tokens, 42);
    assert_eq!(appended.totals.speed_output_tokens, 12);
    assert_eq!(appended.totals.speed_duration_ms, 300);

    second.attempt = 2;
    second.success = false;
    second.http_status = 503;
    second.error_category = Some("upstream_unavailable".into());
    second.latency_ms = 400;
    second.ttft_ms = None;
    second.generation_ms = Some(500);
    second.input_tokens = Some(40);
    second.reasoning_tokens = Some(3);
    second.output_tokens = Some(12);
    second.total_tokens = Some(52);
    database.record(&second).unwrap();

    let replaced = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(replaced.totals.requests, 2);
    assert_eq!(replaced.totals.successful_requests, 1);
    assert_eq!(replaced.totals.latency_ms, 500);
    assert_eq!(replaced.totals.ttft_ms, 20);
    assert_eq!(replaced.totals.ttft_samples, 1);
    assert_eq!(replaced.totals.generation_ms, 60);
    assert_eq!(replaced.totals.generation_samples, 1);
    assert_eq!(replaced.totals.generation_output_tokens, 2);
    assert_eq!(replaced.totals.input_tokens, 50);
    assert_eq!(replaced.totals.reasoning_tokens, 4);
    assert_eq!(replaced.totals.output_tokens, 16);
    assert_eq!(replaced.totals.total_tokens, 66);
    assert_eq!(replaced.totals.speed_output_tokens, 4);
    assert_eq!(replaced.totals.speed_duration_ms, 100);

    second.attempt = 1;
    second.input_tokens = Some(1_000);
    database.record(&second).unwrap();
    assert_eq!(
        database.usage_page(&UsageQuery::default()).unwrap().totals,
        replaced.totals
    );

    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_v26_migration_merges_current_logs_into_existing_rollups() {
    let root =
        std::env::temp_dir().join(format!("zenith-relay-usage-v26-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("usage.sqlite");
    let connection = Connection::open(&path).unwrap();
    for migration in [
        MIGRATION_001,
        MIGRATION_002,
        MIGRATION_003,
        MIGRATION_004,
        MIGRATION_005,
        MIGRATION_006,
        MIGRATION_007,
        MIGRATION_008,
        MIGRATION_009,
        MIGRATION_010,
        MIGRATION_011,
        MIGRATION_012,
        MIGRATION_013,
        MIGRATION_014,
        MIGRATION_015,
        MIGRATION_016,
        MIGRATION_017,
        MIGRATION_018,
        MIGRATION_019,
        MIGRATION_020,
        MIGRATION_021,
        MIGRATION_022,
        MIGRATION_023,
        MIGRATION_024,
        MIGRATION_025,
    ] {
        connection.execute_batch(migration).unwrap();
    }
    connection
        .execute(
            "INSERT INTO usage_candidate_rollups(
                    candidate_kind, candidate_id, model,
                    input_tokens, input_samples, cache_write_input_tokens,
                    cache_write_input_samples, output_tokens, output_samples,
                    total_tokens, total_samples
                 ) VALUES ('source', 'source', 'model', 2, 1, 3, 1, 4, 1, 5, 1)",
            [],
        )
        .unwrap();
    connection
            .execute(
                "INSERT INTO request_logs(
                    request_id, attempt, local_key_id, source_id, candidate_id, account_id,
                    requested_model, resolved_model, wire_api, success, http_status, latency_ms,
                    input_tokens, cache_write_input_tokens, cache_write_ttl, output_tokens, total_tokens
                 ) VALUES ('request-current', 1, 'key', 'source', 'source', NULL,
                    'model', 'model', 'responses', 1, 200, 1, 5, 7, '5m', 8, 13)",
                [],
            )
            .unwrap();
    drop(connection);

    let database = TelemetryDb::open(&path).unwrap();
    let row: (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64) = database
        .connection
        .lock()
        .unwrap()
        .query_row(
            "SELECT input_tokens, input_samples, cache_write_input_tokens,
                    cache_write_input_samples, cache_write_5m_tokens, cache_write_1h_tokens,
                    unknown_cache_write_tokens, output_tokens, output_samples,
                    total_tokens, total_samples
                 FROM usage_candidate_rollups
                 WHERE candidate_kind = 'source' AND candidate_id = 'source' AND model = 'model'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row, (7, 2, 10, 2, 7, 0, 3, 12, 2, 18, 2));
    assert_eq!(database.list(10).unwrap().len(), 1);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_keeps_only_the_terminal_fallback_attempt() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-usage-attempts-{}",
        uuid::Uuid::new_v4()
    ));
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    let mut event = failed_fallback_test_event("req_fallback");
    event.upstream_error = Some(zenith_relay_core::usage::UpstreamErrorDetails::from_body(
        Some(503),
        br#"{"error":{"code":"future_capacity","message":"Capacity temporarily exhausted"}}"#,
    ));
    event.requested_reasoning_effort = Some("max".into());
    event.effective_reasoning_effort = Some("max".into());
    database.record(&event).unwrap();
    assert_eq!(
        database.list(10).unwrap()[0].upstream_error,
        event.upstream_error
    );
    event.attempt = 2;
    event.source_id = "source_2".into();
    event.candidate_id = Some("source_2".into());
    event.success = true;
    event.http_status = 200;
    event.error_category = None;
    event.effective_reasoning_effort = Some("low".into());
    event.cooldown_scope = None;
    event.retry_at_ms = None;
    event.consecutive_failures = Some(0);
    database.record(&event).unwrap();

    event.attempt = 1;
    event.source_id = "source_stale".into();
    event.success = false;
    event.http_status = 503;
    event.error_category = Some("upstream_unavailable".into());
    database.record(&event).unwrap();

    let logs = database.list(10).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].attempt, 2);
    assert!(logs[0].success);
    assert!(logs[0].upstream_error.is_none());
    assert_eq!(logs[0].source_id, "source_2");
    assert_eq!(logs[0].requested_reasoning_effort.as_deref(), Some("max"));
    assert_eq!(logs[0].effective_reasoning_effort.as_deref(), Some("low"));
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_keeps_only_the_last_failure_when_all_attempts_fail() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-usage-failed-attempts-{}",
        uuid::Uuid::new_v4()
    ));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut event = failed_fallback_test_event("req_failed");
    event.upstream_error = Some(zenith_relay_core::usage::UpstreamErrorDetails::from_body(
        Some(503),
        br#"{"error":{"code":"future_capacity","message":"Capacity temporarily exhausted"}}"#,
    ));
    event.upstream_error.as_mut().unwrap().message =
        Some("Capacity exhausted; Bearer synthetic-private".into());
    database.record(&event).unwrap();
    let stored: String = database
        .connection
        .lock()
        .unwrap()
        .query_row("SELECT upstream_error_json FROM request_logs", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(!stored.contains("synthetic-private"));
    event.attempt = 2;
    event.source_id = "source_2".into();
    event.candidate_id = Some("source_2".into());
    event.http_status = 429;
    event.error_category = Some("upstream_rate_limited".into());
    database.record(&event).unwrap();

    let logs = database.list(10).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].attempt, 2);
    assert!(!logs[0].success);
    assert_eq!(
        logs[0].upstream_error,
        event
            .upstream_error
            .as_ref()
            .map(|details| details.sanitized())
    );
    assert_eq!(logs[0].http_status, 429);
    assert_eq!(logs[0].source_id, "source_2");
    assert_eq!(logs[0].error_origin, Some(ErrorOrigin::Provider));
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_v1_migrates_existing_rows_to_attempt_one() {
    let root = std::env::temp_dir().join(format!("zenith-relay-usage-v1-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("usage.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch(MIGRATION_001).unwrap();
    connection
        .execute(
            "INSERT INTO request_logs (
                    request_id, local_key_id, source_id, wire_api, success, http_status, latency_ms
                ) VALUES ('req_old', 'key_1', 'source_1', 'responses', 1, 200, 3)",
            [],
        )
        .unwrap();
    drop(connection);

    let database = TelemetryDb::open(&path).unwrap();
    let logs = database.list(10).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].attempt, 1);
    assert_eq!(logs[0].requested_reasoning_effort, None);
    assert_eq!(logs[0].effective_reasoning_effort, None);
    let version: u32 = database
        .connection
        .lock()
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, LOCAL_DATABASE_SCHEMA_VERSION);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_v14_migration_keeps_the_latest_attempt_per_request() {
    let root =
        std::env::temp_dir().join(format!("zenith-relay-usage-v14-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("usage.sqlite");
    let connection = Connection::open(&path).unwrap();
    for migration in [
        MIGRATION_001,
        MIGRATION_002,
        MIGRATION_003,
        MIGRATION_004,
        MIGRATION_005,
        MIGRATION_006,
        MIGRATION_007,
        MIGRATION_008,
        MIGRATION_009,
        MIGRATION_010,
        MIGRATION_011,
        MIGRATION_012,
        MIGRATION_013,
        MIGRATION_014,
    ] {
        connection.execute_batch(migration).unwrap();
    }
    connection
        .execute(
            "INSERT INTO request_logs (
                    request_id, attempt, local_key_id, source_id, wire_api, success,
                    http_status, latency_ms
                ) VALUES ('req_duplicate', 1, 'key', 'source_1', 'responses', 0, 503, 1)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO request_logs (
                    request_id, attempt, local_key_id, source_id, wire_api, success,
                    http_status, latency_ms
                ) VALUES ('req_duplicate', 2, 'key', 'source_2', 'responses', 1, 200, 2)",
            [],
        )
        .unwrap();
    drop(connection);

    let database = TelemetryDb::open(&path).unwrap();
    let logs = database.list(10).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].attempt, 2);
    assert!(logs[0].success);
    assert_eq!(logs[0].source_id, "source_2");
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
