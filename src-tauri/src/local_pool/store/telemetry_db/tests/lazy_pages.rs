use super::*;

fn sends(database: &TelemetryDb, query: &UsageQuery) -> Result<LocalUsagePage> {
    let catalog = test_pricing_catalog();
    let overrides = BTreeMap::new();
    let source_overrides = BTreeMap::new();
    let context = test_pricing_context(&overrides, &source_overrides);
    database.usage_send_page_with_pricing(query, &catalog, &context)
}

#[test]
fn report_cache_memory_budget_counts_groups_labels_buckets_and_scope() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    let mut page = database.usage_page(&UsageQuery::default()).unwrap();
    assert!(CachedUsageReport::fits_cache("scope", "revision", &page));
    assert!(!CachedUsageReport::fits_cache(
        &"x".repeat(MAX_USAGE_REPORT_CACHE_BYTES),
        "revision",
        &page
    ));
    page.models.push(UsageGroup {
        key: "model".into(),
        label: Some("x".repeat(MAX_USAGE_REPORT_CACHE_BYTES)),
        ..Default::default()
    });
    assert!(!CachedUsageReport::fits_cache("scope", "revision", &page));
    page.models.clear();
    page.buckets = vec![
        UsageBucket::default();
        MAX_USAGE_REPORT_CACHE_BYTES / std::mem::size_of::<UsageBucket>() + 1
    ];
    assert!(!CachedUsageReport::fits_cache("scope", "revision", &page));
    assert!(
        !page.buckets.is_empty(),
        "Uncacheable reports remain complete"
    );
}

#[test]
fn oversized_report_is_returned_without_retaining_a_duplicate_summary() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    database
        .record(&aggregate_test_event("large-report", 1, 100, 0, None, 10))
        .unwrap();
    let model = "x".repeat(MAX_USAGE_REPORT_CACHE_BYTES);
    {
        let connection = database.connection.lock().unwrap();
        for table in ["request_logs", "request_attempts"] {
            connection
                .execute(
                    &format!("UPDATE {table} SET requested_model = ?1, resolved_model = ?1"),
                    [&model],
                )
                .unwrap();
        }
    }
    let report = database
        .usage_page(&UsageQuery {
            include_events: Some(false),
            include_pool_members: Some(false),
            success: Some(true),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(report.models.len(), 1);
    assert_eq!(report.models[0].key, model);
    assert_eq!(report.totals.requests, 1);
    assert!(database.usage_report_cache.lock().unwrap().is_none());
}

#[test]
fn full_report_reuses_summary_but_reads_each_selected_page_and_invalidates_history() {
    use std::sync::atomic::Ordering;
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    for index in 0..125 {
        database
            .record(&aggregate_test_event(
                &format!("report-{index}"),
                1,
                100,
                0,
                None,
                10,
            ))
            .unwrap();
    }
    let query = UsageQuery {
        include_models: Some(false),
        include_pool_members: Some(false),
        ..Default::default()
    };
    let first = database.usage_page(&query).unwrap();
    assert_eq!(first.events.len(), 50);
    for (page, size, count) in [(2, 50, 50), (3, 50, 25), (2, 10, 10)] {
        let result = database
            .usage_page(&UsageQuery {
                page,
                page_size: size,
                ..query.clone()
            })
            .unwrap();
        assert_eq!(result.events.len(), count);
        assert_eq!(result.total, first.total);
        assert_eq!(result.totals, first.totals);
        assert_eq!(result.page, page);
        assert_eq!(result.page_size, size);
    }
    assert_eq!(database.usage_report_builds.load(Ordering::Relaxed), 1);
    {
        let cached = database.usage_report_cache.lock().unwrap();
        assert!(cached.as_ref().unwrap().page.events.is_empty());
        assert!(cached.as_ref().unwrap().page.filter_options.is_none());
    }
    let filtered = database
        .usage_page(&UsageQuery {
            success: Some(false),
            ..query.clone()
        })
        .unwrap();
    assert_eq!(filtered.total, 0);
    database
        .record(&aggregate_test_event("report-new", 1, 200, 0, None, 20))
        .unwrap();
    let added = database.usage_page(&query).unwrap();
    assert_eq!(added.total, 126);
    database
        .record(&aggregate_test_event("report-new", 1, 300, 0, None, 40))
        .unwrap();
    let corrected = database.usage_page(&query).unwrap();
    assert_eq!(corrected.total, 126);
    assert_eq!(
        corrected.totals.input_tokens,
        added.totals.input_tokens + 100
    );
    assert_eq!(
        corrected.totals.output_tokens,
        added.totals.output_tokens + 20
    );
    database.clear().unwrap();
    assert_eq!(database.usage_page(&query).unwrap().total, 0);
    assert_eq!(database.usage_report_builds.load(Ordering::Relaxed), 5);
}

#[test]
fn selected_send_pages_preserve_order_filters_prices_and_legacy_rows() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    for index in 0..33 {
        let mut event = aggregate_test_event(&format!("lazy-{index}"), 1, 100, 0, None, 10);
        event.success = index % 2 == 0;
        event.requested_model = Some(format!("model-{}", index % 3));
        database.record(&event).unwrap();
        if index % 5 == 0 {
            event.attempt = 2;
            database.record(&event).unwrap();
        }
    }
    database
        .connection
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM request_attempts WHERE request_id = 'lazy-1'",
            [],
        )
        .unwrap();
    for query in [
        UsageQuery {
            page: 1,
            page_size: 10,
            ..Default::default()
        },
        UsageQuery {
            page: 2,
            page_size: 10,
            ..Default::default()
        },
        UsageQuery {
            page: 4,
            page_size: 10,
            ..Default::default()
        },
        UsageQuery {
            page: 5,
            page_size: 10,
            ..Default::default()
        },
        UsageQuery {
            success: Some(false),
            page_size: 10,
            ..Default::default()
        },
        UsageQuery {
            model_query: Some("model-1".into()),
            page_size: 10,
            ..Default::default()
        },
        UsageQuery {
            request_id: Some("lazy-0".into()),
            ..Default::default()
        },
    ] {
        let expected = database.usage_page(&query).unwrap();
        let actual = sends(&database, &query).unwrap();
        assert_eq!(
            actual.has_more,
            Some(query.normalized_page().0 < expected.total_pages)
        );
        assert_eq!(
            serde_json::to_value(actual.events).unwrap(),
            serde_json::to_value(expected.events).unwrap()
        );
        assert!(actual.models.is_empty());
        assert!(actual.pool_members.is_empty());
        assert!(actual.buckets.is_empty());
        assert!(actual.filter_options.is_none());
    }
    let full = serde_json::to_value(database.usage_page(&UsageQuery::default()).unwrap()).unwrap();
    assert!(full.get("hasMore").is_none());
}

#[test]
fn report_summary_cache_reprices_when_the_catalog_revision_changes() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    database
        .record(&aggregate_test_event("reprice-cache", 1, 100, 0, None, 10))
        .unwrap();
    let mut amounts = Vec::new();
    for rate in [0.000001, 0.000002] {
        let catalog = PricingCatalog::from_litellm_payload(
            &serde_json::json!({"model":{"litellm_provider":"openai",
                "input_cost_per_token":rate,"output_cost_per_token":0.00001}}),
            Some(format!("synthetic-{rate}")),
            Some(1),
            false,
        )
        .unwrap();
        let report = database
            .usage_page_with_pricing(&UsageQuery::default(), &catalog, &PricingContext::default())
            .unwrap();
        amounts.push(report.totals.api_equivalent.micro_usd);
    }
    assert!(amounts[1] > amounts[0]);
    assert_eq!(
        database
            .usage_report_builds
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
}

#[test]
fn report_summary_cache_refreshes_catalog_health_without_rebuilding_totals() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    database
        .record(&aggregate_test_event(
            "freshness-cache",
            1,
            100,
            0,
            None,
            10,
        ))
        .unwrap();
    let mut sources = Vec::new();
    for (fetched_at, stale) in [(1, false), (2, true), (3, false)] {
        let catalog = PricingCatalog::from_litellm_payload(
            &serde_json::json!({"model":{"litellm_provider":"openai",
                "input_cost_per_token":0.000001,"output_cost_per_token":0.00001}}),
            Some("same-revision".into()),
            Some(fetched_at),
            stale,
        )
        .unwrap();
        let report = database
            .usage_page_with_pricing(&UsageQuery::default(), &catalog, &PricingContext::default())
            .unwrap();
        assert_eq!(report.pricing.catalog_fetched_at_ms, Some(fetched_at));
        assert_eq!(report.pricing.catalog_stale, stale);
        sources.push(report.pricing.price_source);
    }
    assert!(sources.iter().all(|source| *source == sources[0]));
    assert_eq!(
        database
            .usage_report_builds
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[test]
fn first_page_does_not_read_aggregates_filters_or_decode_the_lookahead_row() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    for index in 0..61 {
        database
            .record(&aggregate_test_event(
                &format!("bounded-{index}"),
                1,
                100,
                0,
                None,
                10,
            ))
            .unwrap();
    }
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "DROP TABLE usage_candidate_rollups;
         UPDATE request_attempts SET total_tokens = 'unread-lookahead'
         WHERE rowid = (SELECT rowid FROM request_attempts
             ORDER BY created_at DESC, ABS(id) DESC LIMIT 1 OFFSET 50);",
        )
        .unwrap();
    let first = sends(&database, &UsageQuery::default()).unwrap();
    assert_eq!(first.events.len(), 50);
    assert_eq!(first.has_more, Some(true));
    assert!(database.usage_totals_cache.lock().unwrap().is_none());
    assert_eq!(
        database
            .usage_filter_option_reads
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    // Only an explicit request for page two attempts to decode that row.
    assert!(sends(
        &database,
        &UsageQuery {
            page: 2,
            ..Default::default()
        }
    )
    .is_err());
}

#[test]
fn explicit_filter_choices_do_not_load_events_or_full_statistics() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    database
        .record(&aggregate_test_event("filter-intent", 1, 100, 0, None, 10))
        .unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE usage_candidate_rollups;")
        .unwrap();
    let page = sends(
        &database,
        &UsageQuery {
            include_events: Some(false),
            include_filter_options: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(page.events.is_empty());
    assert_eq!(page.has_more, Some(false));
    assert!(!page.filter_options.unwrap().models.is_empty());
    assert!(database.usage_totals_cache.lock().unwrap().is_none());
}
