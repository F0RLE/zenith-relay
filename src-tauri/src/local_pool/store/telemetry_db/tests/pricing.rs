use super::*;

#[test]
fn generic_cache_write_catalog_updates_reprice_history_without_a_ttl_migration() {
    let root = crate::files::test_temp_root("relay-generic-cache-price");
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    for (id, ttl) in [
        ("unspecified", None),
        ("provider-window", Some("17m".into())),
    ] {
        database
            .record(&aggregate_test_event(id, 1, 100, 40, ttl, 10))
            .unwrap();
    }
    let context = PricingContext::default();
    for (write_rate, expected) in [(0.0000025, 640), (0.0000031, 688)] {
        let catalog = PricingCatalog::from_litellm_payload(
            &serde_json::json!({"model":{
                "litellm_provider":"openai", "input_cost_per_token":0.000002,
                "output_cost_per_token":0.00001, "cache_creation_input_token_cost":write_rate
            }}),
            Some(format!("synthetic-price-{write_rate}")),
            Some(1),
            false,
        )
        .unwrap();
        let equivalents = database
            .api_equivalents_with_pricing(&catalog, &context)
            .unwrap();
        let value = equivalents.sources.get("source").unwrap();
        assert_eq!(value.micro_usd, expected);
        assert_eq!(value.unpriced_tokens, 0);
    }
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn context_band_sql_matches_request_boundaries() {
    use zenith_relay_core::usage::{USAGE_CONTEXT_BAND_SQL, UsageContextBand};
    let connection = Connection::open_in_memory().unwrap();
    let query = format!("SELECT {USAGE_CONTEXT_BAND_SQL} FROM (SELECT ?1 AS input_tokens)");
    for tokens in [None, Some(0)].into_iter().chain(
        [100_000, 128_000, 200_000, 272_000, 512_000]
            .into_iter()
            .flat_map(|boundary| [Some(boundary), Some(boundary + 1)]),
    ) {
        let label: String = connection
            .query_row(&query, [tokens.map(sql_u64)], |row| row.get(0))
            .unwrap();
        assert_eq!(
            label,
            UsageContextBand::from_input_tokens(tokens).as_label()
        );
    }
}

#[test]
fn context_band_migration_reclassifies_retained_usage_and_preserves_archives() {
    let root = crate::files::test_temp_root("relay-context-bands");
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    for id in ["retained", "archived"] {
        database
            .record(&aggregate_test_event(
                id,
                1,
                110_000,
                20,
                Some("5m".into()),
                50,
            ))
            .unwrap();
    }
    // Recreate schema 31's single base bucket for both requests; one raw log
    // has already been removed by retention and cannot be reclassified.
    database
        .lock_connection()
        .unwrap()
        .execute_batch(
            "DROP VIEW usage_sends;
         ALTER TABLE request_logs DROP COLUMN image_usage_json;
         DROP TABLE request_attempts;
         DROP TRIGGER request_attempts_retention;
         DELETE FROM request_logs WHERE request_id = 'archived';
         UPDATE usage_candidate_rollups SET context_band = 'base';
         DROP TABLE app_state_items;
         DROP TABLE weekly_capacity_anchors;
         DROP TABLE weekly_capacity_history;
         PRAGMA user_version = 31;",
        )
        .unwrap();
    drop(database);
    type Bands = Vec<(String, i64, i64)>;
    let read_bands = |database: &TelemetryDb| -> Bands {
        let connection = database.lock_connection().unwrap();
        let mut statement = connection
            .prepare(
                "SELECT context_band, input_tokens, cache_write_5m_tokens
             FROM usage_candidate_rollups WHERE input_samples > 0 ORDER BY context_band",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let database = TelemetryDb::open(&path).unwrap();
    assert_eq!(
        read_bands(&database),
        vec![
            ("above_100k".into(), 110_000, 20),
            ("base".into(), 110_000, 20)
        ]
    );
    database
        .record(&aggregate_test_event(
            "retained",
            2,
            130_000,
            30,
            Some("5m".into()),
            50,
        ))
        .unwrap();
    let expected = vec![
        ("above_100k".into(), 110_000, 20),
        ("above_128k".into(), 130_000, 30),
        ("base".into(), 110_000, 20),
    ];
    assert_eq!(read_bands(&database), expected);
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    assert_eq!(read_bands(&database), expected);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn api_equivalents_group_priced_and_unknown_usage_by_candidate() {
    let root = crate::files::test_temp_root("zenith-relay-usage-equivalent");
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let event = UsageEvent {
        request_id: "req_equivalent".into(),
        attempt: 1,
        local_key_id: "key_1".into(),
        source_id: "source_1".into(),
        candidate_id: Some("account_1".into()),
        account_id: Some("account_1".into()),
        account_token_generation: None,
        client_context_id: None,
        routing: None,
        requested_model: Some("gpt-5.4".into()),
        resolved_model: Some("gpt-5.4".into()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: WireApi::Responses,
        transport: zenith_relay_core::UsageTransport::Http,
        service_tier: DefaultServiceTier::Standard,
        applied_service_tier: None,
        success: true,
        http_status: 200,
        error_category: None,
        tool_use: ToolUseDiagnostics::default(),
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: Some(0),
        latency_ms: 12,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: Some(20),
        cached_input_tokens: Some(10),
        cache_write_input_tokens: None,
        cache_write_ttl: None,
        reasoning_tokens: Some(3),
        output_tokens: Some(8),
        total_tokens: Some(28),
        upstream_error: None,
        quota_snapshot: None,
        image_usage: None,
    };
    database.record(&event).unwrap();
    let report = database.usage_page(&UsageQuery::default()).unwrap();
    let row = &report.events[0];
    assert_eq!(row.api_equivalent.micro_usd, 148);
    assert_eq!(
        row.api_equivalent_breakdown
            .uncached_input
            .unwrap()
            .micro_usd,
        25
    );
    assert_eq!(
        row.api_equivalent_breakdown.cache_read.unwrap().micro_usd,
        3
    );
    assert_eq!(row.api_equivalent_breakdown.input.unwrap().micro_usd, 28);
    assert_eq!(row.api_equivalent_breakdown.output.unwrap().micro_usd, 120);
    let serialized = serde_json::to_value(row).unwrap();
    assert_eq!(
        serialized["apiEquivalentBreakdown"]["cacheRead"]["microUsd"],
        3
    );
    let equivalents = database.api_equivalents().unwrap();
    assert_eq!(
        equivalents.accounts.get("account_1"),
        Some(&ApiEquivalentSummary {
            micro_usd: 148,
            priced_tokens: 28,
            unpriced_tokens: 0,
        })
    );
    assert!(equivalents.sources.is_empty());
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn custom_price_revalues_existing_unknown_model_usage() {
    let root = crate::files::test_temp_root("zenith-relay-usage-custom-price");
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let event = private_model_source_event("req_custom_price", "source_1");
    database.record(&event).unwrap();

    assert_eq!(
        database
            .usage_page(&UsageQuery::default())
            .unwrap()
            .totals
            .api_equivalent
            .unpriced_tokens,
        1_100_000
    );
    let prices = BTreeMap::from([(
        "private-model".into(),
        ApiModelPriceOverride {
            input_micro_usd_per_million: 2_000_000,
            cached_input_micro_usd_per_million: Some(200_000),
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: 10_000_000,
            ..Default::default()
        },
    )]);
    let page = database
        .usage_page_with_price_overrides(&UsageQuery::default(), &prices, &BTreeMap::new())
        .unwrap();
    assert_eq!(page.totals.api_equivalent.micro_usd, 3_000_000);
    assert_eq!(page.totals.api_equivalent.priced_tokens, 1_100_000);
    assert_eq!(page.totals.api_equivalent.unpriced_tokens, 0);
    assert_eq!(page.events[0].api_equivalent, page.totals.api_equivalent);
    assert_eq!(
        database
            .api_equivalents_with_price_overrides(&prices, &BTreeMap::new())
            .unwrap()
            .sources
            .get("source_1")
            .map(|summary| summary.micro_usd),
        Some(3_000_000)
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn source_prices_are_applied_before_same_model_usage_is_merged() {
    let root = crate::files::test_temp_root("zenith-relay-usage-source-prices");
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let mut event = private_model_source_event("req_source_cheap", "source_cheap");
    database.record(&event).unwrap();
    event.request_id = "req_source_expensive".into();
    event.source_id = "source_expensive".into();
    event.candidate_id = Some("source_expensive".into());
    database.record(&event).unwrap();
    event.request_id = "req_account".into();
    event.source_id = "codex".into();
    event.candidate_id = Some("account_1".into());
    event.account_id = Some("account_1".into());
    database.record(&event).unwrap();

    let price = |input, output| ApiModelPriceOverride {
        input_micro_usd_per_million: input,
        cached_input_micro_usd_per_million: Some(input / 10),
        cache_write_5m_micro_usd_per_million: None,
        cache_write_1h_micro_usd_per_million: None,
        output_micro_usd_per_million: output,
        ..Default::default()
    };
    let source_prices = BTreeMap::from([
        (
            "source_cheap".into(),
            BTreeMap::from([(
                "private-model".into(),
                zenith_relay_core::ApiModelPriceSources {
                    provider: None,
                    manual: Some(price(1_000_000, 2_000_000)),
                },
            )]),
        ),
        (
            "source_expensive".into(),
            BTreeMap::from([(
                "private-model".into(),
                zenith_relay_core::ApiModelPriceSources {
                    provider: None,
                    manual: Some(price(2_000_000, 4_000_000)),
                },
            )]),
        ),
    ]);
    let page = database
        .usage_page_with_price_overrides(&UsageQuery::default(), &BTreeMap::new(), &source_prices)
        .unwrap();
    assert_eq!(page.totals.api_equivalent.micro_usd, 3_600_000);
    assert_eq!(page.totals.api_equivalent.unpriced_tokens, 1_100_000);
    assert_eq!(page.models[0].totals.api_equivalent.micro_usd, 3_600_000);
    let event_value = |request_id: &str| {
        page.events
            .iter()
            .find(|event| event.request_id == request_id)
            .unwrap()
            .api_equivalent
    };
    assert_eq!(event_value("req_source_cheap").micro_usd, 1_200_000);
    assert_eq!(event_value("req_source_expensive").micro_usd, 2_400_000);
    assert_eq!(event_value("req_account").unpriced_tokens, 1_100_000);

    let equivalents = database
        .api_equivalents_with_price_overrides(&BTreeMap::new(), &source_prices)
        .unwrap();
    assert_eq!(equivalents.sources["source_cheap"].micro_usd, 1_200_000);
    assert_eq!(equivalents.sources["source_expensive"].micro_usd, 2_400_000);
    assert_eq!(equivalents.accounts["account_1"].micro_usd, 0);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_speed_rollup_ignores_buffered_outliers() {
    let outlier = usage::usage_totals_from_sample(usage::UsageTotalsSample {
        success: true,
        latency_ms: 50,
        generation_ms: Some(10),
        output_tokens: Some(52),
        ..usage::UsageTotalsSample::default()
    });
    assert_eq!(outlier.generation_output_tokens, 0);
    assert_eq!(outlier.generation_samples, 0);
    assert_eq!(outlier.speed_output_tokens, 0);
    assert_eq!(outlier.speed_duration_ms, 0);

    let valid = usage::usage_totals_from_sample(usage::UsageTotalsSample {
        success: true,
        latency_ms: 100,
        generation_ms: Some(100),
        output_tokens: Some(11),
        ..usage::UsageTotalsSample::default()
    });
    assert_eq!(valid.generation_output_tokens, 10);
    assert_eq!(valid.generation_samples, 1);
    assert_eq!(valid.speed_output_tokens, 11);
    assert_eq!(valid.speed_duration_ms, 100);
}
#[test]
fn account_purchase_cost_migration_preserves_direct_values_and_removes_legacy_economics() {
    let root = crate::files::test_temp_root("zenith-relay-account-purchase-cost");
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    database
        .replace_state_json(&[(
            "accounts",
            r#"[
                    {
                        "account": { "id": "account_direct" },
                        "purchaseCostMicroUsd": 42000000,
                        "economics": { "purchaseCostMicroUsd": 13000000, "sampleCount": 8 }
                    },
                    {
                        "account": { "id": "account_legacy" },
                        "economics": { "purchaseCostMicroUsd": 21000000, "sampleCount": 3 }
                    },
                    {
                        "account": { "id": "account_null_direct" },
                        "purchaseCostMicroUsd": null,
                        "economics": { "purchaseCostMicroUsd": 25000000, "sampleCount": 4 }
                    },
                    {
                        "account": { "id": "account_without_cost" },
                        "economics": { "sampleCount": 1 }
                    }
                ]"#
            .to_string(),
        )])
        .unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "DROP VIEW usage_sends;
                 DROP INDEX request_logs_usage_reads;
                 DROP INDEX request_attempts_usage_reads;
                 DROP INDEX request_logs_account_usage_reads;
                 DROP INDEX request_attempts_account_usage_reads;
                 CREATE INDEX request_logs_account_created_idx ON request_logs(account_id, created_at);
                 ALTER TABLE request_logs DROP COLUMN image_usage_json;
                 DROP INDEX request_logs_unaggregated;
                 ALTER TABLE request_logs DROP COLUMN cache_write_ttl;
                 ALTER TABLE request_logs DROP COLUMN usage_aggregate_recorded;
                 ALTER TABLE request_logs DROP COLUMN client_context_id;
                 ALTER TABLE request_logs DROP COLUMN upstream_error_json;
                 ALTER TABLE request_logs DROP COLUMN transport;
                 DROP TABLE usage_candidate_rollups;
                 CREATE TABLE usage_candidate_rollups (
                     candidate_kind TEXT NOT NULL,
                     candidate_id TEXT NOT NULL,
                     model TEXT NOT NULL,
                     input_tokens INTEGER NOT NULL DEFAULT 0,
                     input_samples INTEGER NOT NULL DEFAULT 0,
                     cached_input_tokens INTEGER NOT NULL DEFAULT 0,
                     cached_input_samples INTEGER NOT NULL DEFAULT 0,
                     cache_write_input_tokens INTEGER NOT NULL DEFAULT 0,
                     cache_write_input_samples INTEGER NOT NULL DEFAULT 0,
                     output_tokens INTEGER NOT NULL DEFAULT 0,
                     output_samples INTEGER NOT NULL DEFAULT 0,
                     total_tokens INTEGER NOT NULL DEFAULT 0,
                     total_samples INTEGER NOT NULL DEFAULT 0,
                     PRIMARY KEY(candidate_kind, candidate_id, model)
                 ) WITHOUT ROWID;",
        )
        .unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .pragma_update(None, "user_version", 23)
        .unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE request_attempts; DROP TRIGGER request_attempts_retention; DROP TABLE app_state_items; DROP TABLE weekly_capacity_anchors; DROP TABLE weekly_capacity_history;")
        .unwrap();
    drop(database);

    let database = TelemetryDb::open(&path).unwrap();
    let accounts: serde_json::Value =
        serde_json::from_str(database.state_json("accounts").unwrap().as_deref().unwrap()).unwrap();
    let accounts = accounts.as_array().unwrap();
    assert_eq!(
        accounts
            .iter()
            .map(|account| account["account"]["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "account_direct",
            "account_legacy",
            "account_null_direct",
            "account_without_cost",
        ]
    );
    assert_eq!(accounts[0]["purchaseCostMicroUsd"], 42_000_000);
    assert_eq!(accounts[1]["purchaseCostMicroUsd"], 21_000_000);
    assert_eq!(accounts[2]["purchaseCostMicroUsd"], 25_000_000);
    assert!(accounts[3].get("purchaseCostMicroUsd").is_none());
    assert!(
        accounts
            .iter()
            .all(|account| account.get("economics").is_none())
    );
    let version: u32 = database
        .connection
        .lock()
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, LOCAL_DATABASE_SCHEMA_VERSION);
    drop(database);

    let reopened = TelemetryDb::open(&path).unwrap();
    let accounts: serde_json::Value =
        serde_json::from_str(reopened.state_json("accounts").unwrap().as_deref().unwrap()).unwrap();
    assert_eq!(accounts[0]["purchaseCostMicroUsd"], 42_000_000);
    assert_eq!(accounts[1]["purchaseCostMicroUsd"], 21_000_000);
    assert_eq!(accounts[2]["purchaseCostMicroUsd"], 25_000_000);
    assert!(
        accounts
            .as_array()
            .unwrap()
            .iter()
            .all(|account| account.get("economics").is_none())
    );
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
