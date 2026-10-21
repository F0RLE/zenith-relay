use super::*;
use crate::PoolRoutingMode;

fn reference_quota_scheduler() -> PoolScheduler {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    let member = scheduler.candidates.get_mut("api-a").unwrap();
    member.quota = CandidateQuota::Available(1_200);
    member.quota_updated_at_ms = Some(100);
    scheduler.set_quota_references(BTreeMap::from([(
        "account".into(),
        crate::CandidateQuotaReference {
            source_account_id: "linked-bps".into(),
            remaining_basis_points: 9_700,
            observed_at_ms: 100,
            valid_until_ms: Some(1_000),
        },
    )]));
    scheduler
}

#[test]
fn automatic_ranks_linked_quota_without_changing_own_capacity() {
    let mut scheduler = reference_quota_scheduler();
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "account");
    assert_eq!(
        scheduler.candidates["account"].quota,
        CandidateQuota::Unknown
    );
    assert_eq!(scheduler.candidates["account"].quota_updated_at_ms, None);
    scheduler.set_quota_references(BTreeMap::new());
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "api-a");
}

#[test]
fn reference_quota_never_overrides_own_observation_or_auth() {
    for quota in [
        CandidateQuota::Available(100),
        CandidateQuota::Available(0),
        CandidateQuota::Exhausted,
        CandidateQuota::Stale,
    ] {
        let mut scheduler = reference_quota_scheduler();
        let member = scheduler.candidates.get_mut("account").unwrap();
        member.quota = quota;
        member.quota_updated_at_ms = Some(100);
        assert_eq!(
            rotation::dispatch(&mut scheduler, 100, None),
            "api-a",
            "{quota:?}"
        );
    }
    let mut scheduler = reference_quota_scheduler();
    scheduler.candidates.get_mut("account").unwrap().health = CandidateHealth::ReauthRequired;
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "api-a");
}

#[test]
fn reference_quota_expires_and_rejects_future_zero_or_invalid_observations() {
    for (remaining, observed, deadline) in [
        (0, 100, 1_000),
        (10_001, 100, 1_000),
        (9_700, 101, 1_000),
        (9_700, 100, 100),
        (9_700, 1, 1_000),
    ] {
        let mut scheduler = reference_quota_scheduler();
        scheduler.set_quota_stale_after_ms(50);
        let reference = scheduler.quota_references.get_mut("account").unwrap();
        reference.remaining_basis_points = remaining;
        reference.observed_at_ms = observed;
        reference.valid_until_ms = Some(deadline);
        assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "api-a");
    }
}

#[test]
fn reference_quota_zero_is_not_an_execution_block_and_removal_clears_reference() {
    let mut scheduler = reference_quota_scheduler();
    scheduler
        .quota_references
        .get_mut("account")
        .unwrap()
        .remaining_basis_points = 0;
    scheduler.remove("api-a");
    scheduler.remove("api-b");
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "account");
    scheduler.remove("account");
    scheduler.upsert(oauth_candidate("account"));
    assert!(scheduler.quota_references.is_empty());
}

#[test]
fn reference_source_invalidation_removes_ranking_without_blocking_siwc() {
    let mut scheduler = reference_quota_scheduler();
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "account");
    assert!(scheduler.invalidate_quota_reference_source("linked-bps"));
    assert!(!scheduler.invalidate_quota_reference_source("linked-bps"));
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "api-a");
    scheduler.remove("api-a");
    scheduler.remove("api-b");
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "account");
}

#[test]
fn removing_an_inactive_reference_source_clears_its_ranking() {
    let mut scheduler = reference_quota_scheduler();
    assert!(scheduler.remove("linked-bps").is_none());
    assert!(scheduler.quota_references.is_empty());
    assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "api-a");
}

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
        member.provider_credits_observed_at_ms = Some(100);
        member.quota_reset_at_ms = Some(reset);
        member.last_used_at = Some(reset);
        member.priority = priority;
    }
    for _ in 0..12 {
        assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "api-a");
    }
}

#[test]
fn automatic_uses_fresh_provider_credits_only_when_no_quota_ranks() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    for (id, credits) in [
        ("account", 1_000_000_000_u64),
        ("api-a", 500_000_000),
        ("api-b", 250_000_000),
    ] {
        let member = scheduler.candidates.get_mut(id).unwrap();
        member.quota = CandidateQuota::CreditFallback;
        member.provider_credits_micro_units = Some(credits);
        member.provider_credits_observed_at_ms = Some(100);
        member.quota_updated_at_ms = Some(100);
    }
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "account"
    );

    scheduler.candidates.get_mut("account").unwrap().quota = CandidateQuota::Available(1);
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "account"
    );
}

#[test]
fn session_affinity_switches_only_when_another_balance_leads_by_fifteen_credits() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    for (id, credits) in [
        ("account", 1_000_000_000_u64),
        ("api-a", 500_000_000),
        ("api-b", 250_000_000),
    ] {
        let member = scheduler.candidates.get_mut(id).unwrap();
        member.quota = CandidateQuota::CreditFallback;
        member.provider_credits_micro_units = Some(credits);
        member.provider_credits_observed_at_ms = Some(100);
        member.quota_updated_at_ms = Some(100);
    }
    scheduler.bind_prompt_affinity("session:chat", "account", 100);
    let select_session = |scheduler: &mut PoolScheduler| {
        scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: Some("session:chat"),
                now_ms: 100,
            })
            .unwrap()
    };

    scheduler
        .candidates
        .get_mut("account")
        .unwrap()
        .provider_credits_micro_units = Some(514_000_000);
    assert_eq!(select_session(&mut scheduler).candidate_id, "account");

    scheduler
        .candidates
        .get_mut("account")
        .unwrap()
        .provider_credits_micro_units = Some(485_000_000);
    let selected = select_session(&mut scheduler);
    assert_eq!(selected.candidate_id, "api-a");
    assert_eq!(
        selected.diagnostics.reason,
        SelectionReason::ProviderCredits
    );
}

#[test]
fn stale_provider_credits_do_not_rank_automatic_candidates() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    scheduler.set_quota_stale_after_ms(10);
    let stale = scheduler.candidates.get_mut("account").unwrap();
    stale.quota = CandidateQuota::CreditFallback;
    stale.provider_credits_micro_units = Some(1_000_000_000);
    stale.provider_credits_observed_at_ms = Some(1);
    stale.quota_updated_at_ms = Some(1);
    let fresh = scheduler.candidates.get_mut("api-a").unwrap();
    fresh.quota = CandidateQuota::CreditFallback;
    fresh.provider_credits_micro_units = Some(500_000_000);
    fresh.provider_credits_observed_at_ms = Some(100);
    fresh.quota_updated_at_ms = Some(100);

    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "api-a"
    );
}

#[test]
fn undated_or_future_observations_do_not_win_automatic_ranking() {
    for observed_at_ms in [None, Some(101)] {
        let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
        let account = scheduler.candidates.get_mut("account").unwrap();
        account.quota = CandidateQuota::Available(10_000);
        account.quota_updated_at_ms = observed_at_ms;
        account.provider_credits_micro_units = Some(1_000_000_000);
        account.provider_credits_observed_at_ms = observed_at_ms;
        let fresh = scheduler.candidates.get_mut("api-a").unwrap();
        fresh.provider_credits_micro_units = Some(500_000_000);
        fresh.provider_credits_observed_at_ms = Some(100);
        assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "api-a");
    }
}

#[test]
fn credit_only_admission_expires_without_another_snapshot() {
    for mode in [PoolRoutingMode::Automatic, PoolRoutingMode::InOrder] {
        for unlimited in [false, true] {
            let mut scheduler = policy::mixed(mode);
            scheduler.set_quota_stale_after_ms(10);
            let account = scheduler.candidates.get_mut("account").unwrap();
            account.quota = CandidateQuota::CreditFallback;
            account.provider_credits_micro_units = (!unlimited).then_some(1_000_000_000);
            account.provider_credits_unlimited = unlimited;
            account.provider_credits_observed_at_ms = Some(100);
            assert_eq!(rotation::dispatch(&mut scheduler, 100, None), "account");
            for _ in 0..6 {
                assert_ne!(rotation::dispatch(&mut scheduler, 111, None), "account");
            }
        }
    }
}

#[test]
fn automatic_switches_only_for_a_material_quota_lead() {
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
    scheduler.bind_prompt_affinity("session:quota", "account", 100);
    assert_eq!(
        rotation::dispatch(&mut scheduler, 100, Some("session:quota")),
        "account"
    );

    let api_a = scheduler.candidates.get_mut("api-a").unwrap();
    api_a.quota = CandidateQuota::Available(3_700);
    api_a.quota_updated_at_ms = Some(100);
    assert_eq!(
        rotation::dispatch(&mut scheduler, 100, Some("session:quota")),
        "api-a"
    );
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
        assert!(scheduler.update_candidate_availability_at(
            "account",
            true,
            CandidateHealth::Healthy,
            quota,
            Some(100),
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
fn automatic_owner_yields_for_a_larger_fresh_quota_remainder() {
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
    assert!(scheduler.automatic_response_owner_should_yield(
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
    assert!(!scheduler.automatic_response_owner_should_yield(
        "response", "gpt-5", &protocols, &scope, &tried, 100,
    ));
    let mut policy = scheduler.pool_routing.clone().unwrap();
    policy.mode = PoolRoutingMode::InOrder;
    scheduler.set_pool_routing(policy).unwrap();
    scheduler.candidates.get_mut("api-a").unwrap().quota = CandidateQuota::Available(9_800);
    assert!(!scheduler.automatic_response_owner_should_yield(
        "response", "gpt-5", &protocols, &scope, &tried, 100,
    ));
}

#[test]
fn automatic_response_owner_yields_when_another_credit_balance_leads_by_fifteen() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    for (id, credits) in [
        ("account", 1_000_000_000_u64),
        ("api-a", 500_000_000),
        ("api-b", 250_000_000),
    ] {
        let member = scheduler.candidates.get_mut(id).unwrap();
        member.quota = CandidateQuota::CreditFallback;
        member.provider_credits_micro_units = Some(credits);
        member.provider_credits_observed_at_ms = Some(100);
        member.quota_updated_at_ms = Some(100);
    }
    assert!(scheduler.bind_response_affinity("response", "account", 100));
    let scope = CandidateScope::default();
    let tried = HashSet::new();
    let protocols = [WireApi::Responses, WireApi::ChatCompletions];

    assert!(!scheduler.automatic_response_owner_should_yield(
        "response", "gpt-5", &protocols, &scope, &tried, 100,
    ));
    scheduler
        .candidates
        .get_mut("account")
        .unwrap()
        .provider_credits_micro_units = Some(485_000_000);
    assert!(scheduler.automatic_response_owner_should_yield(
        "response", "gpt-5", &protocols, &scope, &tried, 100,
    ));
}

#[test]
fn automatic_response_owner_uses_the_same_load_priority_as_new_requests() {
    for (busy, owner_quota, other_quota, expected) in [
        ("api-a", 8_000, 9_000, false),
        ("account", 9_000, 8_000, true),
    ] {
        let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
        scheduler.candidates.get_mut("api-b").unwrap().enabled = false;
        for (id, quota) in [("account", owner_quota), ("api-a", other_quota)] {
            let member = scheduler.candidates.get_mut(id).unwrap();
            member.quota = CandidateQuota::Available(quota);
            member.quota_updated_at_ms = Some(100);
        }
        assert!(scheduler.bind_response_affinity("response", "account", 100));
        let lease = scheduler
            .reserve_request(busy, "gpt-5", 100, false)
            .unwrap();
        assert_eq!(
            scheduler.automatic_response_owner_should_yield(
                "response",
                "gpt-5",
                &[WireApi::Responses, WireApi::ChatCompletions],
                &CandidateScope::default(),
                &HashSet::new(),
                100,
            ),
            expected,
            "busy member: {busy}"
        );
        scheduler.release_reservation(lease);
    }
}

#[test]
fn automatic_response_owner_with_unknown_capacity_yields_to_fresh_credits() {
    let mut scheduler = policy::mixed(PoolRoutingMode::Automatic);
    scheduler.candidates.get_mut("api-b").unwrap().enabled = false;
    let owner = scheduler.candidates.get_mut("account").unwrap();
    owner.quota = CandidateQuota::Unknown;
    owner.provider_credits_micro_units = None;
    owner.provider_credits_observed_at_ms = None;
    let other = scheduler.candidates.get_mut("api-a").unwrap();
    other.quota = CandidateQuota::CreditFallback;
    other.provider_credits_micro_units = Some(100_000_000);
    other.provider_credits_observed_at_ms = Some(100);
    assert!(scheduler.bind_response_affinity("response", "account", 100));
    assert!(scheduler.automatic_response_owner_should_yield(
        "response",
        "gpt-5",
        &[WireApi::Responses, WireApi::ChatCompletions],
        &CandidateScope::default(),
        &HashSet::new(),
        100,
    ));
}
