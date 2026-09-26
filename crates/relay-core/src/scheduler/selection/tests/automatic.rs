use super::*;
use crate::PoolRoutingMode;

#[test]
fn quota_credits_reset_and_last_use_do_not_rank_ready_members() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    for (id, quota, credits, reset, priority) in [
        ("account", 1, 1, 900, -1_000),
        ("api-a", 9_999, 100_000, 100, 1_000),
        ("api-b", 500, 10, 500, 10),
    ] {
        let member = scheduler.candidates.get_mut(id).unwrap();
        member.quota = CandidateQuota::Available(quota);
        member.provider_credits_micro_units = Some(credits);
        member.quota_reset_at_ms = Some(reset);
        member.last_used_at = Some(reset);
        member.priority = priority;
    }
    let mut counts = BTreeMap::new();
    for _ in 0..60 {
        *counts
            .entry(rotation::dispatch(&mut scheduler, 100, None))
            .or_insert(0) += 1;
    }
    assert_eq!(
        counts,
        BTreeMap::from([
            ("account".into(), 20),
            ("api-a".into(), 20),
            ("api-b".into(), 20),
        ])
    );
}

#[test]
fn quota_refresh_changes_admission_without_rebuilding_or_reweighting() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    assert!(scheduler.update_candidate_availability(
        "account",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Exhausted,
    ));
    let selected: BTreeSet<_> = (0..12)
        .map(|_| rotation::dispatch(&mut scheduler, 100, None))
        .collect();
    assert_eq!(selected, BTreeSet::from(["api-a".into(), "api-b".into()]));
    for quota in [
        CandidateQuota::Available(1),
        CandidateQuota::Unknown,
        CandidateQuota::Stale,
    ] {
        assert!(scheduler.update_candidate_availability(
            "account",
            true,
            CandidateHealth::Healthy,
            quota,
        ));
        let selected: BTreeSet<_> = (0..12)
            .map(|_| rotation::dispatch(&mut scheduler, 100, None))
            .collect();
        assert_eq!(selected.len(), 3);
    }
}

#[test]
fn automatic_uses_normalized_physical_load_before_weight() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    let mut policy = scheduler.pool_routing.clone().unwrap();
    for member in &mut policy.members {
        member.max_concurrency = if member.id == "api-a" { 4 } else { 2 };
        member.weight = if member.id == "account" { 100 } else { 1 };
    }
    scheduler.set_pool_routing(policy).unwrap();
    for id in ["account", "api-a", "api-b"] {
        assert!(scheduler.reserve_for(id, "gpt-5", 100));
    }
    // 1/4 beats 1/2 even against a much larger weight.
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "api-a");
    for id in ["account", "api-a", "api-b"] {
        assert!(scheduler.release_for(id, Some("gpt-5")));
    }
}

#[test]
fn stale_monitoring_is_neutral_for_accounts_and_sources() {
    for mut member in [candidate("member"), oauth_candidate("member")] {
        for quota in [CandidateQuota::Unknown, CandidateQuota::Stale] {
            member.quota = quota;
            let mut scheduler = PoolScheduler::new();
            scheduler.upsert(member.clone());
            assert!(select(&mut scheduler, &HashSet::new()).is_some());
            assert!(scheduler.runtime_order(100)[0].available);
        }
    }
}

#[test]
fn prompt_affinity_never_overrides_lower_load_or_explicit_order() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    scheduler.bind_prompt_affinity("cache:test", "api-b", 100);
    assert_eq!(
        rotation::dispatch(&mut scheduler, 100, Some("cache:test")),
        "api-b"
    );
    assert!(scheduler.reserve_for("api-b", "gpt-5", 100));
    for _ in 0..12 {
        assert_ne!(
            rotation::dispatch(&mut scheduler, 100, Some("cache:test")),
            "api-b"
        );
    }
    assert!(scheduler.release_for("api-b", Some("gpt-5")));
    let mut policy = scheduler.pool_routing.clone().unwrap();
    policy.mode = PoolRoutingMode::InOrder;
    scheduler.set_pool_routing(policy).unwrap();
    assert_eq!(
        rotation::dispatch(&mut scheduler, 100, Some("cache:test")),
        "account"
    );
}
