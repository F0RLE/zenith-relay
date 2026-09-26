use super::*;
use crate::scheduler::rotation::{
    AttemptObservation, ExecutionObservation, HealthObservation, SharedRequestBudget,
};
use crate::PoolRoutingMode;

pub(super) fn dispatch(scheduler: &mut PoolScheduler, now: u64, affinity: Option<&str>) -> String {
    let active_before = scheduler.rotation.active_leases();
    let selection = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: affinity,
            now_ms: now,
        })
        .unwrap();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let reservation = scheduler
        .reserve_request_with_operation(
            &selection.candidate_id,
            "gpt-5",
            now,
            false,
            RotationOperation::Text,
            Some(budget.request_id()),
            Some(&selection.rotation_request),
        )
        .unwrap();
    budget.with_budget(|budget| {
        scheduler
            .begin_rotation_dispatch(reservation, budget)
            .unwrap();
        scheduler
            .settle_rotation(
                reservation,
                AttemptObservation {
                    execution: ExecutionObservation::committed(),
                    health: HealthObservation::Success,
                },
                budget,
                now,
            )
            .unwrap();
    });
    assert!(scheduler.release_reservation(reservation));
    assert_eq!(scheduler.rotation.active_leases(), active_before);
    selection.candidate_id
}

#[test]
fn reservations_keep_weighted_group_with_affinity_and_native_aliases() {
    let mut scheduler = policy::mixed(PoolRoutingMode::RoundRobin);
    let mut policy = scheduler.pool_routing.clone().unwrap();
    policy
        .members
        .iter_mut()
        .find(|m| m.id == "api-b")
        .unwrap()
        .weight = 3;
    scheduler.set_pool_routing(policy).unwrap();
    let mut alias = candidate("api-a::chat");
    alias.source_id = "api-a".into();
    alias.protocol = WireApi::ChatCompletions;
    scheduler.upsert(alias);
    scheduler.set_native_route("api-a", true);
    scheduler.bind_prompt_affinity("cache:test", "api-a", 100);
    let mut counts = BTreeMap::new();
    for _ in 0..100 {
        let before = select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id;
        assert_eq!(
            before,
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id
        );
        *counts
            .entry(dispatch(&mut scheduler, 100, Some("cache:test")))
            .or_insert(0) += 1;
    }
    assert_eq!(
        counts,
        BTreeMap::from([
            ("account".into(), 20),
            ("api-a".into(), 20),
            ("api-b".into(), 60)
        ])
    );
}

#[test]
fn configured_order_and_operation_bindings_survive_rotation_admission() {
    let mut scheduler = policy::mixed(PoolRoutingMode::InOrder);
    let mut policy = scheduler.pool_routing.clone().unwrap();
    policy.members.rotate_left(2);
    scheduler.set_pool_routing(policy).unwrap();
    assert_eq!(dispatch(&mut scheduler, 100, None), "api-b");
    let compact = scheduler
        .select_compaction(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 100,
        })
        .unwrap();
    assert_eq!(compact.candidate_id, "account");
    assert!(scheduler
        .rotation
        .select(
            &RotationRequest::new(
                RotationRequestId(99),
                PoolScheduler::rotation_route_key("gpt-5", RotationOperation::Image),
                "gpt-5"
            )
            .with_operation(RotationOperation::Image),
            100
        )
        .is_none());
}
