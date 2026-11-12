use super::*;

fn observe(scheduler: &mut PoolScheduler, scope: &str, effective: u64, provider: Option<u64>) {
    observe_at(scheduler, scope, effective, provider, None);
}

fn observe_at(
    scheduler: &mut PoolScheduler,
    scope: &str,
    effective: u64,
    provider: Option<u64>,
    observed_at_ms: Option<u64>,
) {
    assert!(scheduler.set_cooldown_with_reason_for_model_at(
        "a",
        CooldownRequest {
            scope,
            retry_at_ms: effective,
            reason: CooldownReason::RateLimit,
            provider_clock: provider.map(|deadline| ProviderClock::new(deadline, observed_at_ms)),
        },
    ));
}

#[test]
fn provider_clock_and_local_policy_expire_independently() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("a"));
    observe(&mut scheduler, "GpT-5", 10_000, Some(2_000));
    assert_eq!(
        scheduler.provider_backoff_for("a", "GPT-5", 100),
        Some(ProviderBackoff {
            scope: "gpt-5".into(),
            not_before_ms: 2_000,
            observed_at_ms: None,
        })
    );
    assert_eq!(scheduler.candidate("a").unwrap().cooldowns["gpt-5"], 10_000);
    scheduler.record_success("a", "gpt-5", 1_500);
    assert!(
        scheduler
            .provider_backoff_for("a", "gpt-5", 1_500)
            .is_some()
    );
    assert!(
        scheduler
            .provider_backoff_for("a", "gpt-5", 2_000)
            .is_none()
    );
    assert_eq!(scheduler.candidate("a").unwrap().cooldowns["gpt-5"], 10_000);
    scheduler.record_success("a", "gpt-5", 10_001);
    assert!(scheduler.candidate("a").unwrap().cooldowns.is_empty());
    assert!(scheduler.cooldown_metadata.is_empty());
}

#[test]
fn inferred_extensions_keep_existing_provider_evidence_and_reason_precedence() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("a"));
    observe(&mut scheduler, "gpt-5", 2_000, Some(2_000));
    assert!(scheduler.set_cooldown("a", "gpt-5", 8_000));
    observe(&mut scheduler, "gpt-5", 3_000, Some(3_000));
    assert_eq!(scheduler.candidate("a").unwrap().cooldowns["gpt-5"], 8_000);
    assert_eq!(
        scheduler
            .provider_backoff_for("a", "gpt-5", 100)
            .unwrap()
            .not_before_ms,
        3_000
    );
    assert_eq!(
        scheduler.cooldown_metadata[&("a".into(), "gpt-5".into())].reason,
        Some(CooldownReason::Mandatory)
    );
    assert!(scheduler.clear_cooldown("a", "gpt-5"));
    assert!(scheduler.provider_backoff_for("a", "gpt-5", 100).is_none());
}

#[test]
fn provider_evidence_keeps_candidate_and_model_scope() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("a"));
    scheduler.upsert(candidate("b"));
    observe(&mut scheduler, "gpt-5", 5_000, Some(5_000));
    assert!(
        scheduler
            .provider_backoff_for("a", "other-model", 100)
            .is_none()
    );
    assert!(scheduler.provider_backoff_for("b", "gpt-5", 100).is_none());
    observe(&mut scheduler, "*", 3_000, Some(3_000));
    assert_eq!(
        scheduler
            .provider_backoff_for("a", "other-model", 100)
            .unwrap()
            .scope,
        "*"
    );
    assert_eq!(
        scheduler
            .provider_backoff_for("a", "gpt-5", 100)
            .unwrap()
            .scope,
        "gpt-5"
    );
    assert!(
        scheduler
            .provider_backoff_for("b", "other-model", 100)
            .is_none()
    );
    scheduler.upsert(candidate("a"));
    assert!(scheduler.provider_backoff_for("a", "gpt-5", 100).is_none());
}

#[test]
fn missing_or_inconsistent_provider_evidence_stays_unknown() {
    let mut scheduler = PoolScheduler::new();
    let mut legacy = candidate("a");
    legacy.cooldowns.insert("gpt-5".into(), 8_000);
    scheduler.upsert(legacy);
    assert!(scheduler.provider_backoff_for("a", "gpt-5", 100).is_none());
    observe(&mut scheduler, "gpt-5", 3_000, Some(3_000));
    assert_eq!(
        scheduler.cooldown_metadata[&("a".into(), "gpt-5".into())].reason,
        None
    );
    assert_eq!(
        scheduler
            .provider_backoff_for("a", "gpt-5", 100)
            .unwrap()
            .not_before_ms,
        3_000
    );
    assert!(scheduler.clear_cooldown("a", "gpt-5"));
    observe(&mut scheduler, "gpt-5", 2_000, Some(9_000));
    assert!(scheduler.provider_backoff_for("a", "gpt-5", 100).is_none());
    assert_eq!(scheduler.candidate("a").unwrap().cooldowns["gpt-5"], 2_000);
}

#[test]
fn shared_provider_evidence_keeps_its_bucket_and_expires_before_local_backoff() {
    let mut scheduler = PoolScheduler::new();
    for id in ["a", "b", "c", "new"] {
        scheduler.upsert(candidate(id));
    }
    let shared = |scheduler: &mut PoolScheduler, id: &str, bucket, now, provider: Option<u64>| {
        scheduler.observe_shared_rate_limit(
            id,
            [bucket; 32],
            CooldownRequest {
                scope: "gpt-5",
                retry_at_ms: 20_000,
                reason: CooldownReason::RateLimit,
                provider_clock: provider.map(|deadline| ProviderClock::new(deadline, Some(now))),
            },
            now,
        );
    };
    shared(&mut scheduler, "a", 1, 1_000, Some(8_000));
    // A later ungrouped hint for the same model is not evidence for this bucket.
    observe(&mut scheduler, "gpt-5", 30_000, Some(25_000));
    shared(&mut scheduler, "b", 1, 1_100, None);
    assert_eq!(
        scheduler.provider_backoff_for("b", "gpt-5", 1_100),
        Some(ProviderBackoff {
            scope: "gpt-5".into(),
            not_before_ms: 8_000,
            observed_at_ms: Some(1_000),
        })
    );
    assert_eq!(scheduler.candidate("b").unwrap().cooldowns["gpt-5"], 20_000);
    shared(&mut scheduler, "c", 2, 1_200, None);
    assert!(
        scheduler
            .provider_backoff_for("c", "gpt-5", 1_200)
            .is_none()
    );
    assert!(
        scheduler
            .provider_backoff_for("b", "other-model", 1_200)
            .is_none()
    );
    shared(&mut scheduler, "b", 1, 1_500, None);
    assert_eq!(
        scheduler
            .provider_backoff_for("b", "gpt-5", 1_500)
            .unwrap()
            .observed_at_ms,
        Some(1_000),
        "a membership refresh must not become a new provider observation",
    );
    shared(&mut scheduler, "new", 1, 8_000, None);
    assert!(
        scheduler
            .provider_backoff_for("new", "gpt-5", 8_000)
            .is_none()
    );
    assert_eq!(
        scheduler.candidate("new").unwrap().cooldowns["gpt-5"],
        20_000
    );
    scheduler.remove("b");
    scheduler.upsert(candidate("b"));
    assert!(
        scheduler
            .provider_backoff_for("b", "gpt-5", 1_200)
            .is_none()
    );
}

#[test]
fn normalization_clock_survives_reads_local_floors_and_shorter_hints() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("a"));
    observe_at(&mut scheduler, "gpt-5", 10_000, Some(2_000), Some(100));
    assert!(scheduler.set_cooldown("a", "gpt-5", 14_000));
    observe_at(&mut scheduler, "gpt-5", 8_000, Some(1_500), Some(300));
    observe_at(&mut scheduler, "gpt-5", 9_000, Some(2_000), Some(500));
    for read_at in [501, 1_000, 1_999] {
        let clock = scheduler
            .provider_backoff_for("a", "gpt-5", read_at)
            .unwrap();
        assert_eq!(clock.not_before_ms, 2_000);
        assert_eq!(clock.observed_at_ms, Some(100));
    }
    observe_at(&mut scheduler, "gpt-5", 6_000, Some(3_500), Some(900));
    let clock = scheduler.provider_backoff_for("a", "gpt-5", 1_000).unwrap();
    assert_eq!(clock.not_before_ms, 3_500);
    assert_eq!(clock.observed_at_ms, Some(900));
    assert_eq!(scheduler.candidate("a").unwrap().cooldowns["gpt-5"], 14_000);
    assert!(scheduler.clear_cooldown("a", "gpt-5"));
    assert!(
        scheduler
            .provider_backoff_for("a", "gpt-5", 1_000)
            .is_none()
    );
}

#[test]
fn unknown_clock_time_is_not_borrowed_from_a_different_deadline_or_storage() {
    let legacy = serde_json::json!({"scope":"gpt-5","notBeforeMs":2_000});
    let decoded: ProviderBackoff = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(decoded.observed_at_ms, None);
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);

    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("a"));
    observe_at(&mut scheduler, "gpt-5", 8_000, Some(2_000), Some(100));
    observe_at(&mut scheduler, "gpt-5", 8_000, Some(4_000), None);
    observe_at(&mut scheduler, "gpt-5", 8_000, Some(3_000), Some(500));
    let clock = scheduler.provider_backoff_for("a", "gpt-5", 1_000).unwrap();
    assert_eq!(clock.not_before_ms, 4_000);
    assert_eq!(clock.observed_at_ms, None);
    observe_at(&mut scheduler, "gpt-5", 8_000, Some(4_000), Some(2_000));
    observe_at(&mut scheduler, "gpt-5", 8_000, Some(4_000), Some(2_500));
    assert_eq!(
        scheduler
            .provider_backoff_for("a", "gpt-5", 3_000)
            .unwrap()
            .observed_at_ms,
        Some(2_000),
    );

    observe_at(
        &mut scheduler,
        "invalid-time",
        8_000,
        Some(4_000),
        Some(4_001),
    );
    assert_eq!(
        scheduler
            .provider_backoff_for("a", "invalid-time", 3_000)
            .unwrap()
            .observed_at_ms,
        None,
    );
}
