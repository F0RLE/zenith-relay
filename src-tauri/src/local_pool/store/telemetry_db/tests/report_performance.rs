use super::*;

const ROWS_PER_MEMBER: u64 = 12_500;
const MEMBERS: u64 = 8;

#[test]
#[ignore = "selected-page versus full-report timing on synthetic history"]
fn measure_usage_selected_pages() {
    let root = std::env::temp_dir().join(format!("relay-selected-pages-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    populate_report_history(&database);
    let catalog = test_pricing_catalog();
    let overrides = BTreeMap::new();
    let source_overrides = BTreeMap::new();
    let context = test_pricing_context(&overrides, &source_overrides);
    let mut first_events = None;
    for (phase, page, report) in [
        ("selected_first", 1, false),
        ("selected_second", 2, false),
        ("full_report", 1, true),
    ] {
        let query = UsageQuery {
            page,
            include_models: Some(false),
            include_pool_members: Some(false),
            include_filter_options: report,
            ..Default::default()
        };
        let started = Instant::now();
        let result = if report {
            database.usage_page_with_pricing(&query, &catalog, &context)
        } else {
            database.usage_send_page_with_pricing(&query, &catalog, &context)
        }
        .unwrap();
        println!(
            "usage_selected_page phase={phase} retained={} events={} elapsed_ms={:.2}",
            ROWS_PER_MEMBER * MEMBERS,
            result.events.len(),
            started.elapsed().as_secs_f64() * 1_000.0
        );
        assert_eq!(result.events.len(), 50);
        if report {
            assert_eq!(result.total, ROWS_PER_MEMBER * MEMBERS);
            assert_eq!(
                Some(serde_json::to_value(result.events).unwrap()),
                first_events
            );
        } else {
            assert_eq!(result.has_more, Some(true));
            assert!(database.usage_totals_cache.lock().unwrap().is_none());
            if page == 1 {
                first_events = Some(serde_json::to_value(result.events).unwrap());
            }
        }
    }
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "synthetic filter-history scan/index/cache comparison; no live data"]
fn measure_usage_filter_choices() {
    let root = std::env::temp_dir().join(format!("relay-filter-timing-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    populate_report_history(&database);
    let connection = database.connection.lock().unwrap();
    connection
        .execute_batch(
            "DROP INDEX request_attempts_usage_reads; DROP INDEX request_logs_usage_reads;",
        )
        .unwrap();
    let mut reference = None;
    for phase in ["scan", "covering_index", "cached"] {
        if phase == "covering_index" {
            connection.execute_batch(MIGRATION_043).unwrap();
            database.usage_filter_options_cache.lock().unwrap().take();
        }
        let started = Instant::now();
        let choices = database
            .cached_usage_filter_options(&connection, &UsageQuery::default())
            .unwrap();
        println!(
            "usage_filter_choices phase={phase} rows={} elapsed_ms={:.2}",
            ROWS_PER_MEMBER * MEMBERS,
            started.elapsed().as_secs_f64() * 1_000.0
        );
        let choices = serde_json::to_value(choices).unwrap();
        if let Some(reference) = &reference {
            assert_eq!(&choices, reference);
        } else {
            reference = Some(choices);
        }
    }
    assert_eq!(
        database
            .usage_filter_option_reads
            .load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "synthetic account-window reads and per-account cache invalidation; no live data"]
fn measure_quota_window_reads() {
    let root = std::env::temp_dir().join(format!("relay-quota-timing-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    populate_report_history(&database);
    {
        let connection = database.connection.lock().unwrap();
        for table in ["request_logs", "request_attempts"] {
            connection
                .execute_batch(&format!(
                    "UPDATE {table} SET account_id = source_id, candidate_id = source_id,
                 source_id = 'codex', requested_model = 'gpt-5.4', resolved_model = 'gpt-5.4',
                 cached_input_tokens = 0;",
                ))
                .unwrap();
        }
    }
    let now = zenith_relay_core::unix_time_ms();
    let windows = (0..MEMBERS)
        .map(|member| (format!("report-source-{member}"), 0, now + 60_000))
        .collect::<Vec<_>>();
    let catalog = test_pricing_catalog();
    let context = test_pricing_context(&BTreeMap::new(), &BTreeMap::new());
    let mut previous = None;
    for phase in ["cold", "cached", "one_account_changed"] {
        if phase == "one_account_changed" {
            let mut event = aggregate_test_event("quota-new-send", 1, 100, 0, None, 10);
            event.account_id = Some(windows[0].0.clone());
            event.source_id = "codex".into();
            event.requested_model = Some("gpt-5.4".into());
            event.resolved_model = event.requested_model.clone();
            event.cached_input_tokens = Some(0);
            database.record(&event).unwrap();
        }
        let started = Instant::now();
        let result = database
            .account_api_equivalents_with_pricing(&windows, &catalog, &context)
            .unwrap();
        println!("quota_window_reads phase={phase} retained={} accounts={} sql_reads={} elapsed_ms={:.2}",
            ROWS_PER_MEMBER * MEMBERS, result.len(),
            database.quota_window_reads.load(std::sync::atomic::Ordering::Relaxed),
            started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(result.len(), MEMBERS as usize);
        if let Some(previous) = previous.as_ref() {
            if phase == "cached" {
                assert_eq!(&result, previous);
            } else {
                for (id, value) in &result {
                    if id != &windows[0].0 {
                        assert_eq!(value, &previous[id]);
                    }
                }
            }
        }
        for (id, value) in &result {
            let extra = u64::from(phase == "one_account_changed" && id == &windows[0].0);
            assert_eq!(value.priced_tokens, (ROWS_PER_MEMBER + extra) * 110);
        }
        assert_eq!(
            database
                .quota_window_reads
                .load(std::sync::atomic::Ordering::Relaxed),
            MEMBERS + u64::from(phase == "one_account_changed")
        );
        previous = Some(result);
    }
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

/// Bulk-copy synthetic sends and their matching rollups, not real history.
/// Several models/members, seven days and 1 KiB diagnostic rows exercise the
/// report paths that the small first-entry probe intentionally does not cover.
fn populate_report_history(database: &TelemetryDb) {
    for member in 0..MEMBERS {
        let mut event = aggregate_test_event(&format!("report-seed-{member}"), 1, 100, 0, None, 10);
        event.source_id = format!("report-source-{member}");
        event.candidate_id = Some(event.source_id.clone());
        event.requested_model = Some(format!("report-model-{}", member % 4));
        event.resolved_model = event.requested_model.clone();
        database.record(&event).unwrap();
    }
    let connection = database.connection.lock().unwrap();
    for table in ["request_logs", "request_attempts"] {
        let columns: Vec<String> = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        let projection = columns
            .iter()
            .map(|column| match column.as_str() {
                "id" if table == "request_attempts" => format!("-(ABS(id) + n * {MEMBERS})"),
                "id" => format!("id + n * {MEMBERS}"),
                "request_id" => "request_id || '-' || n".into(),
                "created_at" => "datetime('now', '-' || (n % 168) || ' hours')".into(),
                "tool_use_json" => {
                    "json_set(COALESCE(tool_use_json, '{}'), '$.syntheticPadding', printf('%.*c', 1024, 'x'))"
                        .into()
                }
                _ => column.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let seeds = (0..MEMBERS)
            .map(|member| format!("'report-seed-{member}'"))
            .collect::<Vec<_>>()
            .join(", ");
        connection.execute_batch(&format!(
            "WITH RECURSIVE sequence(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM sequence WHERE n < {last})
             INSERT INTO {table} ({columns}) SELECT {projection} FROM {table}, sequence WHERE request_id IN ({seeds})",
            last = ROWS_PER_MEMBER - 1,
            columns = columns.join(", "),
        )).unwrap();
    }
    // All copies have the same counters and price classification as their seed.
    // Keep the normal recorded-rollup fast path, rather than accidentally
    // benchmarking only legacy rows or reporting incomplete all-time prices.
    let counters: Vec<String> = connection
        .prepare("PRAGMA table_info(usage_candidate_rollups)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    let updates = counters
        .iter()
        .filter(|column| column.ends_with("_tokens") || column.ends_with("_samples"))
        .map(|column| format!("{column} = {column} * {ROWS_PER_MEMBER}"))
        .collect::<Vec<_>>()
        .join(", ");
    connection
        .execute_batch(&format!("UPDATE usage_candidate_rollups SET {updates}"))
        .unwrap();
    let padded: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM request_attempts WHERE length(tool_use_json) >= 1024",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(padded, (MEMBERS * (ROWS_PER_MEMBER - 1)) as i64);
    drop(connection);
    database.clear_cached_usage_totals().unwrap();
}

#[test]
#[ignore = "large local report timing probe; synthetic history only, no live accounts"]
fn measure_usage_large_reports() {
    let root = std::env::temp_dir().join(format!("relay-report-timing-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    populate_report_history(&database);
    let requests = UsageQuery {
        include_models: Some(false),
        include_pool_members: Some(false),
        include_filter_options: true,
        ..UsageQuery::default()
    };
    let report = UsageQuery {
        from_ms: Some((chrono::Utc::now().timestamp_millis() as u64) - 7 * 86_400_000),
        bucket_ms: Some(3_600_000),
        include_events: Some(false),
        include_filter_options: true,
        ..UsageQuery::default()
    };
    for (name, query, expected) in [
        ("requests", requests, ROWS_PER_MEMBER * MEMBERS),
        ("report", report.clone(), ROWS_PER_MEMBER * MEMBERS),
        (
            "filtered",
            UsageQuery {
                source_or_account_query: Some("report-source-0".into()),
                ..report.clone()
            },
            ROWS_PER_MEMBER,
        ),
    ] {
        for phase in ["cold", "warm"] {
            let started = Instant::now();
            let page = database.usage_page(&query).unwrap();
            println!(
                "usage_large_report case={name} phase={phase} rows={} elapsed_ms={:.2}",
                page.total,
                started.elapsed().as_secs_f64() * 1000.0
            );
            assert_eq!(page.total, expected);
            assert_eq!(page.totals.requests, expected);
            assert_eq!(page.totals.total_tokens, expected * 110);
            assert_eq!(
                page.events.len(),
                if query.includes_events() { 50 } else { 0 }
            );
            if query.bucket_ms.is_some() {
                assert_eq!(
                    page.buckets
                        .iter()
                        .map(|bucket| bucket.totals.total_tokens)
                        .sum::<u64>(),
                    expected * 110
                );
            }
        }
    }
    // Stage timings isolate repeat scans from page decoding/catalog work.
    let catalog = test_pricing_catalog();
    let context = test_pricing_context(&BTreeMap::new(), &BTreeMap::new());
    let resolver = CatalogPriceResolver::new(&catalog, &context);
    let connection = database.connection.lock().unwrap();
    let (where_sql, parameters) = usage_filter(&report);
    let started = Instant::now();
    let reference = super::performance::reference_full_view_groups(
        &connection,
        &report,
        "COALESCE(resolved_model, requested_model, '')",
    );
    println!(
        "usage_large_report stage=legacy_models elapsed_ms={:.2}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let started = Instant::now();
    let groups = usage_groups(
        &connection,
        &where_sql,
        &parameters,
        "COALESCE(resolved_model, requested_model, '')",
    )
    .unwrap();
    println!(
        "usage_large_report stage=models elapsed_ms={:.2}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(
        serde_json::to_value(
            groups
                .into_iter()
                .map(|group| (group.key, group.totals))
                .collect::<BTreeMap<_, _>>()
        )
        .unwrap(),
        serde_json::to_value(reference).unwrap()
    );
    let started = Instant::now();
    usage_model_equivalents(&connection, &where_sql, &parameters, &resolver, false).unwrap();
    println!(
        "usage_large_report stage=pricing elapsed_ms={:.2}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let started = Instant::now();
    usage_buckets(&connection, &where_sql, &parameters, &report, &resolver).unwrap();
    println!(
        "usage_large_report stage=buckets elapsed_ms={:.2}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
