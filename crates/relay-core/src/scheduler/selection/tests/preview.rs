use super::unified::{dispatch, mixed};
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
        PoolRoutingMode::Smart,
        PoolRoutingMode::InOrder,
        PoolRoutingMode::RoundRobin,
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
    scheduler.release_reservation(lease);
    assert_eq!(next(&scheduler.runtime_order(100)), Some("account"));
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
    chat.source_id = "api-b".into();
    scheduler.upsert(chat);
    assert!(next(&scheduler.runtime_order(100)).is_none());
}
