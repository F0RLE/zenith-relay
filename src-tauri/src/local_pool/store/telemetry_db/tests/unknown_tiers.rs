use super::*;
use zenith_relay_core::usage::{UsagePriceClass, USAGE_PRICE_CLASS_SQL};

#[test]
fn observed_tier_sql_matches_rust_without_a_standard_fallback_for_new_names() {
    let connection = Connection::open_in_memory().unwrap();
    let sql = format!("SELECT {USAGE_PRICE_CLASS_SQL} FROM (SELECT ?1 AS applied_service_tier)");
    for tier in [
        None,
        Some(""),
        Some("default"),
        Some(" Standard "),
        Some("flex"),
        Some("fast"),
        Some("priority"),
        Some(" ULTRAFAST "),
        Some("auto"),
        Some("premium_v2"),
        Some("batch"),
        Some("scale"),
        Some("unknown"),
    ] {
        let class: String = connection
            .query_row(&sql, [tier], |row| row.get(0))
            .unwrap();
        assert_eq!(class, UsagePriceClass::observed_label(tier), "{tier:?}");
        assert_eq!(
            UsagePriceClass::from_label(&class),
            UsagePriceClass::from_observed(tier)
        );
    }
}

#[test]
fn unknown_tier_migration_reclassifies_retained_events_and_preserves_archives() {
    let root = std::env::temp_dir().join(format!("relay-unknown-tiers-{}", uuid::Uuid::new_v4()));
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    let mut event = aggregate_test_event("unknown-tier", 1, 150_001, 0, None, 10);
    event.source_id = "source_unknown".into();
    event.applied_service_tier = Some("premium_v2".into());
    database.record(&event).unwrap();
    let mut standard = aggregate_test_event("standard-tier", 1, 50, 0, None, 5);
    standard.source_id = "source_known".into();
    database.record(&standard).unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "DROP VIEW usage_sends;
         ALTER TABLE request_logs DROP COLUMN image_usage_json;
         ALTER TABLE request_attempts DROP COLUMN image_usage_json;
         DROP INDEX request_attempts_created_at;
         DROP INDEX request_attempts_account_created;
         UPDATE usage_candidate_rollups SET price_class = 'standard',
            input_tokens = input_tokens + 700, input_samples = input_samples + 1,
            output_tokens = output_tokens + 7, output_samples = output_samples + 1,
            total_tokens = total_tokens + 707, total_samples = total_samples + 1
         WHERE candidate_id = 'source_unknown' AND price_class = 'premium_v2';
         DROP TABLE app_state_items;
         DROP TABLE weekly_capacity_anchors;
         DROP TABLE weekly_capacity_history;
         PRAGMA user_version = 34;",
        )
        .unwrap();
    drop(database);

    let catalog = zenith_relay_core::PricingCatalog::from_litellm_json(
        r#"{"model":{"litellm_provider":"openai","input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}"#,
    ).unwrap();
    for _ in 0..2 {
        let reopened = TelemetryDb::open(&path).unwrap();
        // Updating the same observed event must subtract its current tier key,
        // not a historical Standard key, and must not double-count its tokens.
        reopened.record(&event).unwrap();
        let context = PricingContext::default();
        let equivalents = reopened
            .api_equivalents_with_pricing(&catalog, &context)
            .unwrap();
        assert_eq!(equivalents.sources["source_unknown"].micro_usd, 714);
        assert_eq!(equivalents.sources["source_unknown"].priced_tokens, 707);
        assert_eq!(
            equivalents.sources["source_unknown"].unpriced_tokens,
            150_011
        );
        assert_eq!(equivalents.sources["source_known"].micro_usd, 60);
        let connection = reopened.connection.lock().unwrap();
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, LOCAL_DATABASE_SCHEMA_VERSION);
        let events: u32 = connection
            .query_row("SELECT count(*) FROM request_logs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(events, 2);
        let negative: u32 = connection.query_row(
            "SELECT count(*) FROM usage_candidate_rollups WHERE input_tokens < 0 OR output_tokens < 0 OR total_tokens < 0",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(negative, 0);
    }
    std::fs::remove_dir_all(root).unwrap();
}
