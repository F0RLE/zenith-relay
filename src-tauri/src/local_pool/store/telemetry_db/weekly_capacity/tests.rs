use super::*;
use zenith_relay_core::{
    pricing::{PricingCatalog, PricingContext},
    quota::QuotaWindowKind,
};

const NOW: u64 = 2_000_000_000_000;

fn window(at: u64, remaining: u16) -> QuotaWindow {
    serde_json::from_value(serde_json::json!({
        "kind":"secondary", "windowStartMs":NOW - DAY_MS, "resetAtMs":NOW + 6 * DAY_MS,
        "availableBasisPoints":remaining, "windowMinutes":10080, "observedAtMs":at,
    }))
    .unwrap()
}

fn catalog() -> PricingCatalog {
    PricingCatalog::from_litellm_payload(
        &serde_json::json!({"model":{
            "litellm_provider":"openai", "input_cost_per_token":0.000002,
            "cache_read_input_token_cost":0.0000005, "output_cost_per_token":0.00001,
        }}),
        Some("synthetic-v1".into()),
        Some(1),
        false,
    )
    .unwrap()
}

fn database() -> (std::path::PathBuf, TelemetryDb) {
    let root = std::env::temp_dir().join(format!("relay-capacity-{}", uuid::Uuid::new_v4()));
    let database = TelemetryDb::open(&root.join("test.sqlite")).unwrap();
    (root, database)
}

fn observe(database: &TelemetryDb, at: u64, remaining: u16, plan: &str, revision: &str) {
    database
        .observe_weekly_capacity(
            CapacityObservation {
                ledger: "synthetic-ledger",
                account_ids: &["a".into(), "b".into()],
                plan,
                window: &window(at, remaining),
                pricing_revision: revision,
                now_ms: at,
            },
            &catalog(),
            &PricingContext::default(),
        )
        .unwrap();
}

fn usage(database: &TelemetryDb, id: &str, account: &str, at: u64, cached: Option<u64>) {
    let mut event = super::super::tests::aggregate_test_event(id, 1, 100, 0, None, 20);
    event.account_id = Some(account.into());
    event.cached_input_tokens = cached;
    event.applied_service_tier =
        Some(serde_json::from_value(serde_json::json!("default")).unwrap());
    database.record(&event).unwrap();
    let connection = database.lock_connection().unwrap();
    for table in ["request_logs", "request_attempts"] {
        connection.execute(&format!("UPDATE {table} SET created_at = datetime(?1 / 1000, 'unixepoch') WHERE request_id = ?2"), params![sql_u64(at), id]).unwrap();
    }
}

fn report(database: &TelemetryDb, now: u64) -> serde_json::Value {
    serde_json::to_value(
        database
            .weekly_capacity_report(WeeklyCapacityQuery::default(), now)
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn weekly_capacity_persists_anchors_and_counts_linked_routes_as_one_ledger() {
    let (root, database) = database();
    observe(&database, NOW, 10_000, "plus", "v1");
    drop(database);
    let database = TelemetryDb::open(&root.join("test.sqlite")).unwrap();
    usage(&database, "first", "a", NOW + 30_000, Some(80));
    usage(&database, "linked", "b", NOW + 40_000, Some(80));
    observe(&database, NOW + 60_000, 9_500, "plus", "v1");
    let value = report(&database, NOW + 60_000);
    assert_eq!(value["groups"].as_array().unwrap().len(), 1);
    assert_eq!(value["groups"][0]["current"]["accountCount"], 1);
    assert_eq!(value["groups"][0]["current"]["medianMicroUsd"], 11_200);
    let ids = vec!["a".into(), "b".into()];
    let current = window(NOW + 60_000, 9_500);
    let projected = |database: &TelemetryDb| {
        database
            .weekly_value_projections(&[CapacityObservation {
                ledger: "synthetic-ledger",
                account_ids: &ids,
                plan: "plus",
                window: &current,
                pricing_revision: "v1",
                now_ms: NOW + 60_000,
            }])
            .unwrap()
    };
    let before_restart = projected(&database);
    assert_eq!(before_restart.len(), 2);
    assert_eq!(before_restart["a"].calibration.median_micro_usd, 11_200);
    assert_eq!(before_restart["a"], before_restart["b"]);
    let scoped = |id: &str| {
        database
            .weekly_capacity_report(
                WeeklyCapacityQuery {
                    account_id: Some(id.into()),
                    ..Default::default()
                },
                NOW + 60_000,
            )
            .unwrap()
    };
    assert_eq!(scoped("b").groups.len(), 1);
    assert!(scoped("unrelated").groups.is_empty());
    let revision = database.weekly_capacity_revision();
    observe(&database, NOW + 60_000, 9_500, "plus", "v1");
    assert_eq!(database.weekly_capacity_revision(), revision);
    drop(database);
    let database = TelemetryDb::open(&root.join("test.sqlite")).unwrap();
    assert_eq!(projected(&database), before_restart);
    assert_eq!(
        report(&database, NOW + 60_000)["groups"][0]["current"]["medianMicroUsd"],
        11_200
    );
    database.clear().unwrap();
    assert!(projected(&database).is_empty());
    assert!(report(&database, NOW + 60_000)["groups"]
        .as_array()
        .unwrap()
        .is_empty());
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn weekly_capacity_rejects_small_incomplete_expired_or_reversed_observations() {
    let (root, database) = database();
    observe(&database, NOW, 10_000, "plus", "v1");
    usage(&database, "unknown-cache", "a", NOW + 30_000, None);
    observe(&database, NOW + 60_000, 9_600, "plus", "v1");
    assert!(report(&database, NOW + 60_000)["groups"]
        .as_array()
        .unwrap()
        .is_empty());
    observe(&database, NOW + 120_000, 9_500, "plus", "v1");
    assert!(report(&database, NOW + 120_000)["groups"]
        .as_array()
        .unwrap()
        .is_empty());
    usage(&database, "known", "a", NOW + 150_000, Some(80));
    observe(&database, NOW + 180_000, 9_000, "plus", "v1");
    assert_eq!(
        report(&database, NOW + 180_000)["groups"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let revision = database.weekly_capacity_revision();
    observe(&database, NOW + 60_000, 8_000, "plus", "v1");
    assert_eq!(database.weekly_capacity_revision(), revision);
    assert!(report(&database, NOW + 4 * DAY_MS)["groups"][0]["current"].is_null());
    let mut expired = window(NOW + 200_000, 8_000);
    expired.reset_at_ms = Some(NOW + 100_000);
    database
        .observe_weekly_capacity(
            CapacityObservation {
                ledger: "synthetic-ledger",
                account_ids: &["a".into()],
                plan: "plus",
                window: &expired,
                pricing_revision: "v1",
                now_ms: NOW + 200_000,
            },
            &catalog(),
            &PricingContext::default(),
        )
        .unwrap();
    assert_eq!(database.weekly_capacity_revision(), revision);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn weekly_capacity_resets_and_plan_or_price_changes_are_events_not_capacity_increases() {
    let (root, database) = database();
    observe(&database, NOW, 10_000, "plus", "v1");
    usage(&database, "known", "a", NOW + 30_000, Some(80));
    observe(&database, NOW + 60_000, 9_500, "plus", "v1");
    observe(&database, NOW + 120_000, 10_000, "plus", "v1");
    assert!(report(&database, NOW + 120_000)["groups"][0]["current"].is_null());
    observe(&database, NOW + 180_000, 9_000, "pro", "v1");
    observe(&database, NOW + 240_000, 8_000, "pro", "v2");
    let value = report(&database, NOW + 240_000);
    let kinds = value["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"discontinuity"));
    assert!(kinds.contains(&"plan_changed"));
    assert!(kinds.contains(&"pricing_changed"));
    assert_eq!(value["groups"].as_array().unwrap().len(), 1);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn weekly_capacity_window_identity_does_not_confuse_burst_and_weekly_limits() {
    let anchor = Anchor {
        account_ids: vec!["a".into()],
        plan: "plus".into(),
        pricing_revision: "v1".into(),
        members: "a".into(),
        window: window(NOW, 9000),
        from_ms: NOW,
        remaining: 9000,
    };
    let mut next = anchor.clone();
    next.window.observed_at_ms += 60_000;
    next.window.kind = QuotaWindowKind::Primary;
    assert!(discontinuity(&anchor, &next) == Some(CapacityEventKind::Discontinuity));
    next = anchor.clone();
    next.window.window_start_ms = anchor.window.reset_at_ms;
    assert!(discontinuity(&anchor, &next) == Some(CapacityEventKind::Reset));
    next = anchor.clone();
    next.plan = "prolite".into();
    assert!(discontinuity(&anchor, &next) == Some(CapacityEventKind::PlanChanged));
}

#[test]
fn weekly_capacity_same_timestamp_corrections_invalidate_old_evidence() {
    let (root, database) = database();
    observe(&database, NOW, 10_000, "plus", "v1");
    usage(&database, "known", "a", NOW + 30_000, Some(80));
    observe(&database, NOW + 60_000, 9_500, "plus", "v1");
    observe(&database, NOW + 60_000, 9_600, "plus", "v1");
    let value = report(&database, NOW + 60_000);
    assert!(value["groups"].as_array().unwrap().is_empty());
    assert_eq!(value["events"][0]["kind"], "discontinuity");
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn weekly_capacity_maintenance_prunes_history_without_new_measurements() {
    let (root, database) = database();
    let now = zenith_relay_core::unix_time_ms();
    let connection = database.lock_connection().unwrap();
    connection
        .execute(
            "INSERT INTO weekly_capacity_history VALUES ('old', ?1, '{}')",
            [sql_u64(now - 31 * DAY_MS)],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO weekly_capacity_anchors VALUES ('old', ?1, '{}')",
            [sql_u64(now - 4 * DAY_MS)],
        )
        .unwrap();
    drop(connection);
    assert_eq!(database.maintain_usage().unwrap(), 1);
    assert!(report(&database, now)["groups"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(database.weekly_capacity_revision(), 1);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn weekly_capacity_account_purge_removes_derived_shared_ledger_history() {
    let (root, database) = database();
    observe(&database, NOW, 10_000, "plus", "v1");
    usage(&database, "known", "a", NOW + 30_000, Some(80));
    observe(&database, NOW + 60_000, 9_500, "plus", "v1");
    database
        .replace_state_json_and_delete_account_data(&[], "a")
        .unwrap();
    assert!(report(&database, NOW + 60_000)["groups"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(database.capacity_seen.lock().unwrap().is_empty());
    assert_eq!(
        database
            .lock_connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM weekly_capacity_anchors", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
