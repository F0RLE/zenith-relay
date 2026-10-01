use super::*;
use crate::PoolRoutingMode;

#[test]
fn automatic_uses_the_largest_known_quota_before_credits_or_recency() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    for (id, quota, credits, reset, priority) in [
        ("account", 1, 1, 900, -1_000),
        ("api-a", 9_999, 100_000, 100, 1_000),
        ("api-b", 500, 10, 500, 10),
    ] {
        let member = scheduler.candidates.get_mut(id).unwrap();
        member.quota = CandidateQuota::Available(quota);
        member.quota_updated_at_ms = Some(100);
        member.provider_credits_micro_units = Some(credits);
        member.quota_reset_at_ms = Some(reset);
        member.last_used_at = Some(reset);
        member.priority = priority;
    }
    for _ in 0..12 {
        assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "api-a");
    }
}

#[test]
fn automatic_switches_when_the_leader_falls_one_point_below() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    for (id, quota) in [("account", 9_900_u64), ("api-a", 3_300), ("api-b", 3_200)] {
        let member = scheduler.candidates.get_mut(id).unwrap();
        member.quota = CandidateQuota::Available(quota);
        member.quota_updated_at_ms = Some(100);
    }
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "account");

    let account = scheduler.candidates.get_mut("account").unwrap();
    account.quota = CandidateQuota::Available(3_200);
    account.quota_updated_at_ms = Some(100);
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "api-a");

    let api_a = scheduler.candidates.get_mut("api-a").unwrap();
    api_a.quota = CandidateQuota::Available(3_100);
    api_a.quota_updated_at_ms = Some(100);
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "account");
}

#[test]
fn stale_high_quota_does_not_beat_a_smaller_fresh_remainder() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    scheduler.set_quota_stale_after_ms(10);
    let account = scheduler.candidates.get_mut("account").unwrap();
    account.quota = CandidateQuota::Available(10_000);
    account.quota_updated_at_ms = Some(1);
    let api_a = scheduler.candidates.get_mut("api-a").unwrap();
    api_a.quota = CandidateQuota::Available(3_300);
    api_a.quota_updated_at_ms = Some(100);
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "api-a");
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
        if quota == CandidateQuota::Available(1) {
            assert_eq!(selected, BTreeSet::from(["account".into()]));
        } else {
            assert_eq!(selected.len(), 3);
        }
    }
}

#[test]
fn explicit_modes_ignore_quota_percentages() {
    for mode in [PoolRoutingMode::InOrder, PoolRoutingMode::RoundRobin] {
        let mut scheduler = policy::mixed(mode);
        for (id, quota) in [("account", 1_u64), ("api-a", 9_900), ("api-b", 8_000)] {
            let member = scheduler.candidates.get_mut(id).unwrap();
            member.quota = CandidateQuota::Available(quota);
            member.quota_updated_at_ms = Some(100);
        }
        let first = rotation::dispatch(&mut scheduler, 100, None);
        if mode == PoolRoutingMode::InOrder {
            assert_eq!(first, "account");
        } else {
            let mut counts = BTreeMap::new();
            *counts.entry(first).or_insert(0) += 1;
            for _ in 0..11 {
                *counts
                    .entry(rotation::dispatch(&mut scheduler, 100, None))
                    .or_insert(0) += 1;
            }
            assert_eq!(counts.len(), 3);
        }
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

#[test]
fn automatic_owner_yields_only_for_a_strictly_larger_fresh_remainder() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    for (id, quota) in [("account", 3_300_u64), ("api-a", 9_800), ("api-b", 3_200)] {
        let member = scheduler.candidates.get_mut(id).unwrap();
        member.quota = CandidateQuota::Available(quota);
        member.quota_updated_at_ms = Some(100);
    }
    assert!(scheduler.bind_response_affinity("response", "account", 100));
    let scope = CandidateScope::default();
    let tried = HashSet::new();
    let protocols = [WireApi::Responses, WireApi::ChatCompletions];
    assert!(scheduler.automatic_response_owner_should_yield_for_quota(
        "response", "gpt-5", &protocols, &scope, &tried, 100,
    ));
    let selected = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &protocols,
            scope: &scope,
            tried: &tried,
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 100,
        })
        .unwrap();
    assert_eq!(selected.candidate_id, "account");
    assert_eq!(
        selected.diagnostics.reason,
        SelectionReason::ResponseAffinity
    );

    scheduler.candidates.get_mut("api-a").unwrap().quota = CandidateQuota::Available(3_200);
    assert!(!scheduler.automatic_response_owner_should_yield_for_quota(
        "response", "gpt-5", &protocols, &scope, &tried, 100,
    ));
    let mut policy = scheduler.pool_routing.clone().unwrap();
    policy.mode = PoolRoutingMode::InOrder;
    scheduler.set_pool_routing(policy).unwrap();
    scheduler.candidates.get_mut("api-a").unwrap().quota = CandidateQuota::Available(9_800);
    assert!(!scheduler.automatic_response_owner_should_yield_for_quota(
        "response", "gpt-5", &protocols, &scope, &tried, 100,
    ));
}
