use super::*;

fn observe(scheduler: &mut PoolScheduler, member: &str, bucket: u8, model: &str, now: u64) {
    scheduler.observe_shared_rate_limit(
        member,
        [bucket; 32],
        CooldownRequest {
            scope: model,
            retry_at_ms: now + 2,
            reason: CooldownReason::RateLimit,
        },
        now,
    );
}

fn pool() -> PoolScheduler {
    let mut scheduler = PoolScheduler::new();
    for member in ["a", "b", "c", "unknown"] {
        scheduler.upsert(candidate(member));
    }
    scheduler
}

#[test]
fn shared_rate_limits_only_block_confirmed_members_of_the_same_model_and_bucket() {
    let mut scheduler = pool();
    observe(&mut scheduler, "a", 1, "gpt-5", 100);
    observe(&mut scheduler, "b", 1, "gpt-5", 200);
    observe(&mut scheduler, "c", 2, "gpt-5", 300);
    assert_eq!(scheduler.candidate("a").unwrap().cooldowns["gpt-5"], 5_200);
    assert_eq!(scheduler.candidate("b").unwrap().cooldowns["gpt-5"], 5_200);
    observe(&mut scheduler, "a", 1, "gpt-5", 6_000);
    for member in ["a", "b"] {
        let cooldowns = &scheduler.candidate(member).unwrap().cooldowns;
        assert_eq!(cooldowns["gpt-5"], 16_000);
        assert!(!cooldowns.contains_key("*"));
        assert!(!cooldowns.contains_key("gpt-other"));
    }
    assert_eq!(scheduler.candidate("c").unwrap().cooldowns["gpt-5"], 5_300);
    assert!(scheduler.candidate("unknown").unwrap().cooldowns.is_empty());
    observe(&mut scheduler, "a", 1, "gpt-other", 6_100);
    assert!(
        !scheduler
            .candidate("b")
            .unwrap()
            .cooldowns
            .contains_key("gpt-other")
    );
}

#[test]
fn shared_rate_limit_backoff_grows_is_bounded_and_resets_after_recovery() {
    let mut scheduler = pool();
    let mut now = 100;
    for expected_delay in [5_000, 10_000, 20_000, 40_000, 60_000, 60_000] {
        observe(&mut scheduler, "a", 1, "gpt-5", now);
        assert_eq!(
            scheduler.candidate("a").unwrap().cooldowns["gpt-5"],
            now + expected_delay
        );
        // An older in-flight success cannot erase a still-active cooldown.
        scheduler.reset_shared_rate_limit_backoff("a", "gpt-5", now + 1);
        now += expected_delay;
    }
    scheduler.reset_shared_rate_limit_backoff("a", "gpt-5", now);
    observe(&mut scheduler, "a", 1, "gpt-5", now + 1);
    assert_eq!(
        scheduler.candidate("a").unwrap().cooldowns["gpt-5"],
        now + 5_001
    );
}

#[test]
fn shared_rate_limit_preserves_long_provider_deadlines() {
    let mut scheduler = pool();
    observe(&mut scheduler, "a", 1, "gpt-5", 100);
    scheduler.observe_shared_rate_limit(
        "b",
        [1; 32],
        CooldownRequest {
            scope: "gpt-5",
            retry_at_ms: 120_200,
            reason: CooldownReason::RateLimit,
        },
        200,
    );
    observe(&mut scheduler, "a", 1, "gpt-5", 300);
    for member in ["a", "b"] {
        assert_eq!(
            scheduler.candidate(member).unwrap().cooldowns["gpt-5"],
            120_200
        );
    }
}

#[test]
fn shared_rate_limit_membership_expires_and_does_not_survive_replacement() {
    let mut scheduler = pool();
    observe(&mut scheduler, "a", 1, "gpt-5", 100);
    observe(&mut scheduler, "b", 1, "gpt-5", 200);
    scheduler.remove("b");
    scheduler.upsert(candidate("b"));
    observe(&mut scheduler, "a", 1, "gpt-5", 6_000);
    assert!(scheduler.candidate("b").unwrap().cooldowns.is_empty());
    observe(&mut scheduler, "b", 1, "gpt-5", 7_000);
    observe(&mut scheduler, "a", 1, "gpt-5", 608_000);
    assert_eq!(
        scheduler.candidate("a").unwrap().cooldowns["gpt-5"],
        613_000
    );
    assert!(scheduler.candidate("b").unwrap().cooldowns["gpt-5"] < 608_000);
}

#[test]
fn shared_rate_limit_membership_survives_metadata_updates_but_not_permission_changes() {
    let mut scheduler = pool();
    observe(&mut scheduler, "a", 1, "gpt-5", 100);
    observe(&mut scheduler, "b", 1, "gpt-5", 200);
    let mut updated = scheduler.candidate("b").unwrap().clone();
    updated.priority += 1;
    scheduler.upsert(updated.clone());
    observe(&mut scheduler, "a", 1, "gpt-5", 6_000);
    assert_eq!(scheduler.candidate("b").unwrap().cooldowns["gpt-5"], 16_000);
    updated.enabled = false;
    scheduler.upsert(updated);
    let previous = scheduler.candidate("b").unwrap().cooldowns["gpt-5"];
    observe(&mut scheduler, "a", 1, "gpt-5", 17_000);
    assert_eq!(
        scheduler.candidate("b").unwrap().cooldowns["gpt-5"],
        previous
    );
}
