use super::*;

fn pre_dispatch_rejection(request_id: &str) -> UsageEvent {
    let mut event = aggregate_test_event(request_id, 0, 0, 0, None, 0);
    event.source_id = "relay".into();
    event.candidate_id = None;
    event.success = false;
    event.http_status = 400;
    event.error_category = Some("adapter_continuation_mismatch".into());
    event.input_tokens = None;
    event.output_tokens = None;
    event.total_tokens = None;
    event.cache_write_input_tokens = None;
    event
}

#[test]
fn pre_dispatch_rejections_persist_without_provider_sends_or_consumption() {
    let root = std::env::temp_dir().join(format!("relay-local-rejection-{}", uuid::Uuid::new_v4()));
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    for (id, transport) in [
        ("http-rejected", zenith_relay_core::UsageTransport::Http),
        ("ws-rejected", zenith_relay_core::UsageTransport::Websocket),
    ] {
        let mut event = pre_dispatch_rejection(id);
        event.transport = transport;
        database.record(&event).unwrap();
        database.record(&event).unwrap();
        assert_cache_matches_scan(&database);
        let diagnostics = database.request_diagnostics(id).unwrap();
        assert_eq!(diagnostics["availability"]["attemptHistoryMissing"], false);
        assert_eq!(diagnostics["summary"]["includedSends"], 0);
        assert_eq!(
            diagnostics["summary"]["finalSendAddedFromProjection"],
            false
        );
        assert_eq!(
            diagnostics["summary"]["attemptSequenceCompleteThroughLatest"],
            true
        );
        assert_eq!(diagnostics["summary"]["outcome"]["status"], "failed");
        assert!(diagnostics["attempts"].as_array().unwrap().is_empty());
    }
    let page = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(page.totals.requests, 2);
    assert_eq!(page.totals.successful_requests, 0);
    assert_eq!(page.totals.total_tokens, 0);
    assert_eq!(page.totals.cached_input_samples, 0);
    assert_eq!(page.totals.cache_write_input_samples, 0);
    let connection = database.lock_connection().unwrap();
    for table in ["request_attempts", "usage_candidate_rollups"] {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "a local rejection must not invent {table}");
    }
    drop(connection);
    drop(database);

    let database = TelemetryDb::open(&path).unwrap();
    let rows = database.list(10).unwrap();
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row.attempt, 0);
        assert_eq!(row.error_origin, Some(ErrorOrigin::Relay));
        assert_eq!(
            row.error_category.as_deref(),
            Some("adapter_continuation_mismatch")
        );
        assert!(row.candidate_id.is_none());
        assert!(row.input_tokens.is_none());
        assert!(row.output_tokens.is_none());
    }
    assert_cache_matches_scan(&database);
}

#[test]
fn pre_dispatch_records_do_not_accept_provider_usage_or_overwrite_a_send() {
    let root =
        std::env::temp_dir().join(format!("relay-rejection-guards-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let provider = aggregate_test_event("invalid-provider", 0, 10, 0, None, 1);
    assert!(database.record(&provider).is_err());
    let mut invalid_local = pre_dispatch_rejection("invalid-local");
    invalid_local.input_tokens = Some(10);
    assert!(database.record(&invalid_local).is_err());

    let rejection = pre_dispatch_rejection("request");
    database.record(&rejection).unwrap();
    let send = aggregate_test_event("request", 1, 100, 0, None, 10);
    database.record(&send).unwrap();
    database.record(&rejection).unwrap();
    let rows = database.list(10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].attempt, 1);
    assert!(rows[0].success);
    let page = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(page.totals.requests, 1);
    assert_eq!(page.totals.successful_requests, 1);
    assert_eq!(page.totals.total_tokens, 110);
    assert_cache_matches_scan(&database);
}

fn timed_routing(duration_ms: u64, projection_only: bool, outcome: &str) -> RoutingDiagnostics {
    serde_json::from_value(serde_json::json!({
        "reason":"only_eligible", "eligibleCandidates":1,
        "quotaRemainingBasisPoints":null, "inFlightBefore":0, "dispatchesBefore":0,
        "attemptOutcome":outcome,
        "requestTiming":{"durationMs":duration_ms,"ttftMs":null,"projectionOnly":projection_only}
    }))
    .unwrap()
}

#[test]
fn request_finalization_preserves_physical_sends_consumption_and_full_duration() {
    let root = std::env::temp_dir().join(format!("relay-final-usage-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut event = aggregate_test_event("request-final", 1, 100, 0, None, 0);
    event.success = false;
    event.http_status = 429;
    event.error_category = Some("upstream_rate_limited".into());
    event.latency_ms = 5_837;
    event.routing = Some(timed_routing(6_000, false, "retry_selection"));
    database.record(&event).unwrap();
    assert_cache_matches_scan(&database);
    let physical = event.clone();
    event.routing = Some(timed_routing(472_000, true, "returned_failure"));
    database.record(&event).unwrap();
    database.record(&event).unwrap();
    // A delayed physical observation cannot reopen the completed request.
    database.record(&physical).unwrap();
    let connection = database.lock_connection().unwrap();
    let send: (i64, i64, i64) = connection.query_row(
        "SELECT COUNT(*),latency_ms,total_tokens FROM request_attempts WHERE request_id='request-final'",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    assert_eq!(send, (1, 5_837, 100));
    let logical: (i64, String) = connection.query_row(
        "SELECT latency_ms,json_extract(routing_json,'$.attemptOutcome') FROM request_logs WHERE request_id='request-final'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(logical, (472_000, "returned_failure".into()));
    drop(connection);
    let page = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(page.totals.requests, 1);
    assert_eq!(page.totals.total_tokens, 100);
    assert_eq!(page.totals.latency_ms, 472_000);
    assert_cache_matches_scan(&database);
    database.clear().unwrap();
    database.record(&event).unwrap();
    assert_eq!(
        database
            .usage_page(&UsageQuery::default())
            .unwrap()
            .totals
            .requests,
        0
    );
}

fn assert_cache_matches_scan(database: &TelemetryDb) {
    let cached = database.usage_page(&UsageQuery::default()).unwrap();
    let scanned = database
        .usage_page(&UsageQuery {
            from_ms: Some(0),
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(
        serde_json::to_value(cached.totals).unwrap(),
        serde_json::to_value(scanned.totals).unwrap()
    );
}

#[test]
fn successful_logical_timing_keeps_send_timings_and_cached_totals_consistent() {
    let root = std::env::temp_dir().join(format!("relay-timed-usage-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut event = aggregate_test_event("timed-success", 1, 100, 0, None, 10);
    event.latency_ms = 6_000;
    event.ttft_ms = Some(1_000);
    event.routing = Some(timed_routing(14_000, false, "completed"));
    event
        .routing
        .as_mut()
        .unwrap()
        .request_timing
        .as_mut()
        .unwrap()
        .ttft_ms = Some(9_000);
    database.record(&event).unwrap();
    let page = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(
        (page.totals.latency_ms, page.totals.ttft_ms),
        (14_000, 9_000)
    );
    assert_eq!(
        (page.events[0].latency_ms, page.events[0].ttft_ms),
        (6_000, Some(1_000))
    );
    assert_cache_matches_scan(&database);
}

#[test]
fn every_send_contributes_once_but_only_final_outcome_counts_as_request() {
    let root = std::env::temp_dir().join(format!("relay-send-usage-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut first = aggregate_test_event("request", 1, 100, 5, Some("5m".into()), 10);
    first.success = false;
    first.source_id = "lookup-source".into();
    first.resolved_model = Some("lookup-model".into());
    first.cached_input_tokens = Some(40);
    first.latency_ms = 3_000;
    database.record(&first).unwrap();
    assert_cache_matches_scan(&database);
    let mut last = aggregate_test_event("request", 2, 200, 7, Some("1h".into()), 20);
    last.source_id = "answer-source".into();
    last.latency_ms = 8_000;
    last.ttft_ms = Some(7_000);
    database.record(&last).unwrap();
    let page = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(
        (
            page.total,
            page.totals.requests,
            page.totals.successful_requests
        ),
        (2, 1, 1)
    );
    assert_eq!(
        (
            page.totals.input_tokens,
            page.totals.output_tokens,
            page.totals.total_tokens
        ),
        (300, 30, 330)
    );
    assert_eq!(
        (page.totals.latency_ms, page.totals.ttft_ms),
        (8_000, 7_000)
    );
    assert_eq!(
        page.models
            .iter()
            .map(|group| group.totals.total_tokens)
            .sum::<u64>(),
        330
    );
    assert_eq!(
        page.pool_members
            .iter()
            .find(|group| group.key == "lookup-source")
            .unwrap()
            .totals
            .total_tokens,
        110
    );
    assert_eq!(
        page.pool_members
            .iter()
            .find(|group| group.key == "lookup-source")
            .unwrap()
            .totals
            .requests,
        0
    );
    assert_cache_matches_scan(&database);

    // A late correction changes consumption, not the successful logical result.
    first.input_tokens = Some(120);
    first.total_tokens = Some(130);
    database.record(&first).unwrap();
    database.record(&first).unwrap();
    database.record(&last).unwrap();
    let page = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(
        (
            page.total,
            page.totals.requests,
            page.totals.successful_requests
        ),
        (2, 1, 1)
    );
    assert_eq!(page.totals.total_tokens, 350);
    assert_cache_matches_scan(&database);
    let filtered = database
        .usage_page(&UsageQuery {
            source_or_account_query: Some("lookup-source".into()),
            include_filter_options: true,
            bucket_ms: Some(86_400_000),
            from_ms: Some(0),
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!((filtered.total, filtered.totals.total_tokens), (1, 130));
    assert_eq!(
        filtered
            .buckets
            .iter()
            .map(|bucket| bucket.totals.total_tokens)
            .sum::<u64>(),
        130
    );
    assert!(filtered
        .filter_options
        .unwrap()
        .pool_members
        .contains(&"lookup-source".to_string()));

    let unknown = failed_fallback_test_event("unknown");
    database.record(&unknown).unwrap();
    let page = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(
        page.events
            .iter()
            .find(|event| event.request_id == "unknown")
            .unwrap()
            .total_tokens,
        None
    );
    assert_eq!(
        (page.total, page.totals.requests, page.totals.total_tokens),
        (3, 2, 350)
    );
    assert_cache_matches_scan(&database);
    database.clear().unwrap();
    assert_eq!(
        database.usage_page(&UsageQuery::default()).unwrap().total,
        0
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn migration_backfills_retained_sends_once_and_retention_preserves_their_rollups() {
    let root = std::env::temp_dir().join(format!("relay-send-migration-{}", uuid::Uuid::new_v4()));
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    database
        .record(&aggregate_test_event("request", 1, 10, 0, None, 4))
        .unwrap();
    database
        .record(&aggregate_test_event("request", 2, 20, 0, None, 8))
        .unwrap();
    // Synthetic v35: only the final send plus a previously archived 100 tokens.
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "UPDATE usage_candidate_rollups SET input_tokens = 120, input_samples = 2,
            output_tokens = 8, output_samples = 2, total_tokens = 128, total_samples = 2,
            cache_write_input_samples = 2;
         DROP VIEW usage_sends;
         ALTER TABLE request_logs DROP COLUMN image_usage_json;
         ALTER TABLE request_attempts DROP COLUMN image_usage_json;
         DROP INDEX request_attempts_usage_reads;
         DROP INDEX request_logs_usage_reads;
         DROP INDEX request_attempts_account_usage_reads;
         DROP INDEX request_logs_account_usage_reads;
         CREATE INDEX request_logs_account_created_idx ON request_logs(account_id, created_at);
         DROP TABLE app_state_items;
         DROP TABLE weekly_capacity_anchors;
         DROP TABLE weekly_capacity_history;
         PRAGMA user_version = 35;",
        )
        .unwrap();
    drop(database);
    for _ in 0..2 {
        let database = TelemetryDb::open(&path).unwrap();
        let (input, output, total): (i64, i64, i64) = database.connection.lock().unwrap().query_row(
            "SELECT SUM(input_tokens), SUM(output_tokens), SUM(total_tokens) FROM usage_candidate_rollups", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!((input, output, total), (130, 12, 142));
        assert_eq!(
            database
                .usage_page(&UsageQuery::default())
                .unwrap()
                .totals
                .total_tokens,
            42
        );
    }
    let database = TelemetryDb::open(&path).unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch("UPDATE request_logs SET created_at = datetime('now', '-31 days')")
        .unwrap();
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    assert_eq!(
        database.usage_page(&UsageQuery::default()).unwrap().total,
        0
    );
    let total: i64 = database
        .connection
        .lock()
        .unwrap()
        .query_row(
            "SELECT SUM(total_tokens) FROM usage_candidate_rollups",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(total, 142);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovery_and_retries_keep_their_own_prices_members_and_quota_windows() {
    let root = std::env::temp_dir().join(format!("relay-send-prices-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut lookup = aggregate_test_event("request", 1, 100, 0, None, 10);
    lookup.success = false;
    lookup.account_id = Some("lookup-account".into());
    lookup.cached_input_tokens = Some(0);
    lookup.routing = Some(
        serde_json::from_value(serde_json::json!({
            "reason": "quota_headroom", "eligibleCandidates": 1,
            "quotaRemainingBasisPoints": null, "inFlightBefore": 0, "dispatchesBefore": 0,
            "attemptOutcome": "tool_discovery"
        }))
        .unwrap(),
    );
    database.record(&lookup).unwrap();
    let mut failed = lookup.clone();
    failed.attempt = 2;
    failed.http_status = 503;
    failed.error_category = Some("upstream_failure".into());
    database.record(&failed).unwrap();
    let mut answer = aggregate_test_event("request", 3, 200, 0, None, 20);
    answer.account_id = Some("answer-account".into());
    answer.cached_input_tokens = Some(0);
    database.record(&answer).unwrap();
    let errors = database
        .usage_page(&UsageQuery {
            success: Some(false),
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(errors.total, 1);
    assert_eq!(errors.events[0].attempt, 2);
    assert_eq!(errors.totals.total_tokens, 110);
    assert_eq!(errors.totals.requests, 0); // The client's final result succeeded.
    let context = PricingContext::default();
    let now = zenith_relay_core::unix_time_ms();
    for multiplier in [1, 2] {
        let catalog = PricingCatalog::from_litellm_payload(
            &serde_json::json!({"model": {
                "litellm_provider": "openai",
                "input_cost_per_token": 0.000002 * multiplier as f64,
                "output_cost_per_token": 0.000010 * multiplier as f64
            }}),
            Some(format!("synthetic-send-prices-{multiplier}")),
            Some(1),
            false,
        )
        .unwrap();
        let page = database
            .usage_page_with_pricing(&UsageQuery::default(), &catalog, &context)
            .unwrap();
        assert_eq!(page.totals.api_equivalent.micro_usd, 1200 * multiplier);
        assert_eq!(
            page.events
                .iter()
                .map(|event| event.api_equivalent.micro_usd)
                .sum::<u64>(),
            1200 * multiplier
        );
        assert_eq!(
            page.pool_members
                .iter()
                .find(|group| group.key == "lookup-account")
                .unwrap()
                .totals
                .api_equivalent
                .micro_usd,
            600 * multiplier
        );
        let accounts = database
            .api_equivalents_with_pricing(&catalog, &context)
            .unwrap()
            .accounts;
        assert_eq!(accounts["lookup-account"].micro_usd, 600 * multiplier);
        assert_eq!(accounts["answer-account"].micro_usd, 600 * multiplier);
        let windows = database
            .account_api_equivalents_with_pricing(
                &[(
                    "lookup-account".into(),
                    now.saturating_sub(60_000),
                    now + 60_000,
                )],
                &catalog,
                &context,
            )
            .unwrap();
        assert_eq!(windows["lookup-account"].micro_usd, 600 * multiplier);
    }
    // Corrections invalidate account and quota caches even for earlier sends.
    lookup.input_tokens = Some(120);
    lookup.total_tokens = Some(130);
    database.record(&lookup).unwrap();
    assert!(database.api_equivalent_cache.lock().unwrap().is_none());
    assert!(database.quota_equivalent_cache.lock().unwrap().is_none());
    assert_cache_matches_scan(&database);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
