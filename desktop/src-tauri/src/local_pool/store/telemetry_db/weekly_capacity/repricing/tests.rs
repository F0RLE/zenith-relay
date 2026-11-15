use super::super::tests::{NOW, database, usage, window};
use super::*;

const LEDGER: &str = "synthetic-ledger";

fn catalog(multiplier: f64, unrelated_change: bool) -> PricingCatalog {
    let mut payload = serde_json::json!({"model":{
        "litellm_provider":"openai", "input_cost_per_token":0.000002 * multiplier,
        "cache_read_input_token_cost":0.0000005 * multiplier,
        "output_cost_per_token":0.00001 * multiplier,
    }});
    if unrelated_change {
        payload["model"]["description"] = "Changed non-price metadata".into();
        payload["unrelated-model"] = serde_json::json!({
            "litellm_provider":"openai", "input_cost_per_token":0.001,
            "output_cost_per_token":0.002,
        });
    }
    priced_catalog(payload)
}

fn priced_catalog(payload: serde_json::Value) -> PricingCatalog {
    let revision = fingerprint(&payload.to_string());
    PricingCatalog::from_litellm_payload(&payload, Some(revision), Some(1), false).unwrap()
}

fn observe(database: &TelemetryDb, catalog: &PricingCatalog, quota: &QuotaWindow, now: u64) {
    let pricing = PricingContext::default();
    database
        .observe_weekly_capacity(
            CapacityObservation {
                ledger: LEDGER,
                account_ids: &["a".into(), "b".into()],
                plan: "plus",
                window: quota,
                pricing_revision: &pricing.revision_key(catalog),
                now_ms: now,
            },
            catalog,
            &pricing,
        )
        .unwrap();
}

fn projected(
    database: &TelemetryDb,
    catalog: &PricingCatalog,
    quota: &QuotaWindow,
    now: u64,
) -> Option<zenith_relay_core::protocol::WeeklyValueCalibration> {
    database
        .weekly_value_projections(&[CapacityObservation {
            ledger: LEDGER,
            account_ids: &["a".into(), "b".into()],
            plan: "plus",
            window: quota,
            pricing_revision: &PricingContext::default().revision_key(catalog),
            now_ms: now,
        }])
        .unwrap()
        .remove("a")
        .map(|value| value.calibration)
}

fn seeded() -> (std::path::PathBuf, TelemetryDb, PricingCatalog, QuotaWindow) {
    let (root, database) = database();
    let catalog = catalog(1.0, false);
    observe(&database, &catalog, &window(NOW, 10_000), NOW);
    usage(&database, "one", "a", NOW + 30_000, Some(80));
    let quota = window(NOW + 60_000, 9500);
    observe(&database, &catalog, &quota, NOW + 60_000);
    assert_eq!(
        projected(&database, &catalog, &quota, NOW + 60_000)
            .unwrap()
            .median_micro_usd,
        Some(5600)
    );
    (root, database, catalog, quota)
}

#[test]
fn irrelevant_catalog_edits_keep_capacity_and_do_not_invent_new_measurements() {
    let (root, database, old, quota) = seeded();
    let before = projected(&database, &old, &quota, NOW + 60_000).unwrap();
    let changed = catalog(1.0, true);
    assert!(projected(&database, &changed, &quota, NOW + 120_000).is_none());
    observe(&database, &changed, &quota, NOW + 120_000);
    let after = projected(&database, &changed, &quota, NOW + 120_000).unwrap();
    assert_eq!(after.median_micro_usd, before.median_micro_usd);
    assert_eq!(after.observed_at_ms, before.observed_at_ms);
    assert_eq!(after.valid_until_ms, before.valid_until_ms);
    assert_eq!(after.account_count, 1);
    assert_eq!(
        database
            .lock_connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM weekly_capacity_history", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
    let revision = database.weekly_capacity_revision();
    observe(&database, &changed, &quota, NOW + 180_000);
    assert_eq!(database.weekly_capacity_revision(), revision);
    drop(database);
    let database = TelemetryDb::open(&root.join("test.sqlite")).unwrap();
    // Snapshot projection and an unchanged observation need no raw-send query,
    // including after restart when the in-memory coalescing map is empty.
    database
        .lock_connection()
        .unwrap()
        .execute_batch("DROP VIEW usage_sends")
        .unwrap();
    observe(&database, &changed, &quota, NOW + 180_000);
    assert_eq!(
        projected(&database, &changed, &quota, NOW + 180_000)
            .unwrap()
            .median_micro_usd,
        Some(5600)
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn relevant_prices_revalue_the_same_interval_without_refreshing_its_age_or_history() {
    let (root, database, old, quota) = seeded();
    let before = projected(&database, &old, &quota, NOW + 60_000).unwrap();
    let doubled = catalog(2.0, false);
    observe(&database, &doubled, &quota, NOW + HOUR_MS);
    let after = projected(&database, &doubled, &quota, NOW + HOUR_MS).unwrap();
    assert_eq!(after.median_micro_usd, Some(11_200));
    assert_eq!(after.observed_at_ms, before.observed_at_ms);
    assert_eq!(after.valid_until_ms, before.valid_until_ms);
    let history: String = database
        .lock_connection()
        .unwrap()
        .query_row(
            "SELECT payload_json FROM weekly_capacity_history",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let sample: Measurement = serde_json::from_str(&history).unwrap();
    assert_eq!(sample.micro_usd, Some(5600));
    assert_eq!(sample.from_ms, NOW);
    assert!(projected(&database, &doubled, &quota, NOW + MAX_ANCHOR_AGE + 1).is_none());
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn catalog_change_preserves_an_unfinished_five_percent_quota_interval() {
    let (root, database) = database();
    let old = catalog(1.0, false);
    observe(&database, &old, &window(NOW, 10_000), NOW);
    usage(&database, "first", "a", NOW + 30_000, Some(80));
    let partial = window(NOW + 60_000, 9800);
    observe(&database, &old, &partial, NOW + 60_000);
    let doubled = catalog(2.0, false);
    observe(&database, &doubled, &partial, NOW + 120_000);
    usage(&database, "second", "a", NOW + 150_000, Some(80));
    let complete = window(NOW + 180_000, 9500);
    observe(&database, &doubled, &complete, NOW + 180_000);
    assert_eq!(
        projected(&database, &doubled, &complete, NOW + 180_000)
            .unwrap()
            .median_micro_usd,
        Some(22_400)
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_pricing_events_are_recovered_but_structural_events_are_not_skipped() {
    for event in [
        CapacityEventKind::PricingChanged,
        CapacityEventKind::Reset,
        CapacityEventKind::PlanChanged,
        CapacityEventKind::Discontinuity,
    ] {
        let (root, database, _, quota) = seeded();
        let changed = catalog(2.0, false);
        let current = window(NOW + 120_000, 9500);
        {
            let connection = database.lock_connection().unwrap();
            let mut anchor: Anchor =
                serde_json::from_str(&stored_anchor(&connection, LEDGER).unwrap().unwrap())
                    .unwrap();
            let mut sample = anchor.calibration.take().unwrap().sample.unwrap();
            sample.usage_fingerprint = None;
            connection
                .execute(
                    "UPDATE weekly_capacity_history SET payload_json = ?1",
                    [serde_json::to_string(&sample).unwrap()],
                )
                .unwrap();
            anchor.window = current.clone();
            anchor.from_ms = current.observed_at_ms;
            anchor.remaining = 9500;
            anchor.pricing_revision =
                fingerprint(&PricingContext::default().revision_key(&changed));
            connection
                .execute(
                    "UPDATE weekly_capacity_anchors SET observed_at_ms = ?1, payload_json = ?2",
                    params![
                        sql_u64(current.observed_at_ms),
                        serde_json::to_string(&anchor).unwrap()
                    ],
                )
                .unwrap();
            sample.at_ms = current.observed_at_ms;
            sample.from_ms = sample.at_ms;
            sample.event = Some(event);
            sample.workload = None;
            sample.micro_usd = None;
            sample.consumed = 0;
            connection
                .execute(
                    "INSERT INTO weekly_capacity_history VALUES (?1, ?2, ?3)",
                    params![
                        LEDGER,
                        sql_u64(sample.at_ms),
                        serde_json::to_string(&sample).unwrap()
                    ],
                )
                .unwrap();
        }
        drop(database);
        let database = TelemetryDb::open(&root.join("test.sqlite")).unwrap();
        observe(&database, &changed, &current, NOW + 120_000);
        let value = projected(&database, &changed, &current, NOW + 120_000);
        if event == CapacityEventKind::PricingChanged {
            let value = value.unwrap();
            assert_eq!(value.median_micro_usd, Some(11_200));
            assert_eq!(value.observed_at_ms, quota.observed_at_ms);
        } else {
            assert!(value.is_none());
        }
        assert_eq!(
            database
                .lock_connection()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM weekly_capacity_history", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn missing_prices_stay_unknown_and_can_recover_without_new_quota_consumption() {
    let (root, database, _, quota) = seeded();
    let unpriced_cache = priced_catalog(serde_json::json!({"model":{
        "litellm_provider":"openai", "input_cost_per_token":0.000002,
        "output_cost_per_token":0.00001,
    }}));
    observe(&database, &unpriced_cache, &quota, NOW + 120_000);
    assert!(projected(&database, &unpriced_cache, &quota, NOW + 120_000).is_none());
    let restored = catalog(1.0, true);
    observe(&database, &restored, &quota, NOW + 180_000);
    assert_eq!(
        projected(&database, &restored, &quota, NOW + 180_000)
            .unwrap()
            .median_micro_usd,
        Some(5600)
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn incomplete_or_changed_retained_usage_never_reuses_a_stale_valuation() {
    for mutation in [
        "UPDATE request_attempts SET cached_input_tokens = NULL",
        "UPDATE request_attempts SET input_tokens = input_tokens * 2, cached_input_tokens = cached_input_tokens * 2, output_tokens = output_tokens * 2, total_tokens = total_tokens * 2",
        "DELETE FROM request_logs",
    ] {
        let (root, database, _, quota) = seeded();
        database
            .lock_connection()
            .unwrap()
            .execute_batch(mutation)
            .unwrap();
        let changed = catalog(2.0, false);
        observe(&database, &changed, &quota, NOW + 120_000);
        assert!(
            projected(&database, &changed, &quota, NOW + 120_000).is_none(),
            "{mutation}"
        );
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn fresh_quota_and_new_prices_cannot_revive_an_expired_paired_measurement() {
    let (root, database, old, _) = seeded();
    let recent = window(NOW + 2 * DAY_MS, 9500);
    observe(&database, &old, &recent, recent.observed_at_ms);
    let current = window(NOW + MAX_ANCHOR_AGE + 1, 9500);
    let changed = catalog(2.0, false);
    observe(&database, &changed, &current, current.observed_at_ms);
    assert!(projected(&database, &changed, &current, current.observed_at_ms).is_none());
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn simultaneous_price_and_quota_changes_preserve_structural_fences() {
    for change in 0..5 {
        let (root, database, _, quota) = seeded();
        let changed = catalog(2.0, false);
        let mut current = quota;
        let mut ids = vec!["a".into(), "b".into()];
        let mut plan = "plus";
        match change {
            0 => current.available_basis_points = Some(9600),
            1 => current.provider_cycle_id = Some("new-cycle".into()),
            2 => plan = "pro",
            3 => {
                ids.pop();
            }
            _ => current.window_minutes = Some(6 * 1440),
        }
        if change == 1 {
            current.window_start_ms = Some(current.observed_at_ms);
            current.reset_at_ms = Some(current.observed_at_ms + 7 * DAY_MS);
        }
        let pricing = PricingContext::default();
        database
            .observe_weekly_capacity(
                CapacityObservation {
                    ledger: LEDGER,
                    account_ids: &ids,
                    plan,
                    window: &current,
                    pricing_revision: &pricing.revision_key(&changed),
                    now_ms: NOW + 120_000,
                },
                &changed,
                &pricing,
            )
            .unwrap();
        let stored: Anchor = serde_json::from_str(
            &stored_anchor(&database.lock_connection().unwrap(), LEDGER)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(!stored.calibration.unwrap().valid, "case {change}");
        assert!(projected(&database, &changed, &current, NOW + 120_000).is_none());
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }
}
