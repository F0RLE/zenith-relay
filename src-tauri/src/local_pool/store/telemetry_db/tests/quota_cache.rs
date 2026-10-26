use super::*;

struct Fixture {
    database: TelemetryDb,
    root: std::path::PathBuf,
    windows: Vec<(String, u64, u64)>,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("relay-quota-cache-{}", uuid::Uuid::new_v4()));
        let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
        let now = zenith_relay_core::unix_time_ms();
        let windows = ["account-a", "account-b"]
            .into_iter()
            .map(|id| (id.into(), now.saturating_sub(60_000), now + 60_000))
            .collect();
        let fixture = Self {
            database,
            root,
            windows,
        };
        for id in ["account-a", "account-b"] {
            fixture
                .database
                .record(&account_event(id, id, 100))
                .unwrap();
        }
        fixture
    }

    fn read(&self) -> HashMap<String, ApiEquivalentSummary> {
        self.database
            .account_api_equivalents_with_pricing(
                &self.windows,
                &test_pricing_catalog(),
                &test_pricing_context(&BTreeMap::new(), &BTreeMap::new()),
            )
            .unwrap()
    }

    fn reads(&self) -> u64 {
        self.database
            .quota_window_reads
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn cleanup(self) {
        let Self { database, root, .. } = self;
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn account_event(request: &str, account: &str, tokens: u64) -> UsageEvent {
    let mut event = aggregate_test_event(request, 1, tokens, 0, None, 10);
    event.source_id = "codex".into();
    event.account_id = Some(account.into());
    event.candidate_id = Some(account.into());
    event.requested_model = Some("gpt-5.4".into());
    event.resolved_model = event.requested_model.clone();
    event
}

#[test]
fn quota_reads_use_the_shared_reader_with_the_existing_memory_budget() {
    use std::time::{Duration, Instant};

    let fixture = Fixture::new();
    let writer = fixture.database.lock_connection().unwrap();
    let (acquired_reader, accounts) = std::thread::scope(|scope| {
        let task = scope.spawn(|| fixture.read());
        let reader = fixture.database.report_connection.as_ref().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while reader.try_lock().is_ok() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let acquired_reader = reader.try_lock().is_err();
        drop(writer);
        let result = task.join().unwrap();
        (acquired_reader, result.len())
    });
    let memory_budget = fixture.database.memory_cache_budget();
    fixture.cleanup();
    assert!(
        acquired_reader,
        "quota reads bypassed the shared read snapshot"
    );
    assert_eq!(accounts, 2);
    assert_eq!(memory_budget, Some(32 * 1024 * 1024));
}

#[test]
fn quota_cache_preserves_other_accounts_and_source_only_writes() {
    let fixture = Fixture::new();
    let first = fixture.read();
    assert_eq!(fixture.reads(), 2);
    assert_eq!(fixture.read(), first);
    assert_eq!(fixture.reads(), 2);
    fixture
        .database
        .record(&aggregate_test_event("source-only", 1, 123, 0, None, 10))
        .unwrap();
    assert_eq!(fixture.read(), first);
    assert_eq!(
        fixture.reads(),
        2,
        "API source usage must not reread account windows"
    );
    fixture
        .database
        .record(&account_event("account-a", "account-a", 200))
        .unwrap();
    let updated = fixture.read();
    assert_eq!(fixture.reads(), 3, "only the corrected account needs SQL");
    assert!(updated["account-a"].micro_usd > first["account-a"].micro_usd);
    assert_eq!(updated["account-b"], first["account-b"]);
    fixture.cleanup();
}

#[test]
fn quota_cache_invalidates_both_owners_of_a_corrected_earlier_attempt() {
    let fixture = Fixture::new();
    let mut first = account_event("retry-request", "account-a", 100);
    fixture.database.record(&first).unwrap();
    let mut final_send = account_event("retry-request", "account-b", 200);
    final_send.attempt = 2;
    fixture.database.record(&final_send).unwrap();
    let before = fixture.read();
    let reads = fixture.reads();
    first.account_id = Some("account-b".into());
    first.candidate_id = first.account_id.clone();
    first.input_tokens = Some(300);
    first.total_tokens = Some(310);
    fixture.database.record(&first).unwrap();
    let after = fixture.read();
    assert_eq!(fixture.reads(), reads + 2);
    assert!(after["account-a"].micro_usd < before["account-a"].micro_usd);
    assert!(after["account-b"].micro_usd > before["account-b"].micro_usd);
    fixture.database.invalidate_usage_cache();
    assert_eq!(
        fixture.read(),
        after,
        "partial cache must match an uncached scan"
    );
    fixture.cleanup();
}

#[test]
fn quota_cache_rejects_publication_of_a_read_invalidated_by_write_or_clear() {
    let fixture = Fixture::new();
    fixture.read();
    let revision = fixture
        .database
        .usage_revision
        .load(std::sync::atomic::Ordering::Acquire);
    let old = fixture
        .database
        .quota_equivalent_cache
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    fixture
        .database
        .record(&account_event("late-write", "account-a", 500))
        .unwrap();
    assert!(!fixture
        .database
        .publish_quota_equivalents(revision, old.clone())
        .unwrap());
    assert!(!fixture
        .database
        .quota_equivalent_cache
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .equivalents_by_account
        .contains_key("account-a"));
    fixture.database.clear().unwrap();
    assert!(!fixture
        .database
        .publish_quota_equivalents(revision, old)
        .unwrap());
    assert!(fixture
        .database
        .quota_equivalent_cache
        .lock()
        .unwrap()
        .is_none());
    fixture.cleanup();
}

#[test]
fn quota_cache_respects_window_price_clear_and_inventory_changes() {
    let mut fixture = Fixture::new();
    let first = fixture.read();
    fixture.windows.reverse();
    assert_eq!(fixture.read(), first);
    assert_eq!(
        fixture.reads(),
        2,
        "window ordering is not a cache invalidation"
    );
    fixture.windows[0].2 += 1;
    assert_eq!(fixture.read(), first);
    assert_eq!(fixture.reads(), 3);
    let changed_catalog = PricingCatalog::from_litellm_payload(
        &serde_json::json!({"gpt-5.4": {
            "litellm_provider": "openai",
            "input_cost_per_token": 0.000020,
            "output_cost_per_token": 0.000040
        }}),
        Some("synthetic-quota-price-v2".into()),
        Some(1),
        false,
    )
    .unwrap();
    let changed = fixture
        .database
        .account_api_equivalents_with_pricing(
            &fixture.windows,
            &changed_catalog,
            &test_pricing_context(&BTreeMap::new(), &BTreeMap::new()),
        )
        .unwrap();
    assert_eq!(fixture.reads(), 5);
    assert_ne!(changed["account-a"].micro_usd, first["account-a"].micro_usd);
    fixture.database.clear().unwrap();
    assert!(fixture
        .database
        .quota_equivalent_cache
        .lock()
        .unwrap()
        .is_none());
    assert!(fixture.read().values().all(|value| value.micro_usd == 0));
    assert_eq!(fixture.reads(), 7);
    fixture.windows.pop();
    assert_eq!(fixture.read().len(), 1);
    assert_eq!(fixture.reads(), 7);
    fixture.cleanup();
}
