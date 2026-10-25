use super::*;

#[test]
fn fused_summary_matches_independent_scans_with_retries_filters_and_price_rounding() {
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
    ] {
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
