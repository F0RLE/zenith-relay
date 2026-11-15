use super::*;

#[test]
fn split_totals_projections_preserve_counts_nulls_and_speed_guards() {
    use super::super::usage::{consumption_totals, request_totals};

    let connection = Connection::open_in_memory().unwrap();
    let read = |columns: &str, filter: &str| {
        connection
            .query_row(
                &format!(
                    "WITH samples(success, latency_ms, ttft_ms, generation_ms,
             input_tokens, cached_input_tokens, cache_write_input_tokens,
             reasoning_tokens, output_tokens, total_tokens) AS (VALUES
             (1, 100, 10, 50, 100, 50, 10, 1, 20, 120),
             (0, 200, NULL, 100, 50, NULL, NULL, NULL, 5, 55),
             (1, 0, NULL, 0, NULL, 0, 0, 0, 0, 0),
             (1, 10, 0, 1, 10, 5, 0, 20, 5, 15),
             (1, 1, 0, 1, 1, 0, 0, 0, 1000, 1001),
             (1, 100, 20, 20, 0, NULL, 1, NULL, 1, NULL))
             SELECT {columns} FROM samples{filter}"
                ),
                [],
                |row| UsageTotals::from_sql_counts(|column| row.get(column)),
            )
            .unwrap()
    };
    for filter in [
        "",
        " WHERE success = 0",
        " WHERE input_tokens IS NULL",
        " WHERE 0",
    ] {
        let full = read(zenith_relay_core::usage_total_columns_sql!(), filter);
        let expected_request = request_totals(full.clone());
        let mut expected_consumption = consumption_totals(full);
        expected_consumption.requests = expected_request.requests;
        for (actual, expected) in [
            (
                read(zenith_relay_core::usage_total_columns_sql!(logical), filter),
                expected_request,
            ),
            (
                read(
                    zenith_relay_core::usage_total_columns_sql!(physical),
                    filter,
                ),
                expected_consumption,
            ),
        ] {
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                serde_json::to_value(expected).unwrap(),
                "{filter}"
            );
        }
    }
}

#[test]
fn report_aggregates_match_independent_scans_with_retries_filters_and_price_rounding() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    for index in 0..36 {
        let mut event = aggregate_test_event(
            &format!("synthetic-summary-{index}"),
            1,
            1 + index * 7,
            3,
            Some("5m".into()),
            2 + index,
        );
        event.source_id = format!("source-{}", index % 3);
        event.account_id = (index % 2 == 0).then(|| format!("account-{}", index % 4));
        event.resolved_model = Some(
            if index % 5 == 0 {
                "unknown-model"
            } else {
                "gpt-5.4"
            }
            .into(),
        );
        event.cached_input_tokens = (index % 3 == 0).then_some(1);
        event.reasoning_tokens = (index % 4 == 0).then_some(2);
        event.applied_service_tier =
            Some(["standard", "priority", "flex", "future_tier"][index as usize % 4].into());
        if index % 13 == 0 {
            event.input_tokens = Some(300_000);
            event.total_tokens = Some(300_000 + 2 + index);
        }
        event.ttft_ms = Some(10);
        event.latency_ms = 100;
        database.record(&event).unwrap();
        if index % 3 == 0 {
            event.attempt = 2;
            event.success = false;
            event.http_status = 503;
            event.source_id = "retried-source".into();
            event.account_id = None;
            database.record(&event).unwrap();
        }
    }
    let catalog = test_pricing_catalog();
    let context = test_pricing_context(&BTreeMap::new(), &BTreeMap::new());
    let resolver = CatalogPriceResolver::new(&catalog, &context);
    let connection = database.connection.lock().unwrap();
    connection.execute_batch(
        "UPDATE request_logs SET created_at = datetime('2026-01-01', '+' || (id % 4) || ' hours');
         UPDATE request_attempts SET created_at = (SELECT created_at FROM request_logs WHERE request_logs.request_id = request_attempts.request_id);
         UPDATE request_attempts SET created_at = datetime(created_at, '-1 hour')
         WHERE attempt = 1 AND request_id IN (SELECT request_id FROM request_logs WHERE attempt = 2);
         DELETE FROM request_attempts WHERE request_id = 'synthetic-summary-1';
         DELETE FROM request_attempts WHERE request_id = 'synthetic-summary-0' AND attempt = 2;",
    ).unwrap();
    let start_ms = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .timestamp_millis() as u64;
    for mut query in [
        UsageQuery {
            from_ms: Some(1),
            ..Default::default()
        },
        UsageQuery {
            source_or_account_query: Some("account".into()),
            ..Default::default()
        },
        UsageQuery {
            model_query: Some("gpt".into()),
            ..Default::default()
        },
        UsageQuery {
            success: Some(false),
            ..Default::default()
        },
        UsageQuery {
            request_id_query: Some("absent".into()),
            ..Default::default()
        },
        UsageQuery {
            from_ms: Some(start_ms + 123_456),
            to_ms: Some(start_ms + 7_200_000),
            ..Default::default()
        },
    ] {
        for bucket_ms in [None, Some(3_600_000), Some(900_000)] {
            query.bucket_ms = bucket_ms;
            assert_buckets_match_independent_windows(&connection, &query, &resolver);
        }
        for groups in [true, false] {
            query.include_models = Some(groups);
            query.include_pool_members = Some(groups);
            let (filter, parameters) = usage_filter(&query);
            let actual =
                usage::scoped_report_summary(&connection, &query, &filter, &parameters, &resolver)
                    .unwrap();
            let equivalents =
                usage_model_equivalents(&connection, &filter, &parameters, &resolver, false)
                    .unwrap();
            let mut totals = usage_totals(&connection, &filter, &parameters).unwrap();
            for equivalent in equivalents.models.values() {
                totals.api_equivalent.merge(*equivalent);
            }
            assert_eq!(actual.totals, totals);
            for (key, by_key, actual) in [
                (
                    "COALESCE(resolved_model, requested_model, '')",
                    &equivalents.models,
                    &actual.models,
                ),
                (
                    "COALESCE(account_id, source_id, '')",
                    &equivalents.members,
                    &actual.members,
                ),
            ] {
                let mut expected = if groups {
                    usage_groups(&connection, &filter, &parameters, key).unwrap()
                } else {
                    Vec::new()
                };
                for group in &mut expected {
                    group.totals.api_equivalent =
                        by_key.get(&group.key).copied().unwrap_or_default();
                }
                assert_eq!(*actual, expected);
            }
            assert_eq!(
                resolver.pricing_metadata(totals.api_equivalent, &actual.sources),
                resolver.pricing_metadata(totals.api_equivalent, &equivalents.sources)
            );
            let sends = usage::send_projection("1", &filter);
            let expected_count: i64 = connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM ({sends})"),
                    params_from_iter(parameters.iter().chain(parameters.iter())),
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(actual.sends, expected_count as u64);
        }
    }
}

fn assert_buckets_match_independent_windows(
    connection: &Connection,
    query: &UsageQuery,
    resolver: &CatalogPriceResolver<'_>,
) {
    let (filter, parameters) = usage_filter(query);
    let actual = usage::usage_buckets(connection, &filter, &parameters, query, resolver).unwrap();
    let Some(bucket_ms) = query.bucket_ms else {
        assert!(actual.is_empty());
        return;
    };
    let start = SqlValue::Integer(query.from_ms.unwrap_or_default() as i64);
    let bucket = SqlValue::Integer(bucket_ms as i64);
    let bucket_parameters = [start.clone(), start, bucket.clone(), bucket];
    let bucket_sql = "? + ((CAST(strftime('%s', created_at) AS INTEGER) * 1000 - ?) / ?) * ?";
    let keys = connection
        .prepare(&format!(
            "SELECT DISTINCT {bucket_sql} FROM usage_sends{filter} ORDER BY 1"
        ))
        .unwrap()
        .query_map(
            params_from_iter(bucket_parameters.iter().chain(parameters.iter())),
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let mut expected = Vec::new();
    for key in keys {
        let conjunction = if filter.is_empty() { " WHERE" } else { " AND" };
        let bucket_filter = format!("{filter}{conjunction} {bucket_sql} = ?");
        let mut window_parameters = parameters.clone();
        window_parameters.extend_from_slice(&bucket_parameters);
        window_parameters.push(SqlValue::Integer(key));
        let mut totals = usage_totals(connection, &bucket_filter, &window_parameters).unwrap();
        let equivalents = usage_model_equivalents(
            connection,
            &bucket_filter,
            &window_parameters,
            resolver,
            false,
        )
        .unwrap();
        for equivalent in equivalents.models.values() {
            totals.api_equivalent.merge(*equivalent);
        }
        expected.push(zenith_relay_core::protocol::UsageBucket {
            start_ms: key as u64,
            totals,
        });
    }
    assert_eq!(actual, expected, "{query:?}");
}
