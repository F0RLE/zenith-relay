use super::*;

#[test]
fn narrow_send_pages_preserve_filters_order_and_legacy_fallback() {
    let root = std::env::temp_dir().join(format!("relay-usage-pages-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    for index in 0..35 {
        let mut event = aggregate_test_event(&format!("page-{index}"), 1, 100, 0, None, 10);
        event.success = index % 2 == 0;
        event.error_category = (!event.success).then(|| "synthetic_failure".into());
        event.requested_model = Some(format!("model-{}", index % 3));
        database.record(&event).unwrap();
        if index % 5 == 0 {
            event.attempt = 2;
            database.record(&event).unwrap();
        }
    }
    // Keep a real legacy-only final send and an older attempt of another
    // request: excluding by request_id alone would silently lose one of them.
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "DELETE FROM request_attempts WHERE request_id = 'page-1';
         DELETE FROM request_attempts WHERE request_id = 'page-0' AND attempt = 2;
         UPDATE request_logs SET id = id + 1000 WHERE request_id IN ('page-0', 'page-1');",
        )
        .unwrap();
    for filter in [
        UsageQuery::default(),
        UsageQuery {
            model_query: Some("model-1".into()),
            ..UsageQuery::default()
        },
        UsageQuery {
            success: Some(false),
            ..UsageQuery::default()
        },
        UsageQuery {
            request_id: Some("page-0".into()),
            ..UsageQuery::default()
        },
        UsageQuery {
            from_ms: Some(0),
            wire_api: Some(WireApi::Responses),
            ..UsageQuery::default()
        },
    ] {
        for page_number in 1..=5 {
            let query = UsageQuery {
                page: page_number,
                page_size: 7,
                ..filter.clone()
            };
            let page = database.usage_page(&query).unwrap();
            let connection = database.connection.lock().unwrap();
            let (where_sql, mut parameters) = usage_filter(&query);
            let expected_total: i64 = connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM usage_sends{where_sql}"),
                    params_from_iter(parameters.iter()),
                    |row| row.get(0),
                )
                .unwrap();
            parameters.push(SqlValue::Integer(7));
            parameters.push(SqlValue::Integer(i64::from(page_number - 1) * 7));
            let expected = connection
                .prepare(&format!(
                    "SELECT id, request_id, attempt FROM usage_sends{where_sql}
                 ORDER BY created_at DESC, ABS(id) DESC LIMIT ? OFFSET ?"
                ))
                .unwrap()
                .query_map(params_from_iter(parameters.iter()), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u16>(2)?,
                    ))
                })
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            let actual: Vec<_> = page
                .events
                .iter()
                .map(|event| (event.id, event.request_id.clone(), event.attempt))
                .collect();
            assert_eq!(page.total, expected_total as u64);
            assert_eq!(actual, expected);
        }
    }
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn first_entry_uses_narrow_count_and_unaggregated_indexes() {
    let root = std::env::temp_dir().join(format!("relay-usage-plan-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let connection = database.connection.lock().unwrap();
    let explain = |sql: &str| {
        connection
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
            .join("\n")
    };
    let sends = super::super::usage::send_projection("1", "");
    let plan = explain(&format!("SELECT COUNT(*) FROM ({sends})"));
    assert!(
        plan.contains("COVERING INDEX request_logs_send_history"),
        "{plan}"
    );
    let plan = explain("SELECT COUNT(*) FROM usage_sends WHERE usage_aggregate_recorded = 0");
    assert!(plan.contains("request_logs_unaggregated"), "{plan}");
    assert!(plan.contains("request_attempts_unaggregated"), "{plan}");
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "local first-entry timing probe; no live accounts or provider requests"]
fn measure_usage_first_entry() {
    let root = std::env::temp_dir().join(format!("relay-usage-timing-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    database
        .record(&aggregate_test_event("timing-seed", 1, 100, 0, None, 10))
        .unwrap();
    {
        let connection = database.connection.lock().unwrap();
        for table in ["request_logs", "request_attempts"] {
            let mut statement = connection
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            let columns: Vec<String> = statement
                .query_map([], |row| row.get(1))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap();
            let projection = columns
                .iter()
                .map(|column| match column.as_str() {
                    "id" if table == "request_attempts" => "-n".to_string(),
                    "id" => "n".to_string(),
                    "request_id" => "'timing-' || n".to_string(),
                    _ => column.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            connection.execute_batch(&format!(
                "WITH RECURSIVE sequence(n) AS (SELECT 2 UNION ALL SELECT n + 1 FROM sequence WHERE n < 10000)
                 INSERT INTO {table} ({columns}) SELECT {projection} FROM {table}, sequence WHERE request_id = 'timing-seed'",
                columns = columns.join(", "),
            )).unwrap();
        }
    }
    let query = UsageQuery {
        include_models: Some(false),
        include_pool_members: Some(false),
        include_filter_options: true,
        ..UsageQuery::default()
    };
    for phase in ["cold", "warm"] {
        let started = Instant::now();
        let page = database.usage_page(&query).unwrap();
        println!(
            "usage_first_entry phase={phase} rows={} events={} elapsed_ms={:.2}",
            page.total,
            page.events.len(),
            started.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(page.total, 10_000);
        assert_eq!(page.events.len(), 50);
        assert_eq!(page.totals.total_tokens, 1_100_000);
    }
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
