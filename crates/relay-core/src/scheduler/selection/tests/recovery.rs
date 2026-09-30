use super::*;
use crate::scheduler::rotation::{
    AttemptObservation, CircuitState, ExecutionObservation, HealthObservation, SharedRequestBudget,
};

fn at(scheduler: &mut PoolScheduler, now_ms: u64) -> Option<Selection> {
    scheduler.select(SelectionRequest {
        model: "gpt-5",
        allowed_protocols: &[WireApi::Responses],
        scope: &CandidateScope::default(),
        tried: &HashSet::new(),
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms,
    })
}

fn start(
    scheduler: &mut PoolScheduler,
    id: &str,
    now: u64,
) -> (ReservationId, SharedRequestBudget) {
    let budget = SharedRequestBudget::for_incoming_request(3);
    let slot = scheduler
        .reserve_request_with_operation(
            id,
            "gpt-5",
            now,
            false,
            RotationOperation::Text,
            Some(budget.request_id()),
            None,
        )
        .unwrap();
    budget.with_budget(|b| scheduler.begin_rotation_dispatch(slot, b).unwrap());
    (slot, budget)
}

fn finish(
    scheduler: &mut PoolScheduler,
    attempt: (ReservationId, SharedRequestBudget),
    health: HealthObservation,
    now: u64,
) {
    attempt.1.with_budget(|b| {
        scheduler
            .settle_rotation(
                attempt.0,
                AttemptObservation {
                    execution: ExecutionObservation::not_sent(),
                    health,
                },
                b,
                now,
            )
            .unwrap()
    });
    assert!(scheduler.release_reservation(attempt.0));
}

fn fail(scheduler: &mut PoolScheduler, id: &str, now: u64) {
    let attempt = start(scheduler, id, now);
    finish(
        scheduler,
        attempt,
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
        now,
    );
}

fn circuit(scheduler: &PoolScheduler, id: &str) -> crate::scheduler::rotation::CircuitSnapshot {
    scheduler.rotation.circuit(
        id,
        &PoolScheduler::rotation_route_key("gpt-5", RotationOperation::Text),
    )
}

#[test]
fn last_member_obeys_pacing_and_opens_after_three_distinct_requests() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("only"));
    for (now, next, streak) in [(0, 250, 1), (250, 750, 2), (750, 2_750, 3)] {
        fail(&mut scheduler, "only", now);
        assert_eq!(circuit(&scheduler, "only").failure_streak, streak);
        assert!(at(&mut scheduler, next - 1).is_none());
        assert!(at(&mut scheduler, next).unwrap().half_open_probe);
        let snapshot = scheduler.runtime_order(next - 1).remove(0);
        assert!(!snapshot.available);
        assert_eq!(snapshot.next_retry_at_ms, Some(next));
    }
    assert_eq!(circuit(&scheduler, "only").state, CircuitState::Open);
}

#[test]
fn recovery_success_closes_the_circuit_without_a_refresh_or_restart() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("api-a"));
    scheduler.upsert(candidate("api-b"));
    fail(&mut scheduler, "api-a", 0);
    assert_eq!(at(&mut scheduler, 249).unwrap().candidate_id, "api-b");
    let recovered = at(&mut scheduler, 250).unwrap();
    assert_eq!(recovered.candidate_id, "api-a");
    assert!(recovered.half_open_probe);
    let attempt = start(&mut scheduler, "api-a", 250);
    finish(&mut scheduler, attempt, HealthObservation::Success, 251);
    assert_eq!(circuit(&scheduler, "api-a").state, CircuitState::Closed);
    assert_eq!(circuit(&scheduler, "api-a").failure_streak, 0);
}

#[test]
fn due_recovery_never_overrides_auth_quota_or_request_scope() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("only"));
    fail(&mut scheduler, "only", 0);
    scheduler.set_cooldown("only", "*", 600_000);
    assert!(at(&mut scheduler, 250).is_none());
    scheduler.clear_cooldown("only", "*");
    scheduler.candidates.get_mut("only").unwrap().quota = CandidateQuota::Exhausted;
    assert!(at(&mut scheduler, 250).is_none());
    scheduler.candidates.get_mut("only").unwrap().quota = CandidateQuota::Unknown;
    scheduler.set_candidate_health("only", CandidateHealth::ReauthRequired);
    assert!(at(&mut scheduler, 250).is_none());
    scheduler.set_candidate_health("only", CandidateHealth::Healthy);
    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope {
                source_ids: Some(BTreeSet::new()),
                ..Default::default()
            },
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 250,
        })
        .is_none());
}

#[test]
fn old_request_release_and_success_cannot_release_a_running_recovery_trial() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("only"));
    let old = start(&mut scheduler, "only", 0);
    fail(&mut scheduler, "only", 0);
    let probe = start(&mut scheduler, "only", 250);
    finish(&mut scheduler, old, HealthObservation::Success, 251);
    assert!(at(&mut scheduler, 251).is_none());
    assert_eq!(scheduler.active_request_count("only"), 1);
    assert_eq!(circuit(&scheduler, "only").state, CircuitState::HalfOpen);
    finish(&mut scheduler, probe, HealthObservation::Success, 252);
    assert!(!at(&mut scheduler, 252).unwrap().half_open_probe);
}

#[test]
fn repeated_rate_observation_does_not_release_the_recovery_owner() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("only"));
    fail(&mut scheduler, "only", 0);
    let probe = start(&mut scheduler, "only", 250);
    for retry_at in [249, 248] {
        scheduler.set_cooldown("only", "gpt-5", retry_at);
        assert!(at(&mut scheduler, 250).is_none());
        assert!(scheduler
            .reserve_request("only", "gpt-5", 250, false)
            .is_none());
    }
    finish(&mut scheduler, probe, HealthObservation::Success, 251);
    assert!(!at(&mut scheduler, 251).unwrap().half_open_probe);
}

#[test]
fn recovery_trial_is_busy_but_auth_and_quota_blocks_are_not_capacity_waits() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("only"));
    fail(&mut scheduler, "only", 0);
    let probe = start(&mut scheduler, "only", 250);
    let busy = |scheduler: &mut PoolScheduler, tried: &HashSet<String>| {
        scheduler.capacity_blocked_for(
            SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried,
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 250,
            },
            RotationOperation::Text,
        )
    };
    assert!(busy(&mut scheduler, &HashSet::new()));
    assert!(!busy(&mut scheduler, &HashSet::from(["only".into()])));
    scheduler.candidates.get_mut("only").unwrap().quota = CandidateQuota::Exhausted;
    assert!(!busy(&mut scheduler, &HashSet::new()));
    scheduler.candidates.get_mut("only").unwrap().quota = CandidateQuota::Unknown;
    scheduler.set_candidate_health("only", CandidateHealth::ReauthRequired);
    assert!(!busy(&mut scheduler, &HashSet::new()));
    finish(&mut scheduler, probe, HealthObservation::Unknown, 251);
}

#[test]
fn rate_expiry_is_not_a_circuit_and_does_not_create_an_exclusive_probe() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("only"));
    scheduler.set_cooldown("only", "*", 100);
    assert!(at(&mut scheduler, 99).is_none());
    assert!(!at(&mut scheduler, 100).unwrap().half_open_probe);
    let first = scheduler
        .reserve_request("only", "gpt-5", 100, false)
        .unwrap();
    let second = scheduler
        .reserve_request("only", "gpt-5", 100, false)
        .unwrap();
    assert!(scheduler.release_reservation(first));
    assert!(scheduler.release_reservation(second));
}

#[test]
fn transient_health_cannot_be_injected_by_the_legacy_cooldown_map() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("only"));
    assert!(!scheduler.set_cooldown_with_reason(
        "only",
        "gpt-5",
        10_000,
        CooldownReason::Transient
    ));
    assert!(at(&mut scheduler, 100).is_some());
    assert_eq!(circuit(&scheduler, "only").failure_streak, 0);
}

#[test]
fn compaction_waits_only_on_its_own_route_not_a_ready_text_api() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(oauth_candidate("account"));
    scheduler.upsert(candidate("api"));
    let budget = SharedRequestBudget::for_incoming_request(3);
    let slot = scheduler
        .reserve_request_with_operation(
            "account",
            "gpt-5",
            0,
            false,
            RotationOperation::Compaction,
            Some(budget.request_id()),
            None,
        )
        .unwrap();
    budget.with_budget(|b| scheduler.begin_rotation_dispatch(slot, b).unwrap());
    finish(
        &mut scheduler,
        (slot, budget),
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
        0,
    );
    let scope = CandidateScope::default();
    let tried = HashSet::new();
    let request = || SelectionRequest {
        model: "gpt-5",
        allowed_protocols: &[WireApi::Responses],
        scope: &scope,
        tried: &tried,
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms: 1,
    };
    assert!(scheduler.select(request()).is_some());
    assert!(scheduler.select_compaction(request()).is_none());
    assert_eq!(
        scheduler.recovery_retry_at_for(request(), RotationOperation::Compaction),
        Some(250)
    );
    assert_eq!(
        scheduler.all_applicable_cooldown_for(request(), RotationOperation::Compaction),
        Some((250, CooldownReason::Transient))
    );
    assert!(!scheduler.capacity_blocked_for(request(), RotationOperation::Compaction));
}

#[test]
fn success_projection_never_clears_a_new_auth_block_or_an_inference_circuit() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("only"));
    fail(&mut scheduler, "only", 0);
    scheduler.set_candidate_health("only", CandidateHealth::ReauthRequired);
    scheduler.record_success("only", "gpt-5", 1);
    assert_eq!(
        scheduler.candidate("only").unwrap().health,
        CandidateHealth::ReauthRequired
    );
    assert_eq!(circuit(&scheduler, "only").failure_streak, 1);
    assert!(at(&mut scheduler, 250).is_none());
}
