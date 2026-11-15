use super::*;

#[test]
fn narrow_send_pages_preserve_filters_order_and_legacy_fallback() {
    let root = crate::files::test_temp_root("relay-usage-pages");
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    for index in 0..35 {
        let mut event = aggregate_test_event(&format!("page-{index}"), 1, 100, 0, None, 10);
        event.success = index % 2 == 0;
        event.error_category = (!event.success).then(|| "synthetic_failure".into());
        event.requested_model = Some(format!("model-{}", index % 3));
        event.cached_input_tokens = (index % 3 != 0).then_some(index);
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
         UPDATE request_logs SET id = id + 1000 WHERE request_id IN ('page-0', 'page-1');
         UPDATE request_logs SET created_at = datetime('now', '-' || (id % 3) || ' hours');
         UPDATE request_attempts SET created_at = (SELECT created_at FROM request_logs WHERE request_logs.request_id = request_attempts.request_id);",
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
        UsageQuery {
            model_query: Some("model-1".into()),
            source_or_account_query: Some("source".into()),
            success: Some(false),
            ..UsageQuery::default()
        },
    ] {
        for page_number in 1..=5 {
            let query = UsageQuery {
                page: page_number,
                page_size: 7,
                from_ms: Some(0),
                bucket_ms: Some(3_600_000),
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
            if page_number == 1 {
                assert_report_matches_full_view(&connection, &query, &page);
            }
        }
    }
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

pub(super) fn reference_full_view_groups(
    connection: &Connection,
    query: &UsageQuery,
    key_sql: &str,
) -> BTreeMap<String, UsageTotals> {
    use super::super::usage::{consumption_totals, request_totals};
    let (where_sql, parameters) = usage_filter(query);
    let read = |table: &str| -> BTreeMap<String, UsageTotals> {
        connection
            .prepare(&format!(
                "SELECT CAST({key_sql} AS TEXT), {} FROM {table}{where_sql} GROUP BY 1",
                zenith_relay_core::usage_total_columns_sql!()
            ))
            .unwrap()
            .query_map(params_from_iter(parameters.iter()), |row| {
                Ok((
                    row.get(0)?,
                    UsageTotals::from_sql_counts(|column| row.get(1 + column))?,
                ))
            })
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    };
    let requests = read("request_logs");
    read("usage_sends")
        .into_iter()
        .map(|(key, sends)| {
            let mut totals = request_totals(requests.get(&key).cloned().unwrap_or_default());
            apply_usage_totals_delta(&mut totals, consumption_totals(sends), true);
            (key, totals)
        })
        .collect()
}

fn assert_report_matches_full_view(
    connection: &Connection,
    query: &UsageQuery,
    page: &LocalUsagePage,
) {
    let reference = |key_sql: &str| {
        reference_full_view_groups(connection, query, key_sql)
            .into_iter()
            .map(|(key, totals)| (key, serde_json::to_value(totals).unwrap()))
            .collect::<BTreeMap<_, _>>()
    };
    for (groups, key) in [
        (
            &page.models,
            "COALESCE(resolved_model, requested_model, '')",
        ),
        (&page.pool_members, "COALESCE(account_id, source_id, '')"),
    ] {
        let actual: BTreeMap<_, _> = groups
            .iter()
            .map(|group| {
                let mut totals = group.totals.clone();
                totals.api_equivalent = Default::default();
                (group.key.clone(), serde_json::to_value(totals).unwrap())
            })
            .collect();
        assert_eq!(actual, reference(key), "{query:?}");
    }
    let actual: BTreeMap<_, _> = page
        .buckets
        .iter()
        .map(|bucket| {
            let mut totals = bucket.totals.clone();
            totals.api_equivalent = Default::default();
            (
                bucket.start_ms.to_string(),
                serde_json::to_value(totals).unwrap(),
            )
        })
        .collect();
    assert_eq!(
        actual,
        reference("(CAST(strftime('%s', created_at) AS INTEGER) * 1000 / 3600000) * 3600000"),
        "{query:?}"
    );
}

#[test]
fn first_entry_uses_narrow_count_and_unaggregated_indexes() {
    let root = crate::files::test_temp_root("relay-usage-plan");
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
        plan.contains("COVERING INDEX request_logs_usage_reads"),
        "{plan}"
    );
    let plan = explain(&format!(
        "SELECT rowid AS storage_id, 1 AS is_attempt, created_at, ABS(id) AS sort_id
         FROM request_attempts UNION ALL
         SELECT rowid, 0, created_at, ABS(id) FROM request_logs WHERE {}
         ORDER BY created_at DESC, sort_id DESC LIMIT 51",
        super::super::usage::LEGACY_SEND_PREDICATE,
    ));
    assert!(
        plan.contains("COVERING INDEX request_attempts_usage_reads"),
        "{plan}"
    );
    assert!(
        plan.contains("COVERING INDEX request_logs_usage_reads"),
        "{plan}"
    );
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    let plan = explain("SELECT COUNT(*) FROM usage_sends WHERE usage_aggregate_recorded = 0");
    assert!(plan.contains("request_logs_unaggregated"), "{plan}");
    assert!(plan.contains("request_attempts_unaggregated"), "{plan}");
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn weekly_value_projections_use_indexed_ledger_history() {
    let root = crate::files::test_temp_root("relay-quota-plan");
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let connection = database.connection.lock().unwrap();
    let parameters = [
        SqlValue::Text("account-plan-test".into()),
        SqlValue::Integer(i64::MAX),
    ];
    let plan = connection
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            super::super::weekly_capacity::PROJECTION_SQL
        ))
        .unwrap()
        .query_map(params_from_iter(parameters.iter()), |row| {
            row.get::<_, String>(3)
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
        .join("\n");
    assert!(
        plan.contains("SEARCH a USING PRIMARY KEY (account_id=?)"),
        "{plan}"
    );
    assert!(
        plan.contains("SEARCH h USING PRIMARY KEY (account_id=? AND observed_at_ms<?)"),
        "{plan}"
    );
    assert!(
        !plan.contains("SCAN") && !plan.contains("TEMP B-TREE"),
        "{plan}"
    );
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn usage_read_migration_preserves_history_and_consolidates_indexes() {
    let index_count = |connection: &Connection| {
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'index'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    };
    for version in [41, 43] {
        let root = crate::files::test_temp_root("relay-page-upgrade");
        let path = root.join("usage.sqlite");
        let database = TelemetryDb::open(&path).unwrap();
        for index in 0..3 {
            let mut event = aggregate_test_event(&format!("upgrade-{index}"), 1, 100, 0, None, 10);
            if index < 2 {
                event.account_id = Some("upgrade-account".into());
            }
            database.record(&event).unwrap();
        }
        let indexes_before = {
            let connection = database.connection.lock().unwrap();
            connection.execute_batch(
                "DROP INDEX request_attempts_account_usage_reads; DROP INDEX request_logs_account_usage_reads;
                 CREATE INDEX request_attempts_account_created ON request_attempts(account_id, created_at);
                 CREATE INDEX request_logs_account_created_idx ON request_logs(account_id, created_at);
                 DELETE FROM request_attempts WHERE request_id = 'upgrade-1';",
            ).unwrap();
            if version == 41 {
                connection.execute_batch(
                    "DROP INDEX request_attempts_usage_reads; DROP INDEX request_logs_usage_reads;
                     CREATE INDEX request_attempts_created_at ON request_attempts(created_at);
                     CREATE INDEX request_logs_send_history ON request_logs(created_at, request_id, attempt);",
                ).unwrap();
                connection.execute_batch(MIGRATION_039).unwrap();
            }
            connection
                .pragma_update(None, "user_version", version)
                .unwrap();
            index_count(&connection)
        };
        let before = database.usage_page(&UsageQuery::default()).unwrap();
        let expected = serde_json::to_value((&before.events, &before.totals)).unwrap();
        drop(database);
        for _ in 0..2 {
            let database = TelemetryDb::open(&path).unwrap();
            let after = database.usage_page(&UsageQuery::default()).unwrap();
            assert_eq!(
                serde_json::to_value((&after.events, &after.totals)).unwrap(),
                expected
            );
            assert_eq!(after.total, 3);
            let connection = database.connection.lock().unwrap();
            assert_eq!(
                connection
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                    .unwrap(),
                LOCAL_DATABASE_SCHEMA_VERSION
            );
            assert_eq!(
                index_count(&connection),
                indexes_before - if version == 41 { 2 } else { 0 }
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
#[ignore = "local first-entry timing probe; no live accounts or provider requests"]
fn measure_usage_first_entry() {
    let root = crate::files::test_temp_root("relay-usage-timing");
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
