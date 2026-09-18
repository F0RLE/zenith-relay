use super::*;
use crate::{PoolMemberKind, PoolRoutingMode, PoolRoutingPolicy};

pub(super) fn mixed(mode: PoolRoutingMode) -> PoolScheduler {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(oauth_candidate("account"));
    scheduler.upsert(candidate("api-a"));
    scheduler.upsert(candidate("api-b"));
    let mut policy = scheduler.migrated_pool_routing();
    policy.mode = mode;
    scheduler.set_pool_routing(policy).unwrap();
    scheduler
}

pub(super) fn dispatch(scheduler: &mut PoolScheduler) -> String {
    let selection = select(scheduler, &HashSet::new()).unwrap();
    assert!(scheduler.reserve_for(&selection.candidate_id, "gpt-5", 100));
    scheduler.commit_rotation(&selection, false);
    assert!(scheduler.release_for(&selection.candidate_id, Some("gpt-5")));
    selection.candidate_id
}

#[test]
fn unified_order_skips_unavailable_accounts_and_falls_back_between_apis() {
    let mut scheduler = mixed(PoolRoutingMode::InOrder);
    assert_eq!(dispatch(&mut scheduler), "account");
    scheduler.candidates.get_mut("account").unwrap().health = CandidateHealth::ReauthRequired;
    assert_eq!(dispatch(&mut scheduler), "api-a");
    scheduler
        .candidates
        .get_mut("api-a")
        .unwrap()
        .cooldowns
        .insert("gpt-5".into(), 200);
    assert_eq!(dispatch(&mut scheduler), "api-b");
    scheduler.remove("account");
    assert_eq!(dispatch(&mut scheduler), "api-b");
    scheduler.clear_cooldown("api-a", "gpt-5");
    assert_eq!(dispatch(&mut scheduler), "api-a");
}

#[test]
fn unified_order_can_place_apis_before_accounts_without_type_gates() {
    let mut scheduler = mixed(PoolRoutingMode::InOrder);
    let mut policy = scheduler.pool_routing.clone().unwrap();
    policy.members.rotate_left(1);
    scheduler.set_pool_routing(policy).unwrap();
    assert_eq!(dispatch(&mut scheduler), "api-a");
    assert_eq!(
        select(&mut scheduler, &HashSet::from(["api-a".into()]))
            .unwrap()
            .candidate_id,
        "api-b"
    );
    assert_eq!(
        select(
            &mut scheduler,
            &HashSet::from(["api-a".into(), "api-b".into()])
        )
        .unwrap()
        .candidate_id,
        "account"
    );
}

#[test]
fn round_robin_weights_count_physical_members_once() {
    let mut scheduler = mixed(PoolRoutingMode::RoundRobin);
    let mut alias = candidate("api-a::chat");
    alias.source_id = "api-a".into();
    alias.protocol = WireApi::ChatCompletions;
    scheduler.upsert(alias);
    let mut policy = scheduler.pool_routing.clone().unwrap();
    policy
        .members
        .iter_mut()
        .find(|m| m.id == "api-b")
        .unwrap()
        .weight = 3;
    scheduler.set_pool_routing(policy).unwrap();
    let mut counts = BTreeMap::new();
    for _ in 0..100 {
        let id = dispatch(&mut scheduler);
        let member = super::super::unified::member_key(&scheduler.candidates[&id]);
        *counts.entry(member).or_insert(0) += 1;
    }
    assert_eq!(counts["account:account"], 20);
    assert_eq!(counts["source:api-a"], 20);
    assert_eq!(counts["source:api-b"], 60);
}

#[test]
fn previews_do_not_advance_rotation_and_hot_updates_keep_active_leases() {
    let mut scheduler = mixed(PoolRoutingMode::RoundRobin);
    for _ in 0..10 {
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "account"
        );
    }
    assert_eq!(dispatch(&mut scheduler), "account");
    assert_eq!(dispatch(&mut scheduler), "api-a");
    assert!(scheduler.reserve_for("api-a", "gpt-5", 100));
    let mut policy = scheduler.pool_routing.clone().unwrap();
    policy.mode = PoolRoutingMode::InOrder;
    policy.members.rotate_left(1);
    policy.members[0].max_concurrency = 1;
    scheduler.set_pool_routing(policy).unwrap();
    assert_eq!(scheduler.active_request_count("api-a"), 1);
    assert_eq!(dispatch(&mut scheduler), "api-b");
    assert!(scheduler.release_for("api-a", Some("gpt-5")));
    assert_eq!(dispatch(&mut scheduler), "api-a");
}

#[test]
fn member_capacity_covers_protocols_and_lanes_and_survives_removal() {
    let mut scheduler = mixed(PoolRoutingMode::InOrder);
    let mut alias = candidate("api-a::chat");
    alias.source_id = "api-a".into();
    scheduler.upsert(alias);
    let mut policy = scheduler.pool_routing.clone().unwrap();
    policy
        .members
        .iter_mut()
        .find(|m| m.id == "api-a")
        .unwrap()
        .max_concurrency = 1;
    scheduler.set_pool_routing(policy).unwrap();
    assert!(scheduler.reserve_image_for("api-a::chat", "gpt-image-2", 100));
    assert!(!scheduler.reserve_for("api-a", "gpt-5", 100));
    scheduler.remove("api-a::chat");
    assert!(!scheduler.reserve_for("api-a", "gpt-5", 100));
    assert!(scheduler.release_image_for("api-a::chat", Some("gpt-image-2")));
    assert!(scheduler.reserve_for("api-a", "gpt-5", 100));
}

#[test]
fn ownership_overrides_rotation_but_soft_affinity_does_not_override_order() {
    let mut scheduler = mixed(PoolRoutingMode::InOrder);
    scheduler.bind_prompt_affinity("cache:prompt", "api-b", 100);
    scheduler.bind_response_affinity("response", "api-b", 100);
    let request = SelectionRequest {
        model: "gpt-5",
        allowed_protocols: &[WireApi::Responses],
        scope: &CandidateScope::default(),
        tried: &HashSet::new(),
        response_affinity_key: None,
        prompt_affinity_key: Some("cache:prompt"),
        now_ms: 100,
    };
    assert_eq!(scheduler.select(request).unwrap().candidate_id, "account");
    let request = SelectionRequest {
        model: "gpt-5",
        allowed_protocols: &[WireApi::Responses],
        scope: &CandidateScope::default(),
        tried: &HashSet::new(),
        response_affinity_key: Some("response"),
        prompt_affinity_key: None,
        now_ms: 100,
    };
    assert_eq!(scheduler.select(request).unwrap().candidate_id, "api-b");
    scheduler.candidates.get_mut("api-b").unwrap().enabled = false;
    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 100,
        })
        .is_none());
}

#[test]
fn smart_spreads_neutral_members_and_avoids_busy_and_exhausted_members() {
    let mut scheduler = mixed(PoolRoutingMode::Smart);
    let selected: BTreeSet<_> = (0..12).map(|_| dispatch(&mut scheduler)).collect();
    assert_eq!(selected.len(), 3);
    scheduler.candidates.get_mut("account").unwrap().quota = CandidateQuota::Exhausted;
    assert!(scheduler.reserve_for("api-a", "gpt-5", 100));
    assert_eq!(dispatch(&mut scheduler), "api-b");
}

#[test]
fn smart_selection_is_independent_of_saved_manual_order() {
    let sequence = |reverse: bool| {
        let mut scheduler = mixed(PoolRoutingMode::Smart);
        scheduler.upsert(candidate("api-c"));
        let mut policy = scheduler.migrated_pool_routing();
        if reverse {
            policy.members.reverse();
        }
        scheduler.set_pool_routing(policy).unwrap();
        (0..24)
            .map(|_| dispatch(&mut scheduler))
            .collect::<Vec<_>>()
    };
    let selected = sequence(false);
    assert_eq!(selected, sequence(true));
    for id in ["account", "api-a", "api-b", "api-c"] {
        assert_eq!(
            selected.iter().filter(|selected| *selected == id).count(),
            6
        );
    }
}

#[test]
fn smart_does_not_keep_preferring_quota_that_aged_without_a_refresh() {
    let mut scheduler = mixed(PoolRoutingMode::Smart);
    scheduler.set_quota_stale_after_ms(10);
    let account = scheduler.candidates.get_mut("account").unwrap();
    account.quota = CandidateQuota::Available(10_000);
    account.quota_updated_at_ms = Some(1);

    let selected: BTreeSet<_> = (0..12).map(|_| dispatch(&mut scheduler)).collect();
    assert_eq!(
        selected,
        BTreeSet::from(["account".into(), "api-a".into(), "api-b".into()])
    );
    // Aging removes a preference; it does not erase a confirmed quota limit.
    scheduler.candidates.get_mut("account").unwrap().quota = CandidateQuota::Exhausted;
    assert_ne!(dispatch(&mut scheduler), "account");
}

#[test]
fn smart_cache_affinity_stays_within_the_best_scoring_group() {
    let mut scheduler = mixed(PoolRoutingMode::Smart);
    for (id, remaining) in [("account", 10_000), ("api-a", 8_000), ("api-b", 6_000)] {
        scheduler.candidates.get_mut(id).unwrap().quota = CandidateQuota::Available(remaining);
    }
    scheduler.bind_prompt_affinity("cache:prompt", "api-b", 100);
    let mut selected = BTreeSet::new();
    for _ in 0..12 {
        let selection = scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: Some("cache:prompt"),
                now_ms: 100,
            })
            .unwrap();
        assert_ne!(selection.candidate_id, "api-b");
        let slot = scheduler
            .reserve_request(&selection.candidate_id, "gpt-5", 100, false)
            .unwrap();
        scheduler.commit_rotation(&selection, false);
        scheduler.release_reservation(slot);
        selected.insert(selection.candidate_id);
    }
    assert_eq!(selected, BTreeSet::from(["account".into(), "api-a".into()]));
}

#[test]
fn smart_cache_affinity_within_the_best_group_still_counts_toward_rotation() {
    let mut scheduler = mixed(PoolRoutingMode::Smart);
    scheduler.bind_prompt_affinity("cache:prompt", "api-b", 100);
    let selection = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: Some("cache:prompt"),
            now_ms: 100,
        })
        .unwrap();
    assert_eq!(selection.candidate_id, "api-b");
    assert_eq!(
        selection.diagnostics.reason,
        SelectionReason::PromptCacheAffinity
    );
    let slot = scheduler
        .reserve_request(&selection.candidate_id, "gpt-5", 100, false)
        .unwrap();
    scheduler.commit_rotation(&selection, false);
    scheduler.release_reservation(slot);
    assert_eq!(dispatch(&mut scheduler), "account");
    assert_eq!(dispatch(&mut scheduler), "api-a");
}

#[test]
fn migration_matches_shared_inventory_order() {
    let scheduler = mixed(PoolRoutingMode::Smart);
    let expected = crate::resolve_pool_routing(
        None,
        vec![
            (PoolMemberKind::Source, "api-a".into(), 0, 1),
            (PoolMemberKind::Account, "account".into(), 0, 1),
            (PoolMemberKind::Source, "api-b".into(), 0, 1),
        ],
    );
    assert_eq!(scheduler.pool_routing, Some(expected));
    assert_eq!(PoolRoutingPolicy::default().mode, PoolRoutingMode::Smart);
}
