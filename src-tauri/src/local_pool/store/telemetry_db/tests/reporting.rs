use super::*;

#[test]
fn history_purges_deleted_account_attempts_and_expires_with_the_final_row() {
    let root = std::env::temp_dir().join(format!("relay-attempt-purge-{}", uuid::Uuid::new_v4()));
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    let mut event = aggregate_test_event("rerouted", 1, 20, 0, None, 8);
    event.account_id = Some("removed-account".into());
    event.success = false;
    database.record(&event).unwrap();
    event.attempt = 2;
    event.account_id = None;
    event.success = true;
    database.record(&event).unwrap();
    let query = UsageQuery {
        request_id: Some("rerouted".into()),
        ..UsageQuery::default()
    };
    assert_eq!(database.usage_page(&query).unwrap().total, 2);
    database
        .replace_state_json_and_delete_account_data(&[], "removed-account")
        .unwrap();
    let history = database.usage_page(&query).unwrap();
    assert_eq!(history.total, 1);
    assert_eq!(history.events[0].attempt, 2);
    assert_eq!(
        database.usage_page(&UsageQuery::default()).unwrap().total,
        1
    );
    database
        .connection
        .lock()
        .unwrap()
        .execute(
            "UPDATE request_logs SET created_at = datetime('now', '-31 days')",
            [],
        )
        .unwrap();
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    assert_eq!(database.usage_page(&query).unwrap().total, 0);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sends_survive_reopen_without_inflating_logical_request_counts() {
    let root = std::env::temp_dir().join(format!("relay-attempts-{}", uuid::Uuid::new_v4()));
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    let mut event = aggregate_test_event("req", 1, 20, 0, None, 8);
    event.success = false;
    event.http_status = 503;
    event.source_id = "first".into();
    database.record(&event).unwrap();
    event.attempt = 2;
    event.success = true;
    event.http_status = 200;
    event.source_id = "second".into();
    database.record(&event).unwrap();
    // Late arrival of an earlier attempt must be retained without replacing the final row.
    event.attempt = 1;
    event.success = false;
    event.source_id = "first".into();
    database.record(&event).unwrap();
    event.request_id = "req_extra".into();
    database.record(&event).unwrap();
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    assert_eq!(
        database.usage_page(&UsageQuery::default()).unwrap().total,
        3
    );
    assert_eq!(
        database
            .usage_page(&UsageQuery::default())
            .unwrap()
            .totals
            .requests,
        2
    );
    let history = database
        .usage_page(&UsageQuery {
            request_id: Some("req".into()),
            page_size: 1,
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(history.total, 2);
    assert_eq!(history.total_pages, 2);
    assert_eq!(history.events[0].request_id, "req");
    assert_eq!(history.events[0].attempt, 2);
    assert!(history.events[0].success);
    assert!(history.events[0].id < 0);
    let first = database
        .usage_page(&UsageQuery {
            request_id: Some("req".into()),
            page: 2,
            page_size: 1,
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(first.events[0].source_id, "first");
    assert!(!first.events[0].success);
    database.clear().unwrap();
    assert_eq!(
        database
            .usage_page(&UsageQuery {
                request_id: Some("req".into()),
                ..UsageQuery::default()
            })
            .unwrap()
            .total,
        0
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn response_affinity_survives_reopen_and_expires() {
    let root = std::env::temp_dir().join(format!("zenith-relay-affinity-{}", uuid::Uuid::new_v4()));
    let path = root.join("usage.sqlite");
    let binding = ResponseAffinityBinding {
        key: "hashed-response".into(),
        candidate_id: "account-1".into(),
        expires_at_ms: 200,
    };
    TelemetryDb::open(&path)
        .unwrap()
        .upsert_affinity(&binding, 100)
        .unwrap();
    let database = TelemetryDb::open(&path).unwrap();
    assert_eq!(
        database.find_affinity(&binding.key, 199).unwrap(),
        Some(binding)
    );
    assert!(database.affinity_bindings(200).unwrap().is_empty());
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn response_affinity_storage_matches_the_runtime_capacity() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-affinity-capacity-{}",
        uuid::Uuid::new_v4()
    ));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(&format!(
            "WITH RECURSIVE entries(value) AS (
                    SELECT 0 UNION ALL SELECT value + 1 FROM entries WHERE value < {limit}
                 )
                 INSERT INTO response_affinity(
                    response_key, candidate_id, expires_at_ms, updated_at_ms
                 )
                 SELECT printf('response-%05d', value), 'account-1', 999999, value
                 FROM entries;",
            limit = MAX_RESPONSE_AFFINITY_ROWS + 1
        ))
        .unwrap();

    let bindings = database.affinity_bindings(0).unwrap();
    assert_eq!(bindings.len(), MAX_RESPONSE_AFFINITY_ROWS);
    let newest_key = format!("response-{:05}", MAX_RESPONSE_AFFINITY_ROWS + 1);
    assert_eq!(
        bindings.first().map(|binding| binding.key.as_str()),
        Some(newest_key.as_str())
    );
    assert_eq!(
        bindings.last().map(|binding| binding.key.as_str()),
        Some("response-00002")
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_page_aggregates_the_full_filtered_range_not_only_the_page() {
    let root =
        std::env::temp_dir().join(format!("zenith-relay-usage-page-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut event = aggregate_test_event("req_page_1", 1, 20, 0, None, 8);
    event.local_key_id = "key_1".into();
    event.source_id = "openai-codex".into();
    event.candidate_id = Some("account_1".into());
    event.account_id = Some("account_1".into());
    event.requested_model = Some("gpt-5.4".into());
    event.resolved_model = Some("gpt-5.4".into());
    event.consecutive_failures = Some(0);
    event.latency_ms = 428;
    event.ttft_ms = Some(128);
    event.generation_ms = Some(300);
    event.cached_input_tokens = Some(12);
    event.cache_write_input_tokens = None;
    event.reasoning_tokens = Some(5);
    database.record(&event).unwrap();
    event.request_id = "req_page_2".into();
    event.candidate_id = Some("account_2".into());
    event.account_id = Some("account_2".into());
    event.wire_api = WireApi::ChatCompletions;
    event.latency_ms = 500;
    event.ttft_ms = Some(100);
    event.input_tokens = Some(10);
    event.cached_input_tokens = Some(0);
    event.reasoning_tokens = Some(0);
    event.output_tokens = Some(20);
    event.total_tokens = Some(30);
    database.record(&event).unwrap();
    event.request_id = "req_page_3".into();
    event.success = false;
    event.http_status = 502;
    event.error_category = Some("upstream_websocket_closed".into());
    event.generation_ms = Some(5_000);
    event.input_tokens = Some(0);
    event.output_tokens = Some(100);
    event.total_tokens = Some(100);
    database.record(&event).unwrap();

    let page = database
        .usage_page(&UsageQuery {
            page: 1,
            page_size: 1,
            from_ms: Some(0),
            bucket_ms: Some(3_600_000),
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.total, 3);
    assert_eq!(page.total_pages, 3);
    assert_eq!(page.totals.requests, 3);
    assert_eq!(page.totals.total_tokens, 158);
    assert_eq!(page.totals.generation_output_tokens, 21);
    assert_eq!(page.totals.generation_ms, 600);
    assert_eq!(page.totals.generation_samples, 2);
    assert_eq!(page.totals.speed_output_tokens, 28);
    assert_eq!(page.totals.speed_duration_ms, 928);
    assert_eq!(page.totals.api_equivalent.priced_tokens, 158);
    assert_eq!(page.models.len(), 1);
    assert_eq!(page.pool_members.len(), 2);
    assert_eq!(page.buckets.len(), 1);
    assert_eq!(page.buckets[0].totals.total_tokens, 158);
    assert_eq!(
        page.buckets[0].totals.api_equivalent,
        page.totals.api_equivalent
    );
    assert_eq!(page.events[0].wire_api, "chat_completions");
    assert_eq!(page.events[0].service_tier, DefaultServiceTier::Standard);
    assert!(page.events[0].tool_use.is_none());

    event.request_id = "req_zero_generation".into();
    event.wire_api = WireApi::Responses;
    event.success = true;
    event.http_status = 200;
    event.error_category = None;
    event.generation_ms = Some(0);
    event.output_tokens = Some(100);
    database.record(&event).unwrap();
    let with_zero_generation = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(with_zero_generation.totals.generation_output_tokens, 21);
    assert_eq!(with_zero_generation.totals.generation_samples, 2);

    let chat = database
        .usage_page(&UsageQuery {
            wire_api: Some(WireApi::ChatCompletions),
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(chat.total, 2);
    assert_eq!(chat.events[0].request_id, "req_page_3");
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn account_quota_projection_matches_the_filtered_usage_total() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-account-quota-projection-{}",
        uuid::Uuid::new_v4()
    ));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut event = aggregate_test_event("account-window-1", 1, 40, 12, None, 8);
    event.source_id = "codex".into();
    event.candidate_id = Some("account-window".into());
    event.account_id = Some("account-window".into());
    database.record(&event).unwrap();
    event.request_id = "account-window-2".into();
    event.requested_model = Some("gpt-5.4".into());
    event.resolved_model = Some("gpt-5.4".into());
    event.input_tokens = Some(20);
    event.cache_write_input_tokens = Some(5);
    event.cache_write_ttl = Some("5m".to_string());
    event.output_tokens = Some(4);
    event.total_tokens = Some(24);
    database.record(&event).unwrap();

    let now = zenith_relay_core::unix_time_ms();
    let from_ms = now.saturating_sub(60_000);
    let to_ms = now.saturating_add(60_000);
    let catalog = test_pricing_catalog();
    let context = test_pricing_context(&BTreeMap::new(), &BTreeMap::new());
    let expected = database
        .usage_page_with_pricing(
            &UsageQuery {
                from_ms: Some(from_ms),
                to_ms: Some(to_ms),
                source_or_account_query: Some("account-window".into()),
                include_events: Some(false),
                include_models: Some(false),
                include_pool_members: Some(false),
                ..UsageQuery::default()
            },
            &catalog,
            &context,
        )
        .unwrap()
        .totals
        .api_equivalent;
    let actual = database
        .account_api_equivalents_with_pricing(
            &[("account-window".into(), from_ms, to_ms)],
            &catalog,
            &context,
        )
        .unwrap()["account-window"];

    assert_eq!(actual, expected);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn account_quota_projection_cache_invalidates_after_usage_write() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-account-quota-cache-{}",
        uuid::Uuid::new_v4()
    ));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut event = aggregate_test_event("account-cache-1", 1, 100, 0, None, 10);
    event.source_id = "codex".into();
    event.candidate_id = Some("account-cache".into());
    event.account_id = Some("account-cache".into());
    event.requested_model = Some("gpt-5.4".into());
    event.resolved_model = Some("gpt-5.4".into());
    database.record(&event).unwrap();

    let now = zenith_relay_core::unix_time_ms();
    let windows = [(
        "account-cache".to_string(),
        now.saturating_sub(60_000),
        now + 60_000,
    )];
    let catalog = test_pricing_catalog();
    let context = test_pricing_context(&BTreeMap::new(), &BTreeMap::new());
    let first = database
        .account_api_equivalents_with_pricing(&windows, &catalog, &context)
        .unwrap()["account-cache"];
    assert!(database.quota_equivalent_cache.lock().unwrap().is_some());
    let cached = database
        .account_api_equivalents_with_pricing(&windows, &catalog, &context)
        .unwrap()["account-cache"];
    assert_eq!(cached, first);

    event.request_id = "account-cache-2".into();
    event.input_tokens = Some(200);
    event.output_tokens = Some(20);
    event.total_tokens = Some(220);
    database.record(&event).unwrap();
    let updated = database
        .account_api_equivalents_with_pricing(&windows, &catalog, &context)
        .unwrap()["account-cache"];
    assert!(updated.micro_usd > first.micro_usd);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn cache_sessions_follow_the_latest_cache_touch_and_reported_window() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-cache-sessions-{}",
        uuid::Uuid::new_v4()
    ));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut start = aggregate_test_event("a-start", 1, 10, 0, None, 1);
    start.client_context_id = Some("client_aaaa".into());
    start.requested_model = Some("claude-sonnet".into());
    start.resolved_model = Some("claude-sonnet".into());
    start.cached_input_tokens = Some(0);
    database.record(&start).unwrap();

    let mut write = aggregate_test_event("a-write", 1, 10, 40, Some("1h".into()), 1);
    write.client_context_id = Some("client_aaaa".into());
    write.requested_model = Some("claude-sonnet".into());
    write.resolved_model = Some("claude-sonnet".into());
    database.record(&write).unwrap();

    let mut miss = aggregate_test_event("a-miss", 1, 10, 0, None, 1);
    miss.client_context_id = Some("client_aaaa".into());
    miss.requested_model = Some("claude-sonnet".into());
    miss.resolved_model = Some("claude-sonnet".into());
    miss.cached_input_tokens = Some(0);
    database.record(&miss).unwrap();

    let mut read = aggregate_test_event("a-read", 1, 10, 0, None, 1);
    read.client_context_id = Some("client_aaaa".into());
    read.requested_model = Some("claude-sonnet".into());
    read.resolved_model = Some("claude-sonnet".into());
    read.cached_input_tokens = Some(12);
    database.record(&read).unwrap();

    let mut gpt = aggregate_test_event("b-write", 1, 8, 8, None, 1);
    gpt.client_context_id = Some("client_bbbb".into());
    gpt.account_id = Some("account_b".into());
    gpt.requested_model = Some("gpt-6-astra".into());
    gpt.resolved_model = Some("gpt-6-astra".into());
    database.record(&gpt).unwrap();

    let orphan = aggregate_test_event("c-none", 1, 10, 9, Some("5m".into()), 1);
    database.record(&orphan).unwrap();

    let mut only_miss = aggregate_test_event("d-miss", 1, 10, 0, None, 1);
    only_miss.client_context_id = Some("client_dddd".into());
    only_miss.cached_input_tokens = Some(0);
    database.record(&only_miss).unwrap();

    {
        let connection = database.connection.lock().unwrap();
        for (request_id, created_at) in [
            ("a-start", "2026-09-27 10:00:00"),
            ("a-write", "2026-09-27 10:05:00"),
            ("a-miss", "2026-09-27 10:20:00"),
            ("a-read", "2026-09-27 10:40:00"),
            ("b-write", "2026-09-27 11:00:00"),
            ("d-miss", "2026-09-27 09:00:00"),
        ] {
            connection
                .execute(
                    "UPDATE request_logs SET created_at = ?1 WHERE request_id = ?2",
                    [created_at, request_id],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE request_attempts SET created_at = ?1 WHERE request_id = ?2",
                    [created_at, request_id],
                )
                .unwrap();
        }
    }

    let sessions = database.cache_sessions(&UsageQuery::default()).unwrap();
    assert_eq!(
        sessions,
        vec![
            CacheSession {
                client_context_id: "client_bbbb".into(),
                candidate_kind: "account".into(),
                candidate_id: "account_b".into(),
                started_at: "2026-09-27T11:00:00Z".into(),
                touched_at: "2026-09-27T11:00:00Z".into(),
                model: Some("gpt-6-astra".into()),
                cache_write_ttl: None,
            },
            CacheSession {
                client_context_id: "client_aaaa".into(),
                candidate_kind: "source".into(),
                candidate_id: write.source_id.clone(),
                started_at: "2026-09-27T10:00:00Z".into(),
                touched_at: "2026-09-27T10:40:00Z".into(),
                model: Some("claude-sonnet".into()),
                cache_write_ttl: Some("1h".into()),
            },
        ]
    );

    let from_ms = chrono::DateTime::parse_from_rfc3339("2026-09-27T10:30:00Z")
        .unwrap()
        .timestamp_millis() as u64;
    let ranged = database
        .cache_sessions(&UsageQuery {
            from_ms: Some(from_ms),
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(ranged.len(), 2);
    assert_eq!(ranged[1].started_at, "2026-09-27T10:00:00Z");
    assert_eq!(ranged[1].touched_at, "2026-09-27T10:40:00Z");

    let account = database
        .cache_sessions(&UsageQuery {
            source_or_account_query: Some("account_b".into()),
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(account.len(), 1);
    assert_eq!(account[0].client_context_id, "client_bbbb");
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cache_sessions_isolate_routes_and_ignore_misses_and_future_policies() {
    let root =
        std::env::temp_dir().join(format!("zenith-relay-cache-scope-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    for (id, account, source, model, read, write, ttl, time) in [
        (
            "write",
            Some("a"),
            "provider",
            "m1",
            0,
            8,
            Some("30m"),
            "12:00:00",
        ),
        (
            "early-miss",
            Some("a"),
            "provider",
            "m1",
            0,
            0,
            Some("1h"),
            "12:20:00",
        ),
        ("read", Some("a"), "provider", "m1", 8, 0, None, "12:25:00"),
        (
            "late-miss",
            Some("a"),
            "provider",
            "m1",
            0,
            0,
            Some("1h"),
            "12:26:00",
        ),
        (
            "model",
            Some("a"),
            "provider",
            "m2",
            0,
            9,
            Some("5m"),
            "12:26:00",
        ),
        (
            "account",
            Some("b"),
            "provider",
            "m1",
            0,
            7,
            Some("1h"),
            "12:27:00",
        ),
        ("source", None, "a", "m1", 0, 2, Some("15m"), "12:28:00"),
        (
            "source-two",
            None,
            "source-b",
            "m1",
            0,
            3,
            Some("45m"),
            "12:29:00",
        ),
        (
            "future",
            Some("a"),
            "provider",
            "m1",
            0,
            1,
            Some("1h"),
            "12:40:00",
        ),
    ] {
        let mut event = aggregate_test_event(id, 1, 10, write, ttl.map(str::to_owned), 1);
        event.client_context_id = Some("same_chat".into());
        event.account_id = account.map(str::to_owned);
        event.source_id = source.into();
        event.requested_model = Some(model.into());
        event.resolved_model = Some(model.into());
        event.cached_input_tokens = Some(read);
        database.record(&event).unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE request_logs SET created_at = ?1 WHERE request_id = ?2",
                [format!("2026-10-18 {time}"), id.to_owned()],
            )
            .unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE request_attempts SET created_at = ?1 WHERE request_id = ?2",
                [format!("2026-10-18 {time}"), id.to_owned()],
            )
            .unwrap();
    }
    let to_ms = chrono::DateTime::parse_from_rfc3339("2026-10-18T12:30:00Z")
        .unwrap()
        .timestamp_millis() as u64;
    let sessions = database
        .cache_sessions(&UsageQuery {
            to_ms: Some(to_ms),
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(sessions.len(), 5);
    for (kind, id, model, expected_ttl, expected_time) in [
        ("account", "a", "m1", "30m", "12:25:00"),
        ("account", "a", "m2", "5m", "12:26:00"),
        ("account", "b", "m1", "1h", "12:27:00"),
        ("source", "a", "m1", "15m", "12:28:00"),
        ("source", "source-b", "m1", "45m", "12:29:00"),
    ] {
        let session = sessions
            .iter()
            .find(|session| {
                session.candidate_kind == kind
                    && session.candidate_id == id
                    && session.model.as_deref() == Some(model)
            })
            .unwrap();
        assert_eq!(session.cache_write_ttl.as_deref(), Some(expected_ttl));
        assert_eq!(session.touched_at, format!("2026-10-18T{expected_time}Z"));
    }
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
