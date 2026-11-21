use super::*;

#[test]
fn image_usage_keeps_consumption_but_does_not_change_text_speed_or_ttft() {
    let root = crate::files::test_temp_root("relay-image-text-timing");
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let read_totals = || {
        let cached = database.usage_page(&UsageQuery::default()).unwrap().totals;
        database.clear_cached_usage_totals().unwrap();
        let scanned = database.usage_page(&UsageQuery::default()).unwrap().totals;
        assert_eq!(cached, scanned);
        scanned
    };
    let mut text = aggregate_test_event("text", 1, 100, 0, None, 20);
    text.latency_ms = 1_000;
    text.ttft_ms = Some(100);
    text.generation_ms = Some(500);
    database.record(&text).unwrap();
    let baseline = read_totals();
    let mut image = aggregate_test_event("image", 1, 45, 0, None, 4_000);
    image.latency_ms = 10_000;
    image.ttft_ms = Some(25);
    image.generation_ms = Some(4_500);
    image.image_usage = Some(zenith_relay_core::usage::ImageUsage {
        generated_images: 1,
        output_image_tokens: Some(4_000),
        ..Default::default()
    });
    database.record(&image).unwrap();
    database.record(&image).unwrap();
    let totals = read_totals();
    assert_eq!(totals.requests, 2);
    assert_eq!(totals.total_tokens, 4_165);
    assert_eq!(totals.latency_ms, 11_000);
    assert_eq!(totals.ttft_samples, 1);
    assert_eq!(totals.ttft_ms, baseline.ttft_ms);
    assert_eq!(totals.generation_samples, 1);
    assert_eq!(totals.generation_ms, baseline.generation_ms);
    assert_eq!(
        totals.generation_output_tokens,
        baseline.generation_output_tokens
    );
    assert_eq!(totals.speed_output_tokens, baseline.speed_output_tokens);
    assert_eq!(totals.speed_duration_ms, baseline.speed_duration_ms);

    // Late modality evidence must remove the old timing contribution as well.
    text.image_usage = Some(zenith_relay_core::usage::ImageUsage::default());
    database.record(&text).unwrap();
    let images_only = read_totals();
    assert_eq!(images_only.total_tokens, 4_165);
    assert_eq!(images_only.ttft_samples, 0);
    assert_eq!(images_only.ttft_ms, 0);
    assert_eq!(images_only.generation_samples, 0);
    assert_eq!(images_only.generation_ms, 0);
    assert_eq!(images_only.speed_output_tokens, 0);
    assert_eq!(images_only.speed_duration_ms, 0);
    text.image_usage = None;
    database.record(&text).unwrap();
    assert_eq!(read_totals(), totals);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

fn image_price_catalog() -> PricingCatalog {
    PricingCatalog::from_litellm_json(
        r#"{
        "gpt-image-test": {"litellm_provider":"openai", "input_cost_per_token":0.000005,
            "input_cost_per_image_token":0.000008, "output_cost_per_image_token":0.00003}
    }"#,
    )
    .unwrap()
}

#[test]
fn image_timing_migration_backfills_retained_history_without_changing_usage() {
    let root = crate::files::test_temp_root("relay-image-timing-migration");
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    let mut event = aggregate_test_event("retained-image", 1, 45, 0, None, 515);
    event.latency_ms = 1_000;
    event.ttft_ms = Some(30);
    event.image_usage = Some(zenith_relay_core::usage::ImageUsage {
        generated_images: 1,
        output_image_tokens: Some(515),
        ..Default::default()
    });
    database.record(&event).unwrap();
    database
        .record(&aggregate_test_event("retained-text", 1, 10, 0, None, 20))
        .unwrap();
    let expected = database.usage_page(&UsageQuery::default()).unwrap().totals;
    database.connection.lock().unwrap().execute_batch(
        "DROP VIEW usage_sends;
         DROP INDEX request_logs_usage_reads;
         DROP INDEX request_attempts_usage_reads;
         ALTER TABLE request_logs DROP COLUMN is_image_usage;
         ALTER TABLE request_attempts DROP COLUMN is_image_usage;
         CREATE VIEW usage_sends AS SELECT * FROM request_attempts UNION ALL
             SELECT * FROM request_logs WHERE NOT EXISTS (SELECT 1 FROM request_attempts
                 WHERE request_attempts.request_id=request_logs.request_id AND request_attempts.attempt=request_logs.attempt);
         PRAGMA user_version=47;"
    ).unwrap();
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    assert_eq!(
        database.usage_page(&UsageQuery::default()).unwrap().totals,
        expected
    );
    let connection = database.lock_connection().unwrap();
    for table in ["request_logs", "request_attempts"] {
        let (count, images): (i64, i64) = connection
            .query_row(
                &format!("SELECT COUNT(*), SUM(is_image_usage) FROM {table}"),
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((count, images), (2, 1));
    }
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn image_prices_agree_across_rows_reports_rollups_and_reopen() {
    let root = crate::files::test_temp_root("relay-image-pricing");
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    let mut event = aggregate_test_event("image-price", 1, 45, 0, None, 515);
    event.account_id = Some("synthetic-image-account".into());
    event.requested_model = Some("gpt-image-test".into());
    event.resolved_model = event.requested_model.clone();
    event.cached_input_tokens = None;
    event.image_usage = Some(zenith_relay_core::usage::ImageUsage {
        input_text_tokens: Some(45),
        input_image_tokens: Some(0),
        output_image_tokens: Some(515),
        output_text_tokens: Some(0),
        generated_images: 1,
        ..Default::default()
    });
    let catalog = image_price_catalog();
    let context = PricingContext::default();
    database.record(&event).unwrap();
    database.record(&event).unwrap();
    for query in [
        UsageQuery::default(),
        UsageQuery {
            request_id: Some("image-price".into()),
            from_ms: Some(0),
            bucket_ms: Some(60_000),
            ..Default::default()
        },
    ] {
        let page = database
            .usage_page_with_pricing(&query, &catalog, &context)
            .unwrap();
        assert_eq!(page.events[0].api_equivalent.micro_usd, 15_675);
        assert_eq!(page.totals.api_equivalent.micro_usd, 15_675);
        assert_eq!(page.totals.api_equivalent.priced_tokens, 560);
        assert_eq!(page.totals.api_equivalent.unpriced_tokens, 0);
        if query.bucket_ms.is_some() {
            assert_eq!(page.buckets[0].totals.api_equivalent.micro_usd, 15_675);
        }
    }
    assert_eq!(
        database
            .api_equivalents_with_pricing(&catalog, &context)
            .unwrap()
            .accounts["synthetic-image-account"]
            .micro_usd,
        15_675
    );
    // Recreate the new migration boundary with retained pre-upgrade usage.
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE usage_image_price_rollups; PRAGMA user_version=46;")
        .unwrap();
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    assert_eq!(
        database
            .api_equivalents_with_pricing(&catalog, &context)
            .unwrap()
            .accounts["synthetic-image-account"]
            .micro_usd,
        15_675
    );
    event.input_tokens = Some(40);
    event.total_tokens = Some(555);
    event.image_usage.as_mut().unwrap().input_text_tokens = Some(40);
    database.record(&event).unwrap();
    database.record(&event).unwrap();
    assert_eq!(
        database
            .api_equivalents_with_pricing(&catalog, &context)
            .unwrap()
            .accounts["synthetic-image-account"]
            .micro_usd,
        15_650
    );
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch("UPDATE request_logs SET created_at=datetime('now','-31 days');")
        .unwrap();
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    assert!(database.list(10).unwrap().is_empty());
    assert_eq!(
        database
            .api_equivalents_with_pricing(&catalog, &context)
            .unwrap()
            .accounts["synthetic-image-account"]
            .micro_usd,
        15_650
    );
    let revised = PricingCatalog::from_litellm_json(r#"{"gpt-image-test":{"litellm_provider":"openai","input_cost_per_token":0.00001,"output_cost_per_image_token":0.00006}}"#).unwrap();
    let mut revised = revised;
    revised.revision = Some("synthetic-repriced".into());
    assert_eq!(
        database
            .api_equivalents_with_pricing(&revised, &context)
            .unwrap()
            .accounts["synthetic-image-account"]
            .micro_usd,
        31_300
    );
    database
        .replace_state_json_and_delete_account_data(&[], "synthetic-image-account")
        .unwrap();
    assert!(
        database
            .api_equivalents_with_pricing(&catalog, &context)
            .unwrap()
            .accounts
            .is_empty()
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn image_usage_survives_failure_reopen_and_duplicate_observation() {
    let root = crate::files::test_temp_root("relay-image-usage");
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    let mut event = aggregate_test_event("image-request", 1, 13, 0, None, 10);
    event.requested_model = Some("gpt-image-test".into());
    event.resolved_model = event.requested_model.clone();
    event.success = false;
    event.http_status = 502;
    event.error_category = Some("client_cancelled".into());
    event.cached_input_tokens = None;
    event.image_usage = Some(zenith_relay_core::usage::ImageUsage {
        input_text_tokens: Some(3),
        input_image_tokens: Some(7),
        cached_input_tokens: Some(8),
        output_image_tokens: Some(4),
        output_text_tokens: Some(1),
        svg_usage: Some(zenith_relay_core::usage::SvgUsage {
            model: Some("synthetic-svg-author".into()),
            input_tokens: Some(3),
            output_tokens: Some(5),
            reasoning_tokens: Some(2),
            total_tokens: Some(8),
        }),
        input_details_consistent: Some(true),
        generated_images: 1,
        ..Default::default()
    });
    database.record(&event).unwrap();
    database.record(&event).unwrap();
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    let rows = database.list(10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].image_usage, event.image_usage);
    assert_eq!(rows[0].total_tokens, Some(23));
    assert_eq!(rows[0].cached_input_tokens, None);
    let report = database.request_diagnostics("image-request").unwrap();
    assert_eq!(
        report["attempts"][0]["usage"]["imageUsage"]["inputImageTokens"],
        7
    );
    assert_eq!(
        report["summary"]["consumption"]["totalTokens"]["observedTotal"],
        23
    );
    assert_eq!(
        report["attempts"][0]["usage"]["imageUsage"]["svgUsage"]["totalTokens"],
        8
    );
    assert_eq!(report["summary"]["includedSends"], 1);
    let connection = database.connection.lock().unwrap();
    let sends: i64 = connection
        .query_row("SELECT COUNT(*) FROM usage_sends", [], |row| row.get(0))
        .unwrap();
    assert_eq!(sends, 1);
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn image_usage_migration_preserves_legacy_rows_and_send_view() {
    let root = crate::files::test_temp_root("relay-image-migration");
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    database
        .record(&aggregate_test_event("legacy", 1, 2, 0, None, 3))
        .unwrap();
    database.connection.lock().unwrap().execute_batch(
        "DROP VIEW usage_sends;
         ALTER TABLE request_logs DROP COLUMN image_usage_json;
         ALTER TABLE request_attempts DROP COLUMN image_usage_json;
         CREATE VIEW usage_sends AS SELECT * FROM request_attempts UNION ALL
             SELECT * FROM request_logs WHERE NOT EXISTS (SELECT 1 FROM request_attempts
                 WHERE request_attempts.request_id=request_logs.request_id AND request_attempts.attempt=request_logs.attempt);
         DROP TABLE app_state_items;
         DROP TABLE weekly_capacity_anchors;
         DROP TABLE weekly_capacity_history;
         PRAGMA user_version=37;"
    ).unwrap();
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    let rows = database.list(10).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].image_usage.is_none());
    assert_eq!(rows[0].total_tokens, Some(5));
    let connection = database.connection.lock().unwrap();
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, LOCAL_DATABASE_SCHEMA_VERSION);
    let count: u32 = connection
        .query_row("SELECT COUNT(*) FROM usage_sends", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
