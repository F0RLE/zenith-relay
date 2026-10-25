use super::*;
use zenith_relay_core::protocol::UsageWorkload;

#[test]
fn workload_reports_partition_all_sends_without_hiding_background_consumption() {
    let root = std::env::temp_dir().join(format!("relay-workloads-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    for (id, attempt, category, input, cached) in [
        ("dialog", 1, None, 1_000, Some(900)),
        (
            "unknown",
            1,
            Some("codex_unknown_provider_error"),
            2_000,
            None,
        ),
        (
            "extract",
            1,
            Some("codex_memory_extraction"),
            500_000,
            Some(10_000),
        ),
        (
            "extract",
            2,
            Some("codex_memory_extraction"),
            5_000,
            Some(4_000),
        ),
        (
            "consolidate",
            1,
            Some("codex_memory_consolidation"),
            20_000,
            Some(15_000),
        ),
        (
            "blocked",
            1,
            Some("codex_background_blocked_memory_extraction"),
            0,
            None,
        ),
        ("wake", 1, Some("codex_wake"), 700, Some(100)),
    ] {
        let mut event = aggregate_test_event(id, attempt, input, 0, None, 0);
        event.error_category = category.map(str::to_string);
        event.cached_input_tokens = cached;
        if id == "unknown" || id == "blocked" {
            event.success = false;
            event.http_status = 403;
        }
        database.record(&event).unwrap();
    }
    let all = database.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(all.workload, None);
    assert_eq!(
        (all.total, all.totals.requests, all.totals.input_tokens),
        (7, 6, 528_700)
    );
    let mut scoped_input = 0;
    let mut scoped_cached = 0;
    for (workload, sends, requests, input, cached, samples) in [
        (UsageWorkload::Conversation, 2, 2, 3_000, 900, 1),
        (UsageWorkload::Memory, 4, 3, 525_000, 29_000, 3),
        (UsageWorkload::Background, 1, 1, 700, 100, 1),
    ] {
        let query = UsageQuery {
            workload: Some(workload),
            ..UsageQuery::default()
        };
        let page = database.usage_page(&query).unwrap();
        assert_eq!(page.workload, Some(workload));
        assert_eq!((page.total, page.totals.requests), (sends, requests));
        assert_eq!(
            (
                page.totals.input_tokens,
                page.totals.cached_input_tokens,
                page.totals.cached_input_samples
            ),
            (input, cached, samples)
        );
        assert_eq!(
            page.models
                .iter()
                .map(|g| g.totals.input_tokens)
                .sum::<u64>(),
            input
        );
        assert_eq!(
            page.pool_members
                .iter()
                .map(|g| g.totals.input_tokens)
                .sum::<u64>(),
            input
        );
        let chart = database
            .usage_page(&UsageQuery {
                from_ms: Some(0),
                bucket_ms: Some(86_400_000),
                include_events: Some(false),
                ..query
            })
            .unwrap();
        assert!(chart.events.is_empty());
        assert_eq!(
            chart
                .buckets
                .iter()
                .map(|b| b.totals.input_tokens)
                .sum::<u64>(),
            input
        );
        scoped_input += input;
        scoped_cached += cached;
    }
    assert_eq!(scoped_input, all.totals.input_tokens);
    assert_eq!(scoped_cached, all.totals.cached_input_tokens);
    let memory_only = database
        .usage_page(&UsageQuery {
            workload: Some(UsageWorkload::Memory),
            error_category: Some("codex_memory_extraction".into()),
            page_size: 1,
            page: 2,
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(memory_only.events.len(), 1);
    assert_eq!(memory_only.total, 2);
    assert_eq!(memory_only.totals.input_tokens, 505_000);
}

#[test]
fn workload_query_is_typed_optional_and_rejects_unknown_scopes() {
    let query: UsageQuery =
        serde_json::from_value(serde_json::json!({"workload":"memory"})).unwrap();
    assert_eq!(query.workload, Some(UsageWorkload::Memory));
    assert!(
        serde_json::from_value::<UsageQuery>(serde_json::json!({"workload":"made_up"})).is_err()
    );
    assert_eq!(
        serde_json::from_value::<UsageQuery>(serde_json::json!({}))
            .unwrap()
            .workload,
        None
    );
}
