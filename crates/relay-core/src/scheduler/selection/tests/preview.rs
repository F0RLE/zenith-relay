use super::policy::{dispatch, mixed};
use super::*;
use crate::PoolRoutingMode;

fn next(order: &[CandidateRuntimeSnapshot]) -> Option<&str> {
    order
        .iter()
        .find(|candidate| candidate.next_for_new_request)
        .map(|candidate| candidate.candidate_id.as_str())
}

#[test]
fn preview_matches_dispatch_in_every_mode_without_advancing_rotation() {
    for mode in [
        PoolRoutingMode::Automatic,
        PoolRoutingMode::InOrder,
        PoolRoutingMode::RoundRobin,
        PoolRoutingMode::WeightedRoundRobin,
        PoolRoutingMode::Failover,
    ] {
        let mut scheduler = mixed(mode);
        for _ in 0..24 {
            let selected = select(&mut scheduler, &HashSet::new()).unwrap();
            for _ in 0..3 {
                assert_eq!(
                    next(&scheduler.runtime_order(100)),
                    Some(selected.candidate_id.as_str())
                );
            }
            assert_eq!(dispatch(&mut scheduler), selected.candidate_id);
        }
    }
}

#[test]
fn display_preference_matches_fallback_selection_in_every_mode() {
    for mode in [
        PoolRoutingMode::Automatic,
        PoolRoutingMode::InOrder,
        PoolRoutingMode::RoundRobin,
        PoolRoutingMode::WeightedRoundRobin,
        PoolRoutingMode::Failover,
    ] {
        let mut scheduler = mixed(mode);
        let mut policy = scheduler.pool_routing.clone().unwrap();
        policy.members[1].weight = 3;
        scheduler.set_pool_routing(policy).unwrap();
        for _ in 0..12 {
            let snapshots = scheduler.runtime_order(100);
            assert_eq!(snapshots, scheduler.runtime_order(100));
            let mut ranked = snapshots
                .iter()
                .filter(|candidate| candidate.selection_rank.is_some())
                .collect::<Vec<_>>();
            ranked.sort_by_key(|candidate| candidate.selection_rank);
            assert_eq!(ranked.len(), 3);
            let mut tried = HashSet::new();
            for (rank, candidate) in ranked.iter().enumerate() {
                assert_eq!(candidate.selection_rank, Some(rank));
                let selected = select(&mut scheduler, &tried).unwrap();
                assert_eq!(candidate.candidate_id, selected.candidate_id);
                tried.insert(selected.candidate_id);
            }
            assert_eq!(dispatch(&mut scheduler), ranked[0].candidate_id);
        }
    }
}

#[test]
fn display_preference_wraps_from_the_live_round_robin_cursor() {
    let mut scheduler = mixed(PoolRoutingMode::RoundRobin);
    assert_eq!(dispatch(&mut scheduler), "account");
    let mut snapshots = scheduler.runtime_order(100);
    snapshots.sort_by_key(|candidate| candidate.selection_rank);
    assert_eq!(
        snapshots
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>(),
        ["api-a", "api-b", "account"]
    );
}

#[test]
fn display_preference_covers_a_large_pool_without_advancing_it() {
    let mut scheduler = PoolScheduler::new();
    for index in 0..64 {
        let mut account = oauth_candidate(&format!("account-{index:03}"));
        account.models.extend(["model-b".into(), "model-c".into()]);
        scheduler.upsert(account);
    }
    let started = std::time::Instant::now();
    let snapshots = scheduler.runtime_order(100);
    let elapsed = started.elapsed();
    assert_eq!(snapshots, scheduler.runtime_order(100));
    assert_eq!(
        snapshots
            .iter()
            .filter_map(|candidate| candidate.selection_rank)
            .collect::<BTreeSet<_>>(),
        (0..64).collect()
    );
    eprintln!("64-member, 3-model read-only preference: {elapsed:?}");
}

#[test]
fn display_preference_omits_blocked_members_and_stops_at_ambiguous_fallbacks() {
    let mut scheduler = mixed(PoolRoutingMode::Failover);
    scheduler.candidates.get_mut("account").unwrap().draining = true;
    scheduler
        .candidates
        .get_mut("api-a")
        .unwrap()
        .cooldowns
        .insert("gpt-5".into(), 200);
    let snapshots = scheduler.runtime_order(100);
    assert_eq!(
        snapshots
            .iter()
            .filter(|candidate| candidate.selection_rank.is_some())
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>(),
        ["api-b"]
    );

    let mut scheduler = mixed(PoolRoutingMode::Failover);
    for id in ["account", "api-b"] {
        scheduler
            .candidates
            .get_mut(id)
            .unwrap()
            .models
            .insert("other-model".into());
    }
    let snapshots = scheduler.runtime_order(100);
    assert_eq!(next(&snapshots), Some("account"));
    assert_eq!(
        snapshots
            .iter()
            .filter(|candidate| candidate.selection_rank.is_some())
            .count(),
        1
    );
}

#[test]
fn preview_keeps_a_busy_primary_until_its_member_capacity_is_full() {
    let mut scheduler = mixed(PoolRoutingMode::InOrder);
    let lease = scheduler
        .reserve_request("account", "gpt-5", 100, false)
        .unwrap();
    assert_eq!(next(&scheduler.runtime_order(100)), Some("account"));
    let mut policy = scheduler.pool_routing.clone().unwrap();
    policy.members[0].max_concurrency = 1;
    scheduler.set_pool_routing(policy).unwrap();
    assert_eq!(next(&scheduler.runtime_order(100)), Some("api-a"));
    assert!(
        scheduler
            .runtime_order(100)
            .iter()
            .find(|candidate| candidate.candidate_id == "account")
            .unwrap()
            .capacity_wait
    );
    scheduler.release_reservation(lease);
    assert_eq!(next(&scheduler.runtime_order(100)), Some("account"));
    assert!(
        !scheduler
            .runtime_order(100)
            .iter()
            .find(|candidate| candidate.candidate_id == "account")
            .unwrap()
            .capacity_wait
    );
}

#[test]
fn preview_does_not_guess_a_single_candidate_for_different_model_routes() {
    let mut scheduler = mixed(PoolRoutingMode::InOrder);
    scheduler
        .candidates
        .get_mut("api-b")
        .unwrap()
        .models
        .insert("other-model".into());
    assert!(next(&scheduler.runtime_order(100)).is_none());
    assert!(scheduler
        .runtime_order(100)
        .iter()
        .all(|candidate| candidate.selection_rank.is_none()));
    let rules = crate::ModelRules {
        allowed: BTreeSet::from(["gpt-5".into()]),
        ..Default::default()
    };
    let scope = CandidateScope {
        source_ids: Some(BTreeSet::from(["api-a".into()])),
        ..Default::default()
    };
    assert_eq!(
        next(&scheduler.runtime_order_for(&scope, &rules, &[WireApi::Responses], 100)),
        Some("api-a")
    );
}

#[test]
fn preview_agrees_across_bindings_of_the_same_member_but_not_other_providers() {
    let mut scheduler = mixed(PoolRoutingMode::InOrder);
    scheduler.remove("account");
    let mut chat = candidate("api-a::chat");
    chat.source_id = "api-a".into();
    chat.protocol = WireApi::ChatCompletions;
    scheduler.upsert(chat.clone());
    assert_eq!(next(&scheduler.runtime_order(100)), Some("api-a"));
    let snapshots = scheduler.runtime_order(100);
    for id in ["api-a", "api-a::chat"] {
        assert_eq!(
            snapshots
                .iter()
                .find(|candidate| candidate.candidate_id == id)
                .unwrap()
                .selection_rank,
            Some(0)
        );
    }
    chat.source_id = "api-b".into();
    scheduler.upsert(chat);
    assert!(next(&scheduler.runtime_order(100)).is_none());
}
