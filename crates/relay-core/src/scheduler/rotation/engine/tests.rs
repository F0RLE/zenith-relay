use super::*;

fn candidate(id: &str, priority: i32, weight: u32) -> RotationCandidate {
    let mut candidate = RotationCandidate::new(id, "responses:gpt", "gpt-6");
    candidate.priority = priority;
    candidate.weight = weight;
    candidate
}

fn engine_with(candidates: impl IntoIterator<Item = RotationCandidate>) -> RotationEngine {
    let mut engine = RotationEngine::default();
    for candidate in candidates {
        engine.upsert(candidate).unwrap();
    }
    engine
}

fn request(id: u64) -> RotationRequest {
    RotationRequest::new(RequestId(id), "responses:gpt", "gpt-6")
}

fn dispatch(engine: &mut RotationEngine, lease: &RotationLease) -> RequestBudget {
    let mut budget = RequestBudget::default_for(lease.request_id);
    engine.begin_dispatch(lease.lease_id, &mut budget).unwrap();
    budget
}

#[test]
fn allowed_healthy_reserve_is_not_blocked_by_primary_priority() {
    let mut primary = candidate("primary", 100, 1);
    primary.auth = AuthState::Ready;
    let reserve = candidate("reserve", 0, 1);
    let mut engine = engine_with([primary, reserve]);
    engine.set_mode(RotationMode::InOrder);
    let first_request = request(1);
    let lease = engine.reserve(&first_request, 0).unwrap();
    let budget = dispatch(&mut engine, &lease);
    engine
        .settle(
            lease.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::not_sent(),
                health: HealthObservation::CountableTransient {
                    provider_not_before_ms: None,
                },
            },
            &budget,
            0,
        )
        .unwrap();
    assert_eq!(
        engine.circuit("primary", "responses:gpt").state,
        CircuitState::Degraded
    );
    assert!(engine
        .select(&request(2), 251)
        .is_some_and(|selected| { selected.candidate_id == "primary" }));
    assert_eq!(
        engine.select(&request(3), 0).unwrap().candidate_id,
        "reserve"
    );
    assert_eq!(
        engine.select(
            &request(4).with_allowed_candidates(BTreeSet::from(["primary".into()])),
            0
        ),
        None
    );
}

#[test]
fn weighted_preview_is_side_effect_free_and_reservation_advances_state() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 3)]);
    engine.set_mode(RotationMode::RoundRobin);
    let first_request = request(1);
    let preview = engine.select(&first_request, 0).unwrap();
    let second_preview = engine.select(&request(2), 0).unwrap();
    assert_eq!(preview.candidate_id, second_preview.candidate_id);
    let lease = engine.reserve(&first_request, 0).unwrap();
    assert_eq!(lease.candidate_id, preview.candidate_id);
    let next = engine.select(&request(3), 0).unwrap();
    assert_ne!(next.candidate_id, lease.candidate_id);
}

#[test]
fn hard_owner_never_falls_through_to_another_candidate() {
    let mut engine = engine_with([candidate("owner", 100, 1), candidate("other", 100, 1)]);
    let request = request(1).with_owner("owner");
    assert_eq!(engine.select(&request, 0).unwrap().candidate_id, "owner");
    engine.set_auth("owner", AuthState::NeedsRefresh);
    assert_eq!(engine.select(&request, 0), None);
    assert!(matches!(
        engine.candidate_availability(&request, "other", 0),
        CandidateAvailability::Blocked(CandidateBlockReason::OwnerMismatch)
    ));
}

#[test]
fn unknown_and_stale_quota_are_not_invented_as_exhaustion() {
    let mut engine = engine_with([candidate("member", 0, 1)]);
    let request = request(1);
    for quota in [
        QuotaState::Unknown,
        QuotaState::Stale,
        QuotaState::Available,
    ] {
        engine.set_quota("member", quota);
        assert!(matches!(
            engine.candidate_availability(&request, "member", 0),
            CandidateAvailability::Ready { recovery: false }
        ));
    }
    engine.set_quota(
        "member",
        QuotaState::Exhausted {
            reset_at_ms: Some(100),
        },
    );
    assert!(matches!(
        engine.candidate_availability(&request, "member", 0),
        CandidateAvailability::WaitUntil {
            at_ms: 100,
            reason: CandidateBlockReason::QuotaExhausted
        }
    ));
}

#[test]
fn three_distinct_transient_requests_open_one_member_route_only() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 1)]);
    for (request_id, now_ms) in [(1, 0), (2, 5_000), (3, 10_000)] {
        let request = request(request_id).with_owner("a");
        let lease = engine.reserve(&request, now_ms).unwrap();
        let budget = dispatch(&mut engine, &lease);
        let snapshot = engine
            .settle(
                lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::not_sent(),
                    health: HealthObservation::CountableTransient {
                        provider_not_before_ms: None,
                    },
                },
                &budget,
                now_ms,
            )
            .unwrap()
            .circuit;
        if request_id == 3 {
            assert_eq!(snapshot.state, CircuitState::Open);
        }
    }
    assert_eq!(
        engine.circuit("a", "responses:gpt").state,
        CircuitState::Open
    );
    assert_eq!(
        engine.circuit("b", "responses:gpt").state,
        CircuitState::Closed
    );
}

#[test]
fn same_request_does_not_vote_twice_and_success_breaks_the_streak() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    for now_ms in [0, 5_000] {
        let request = request(1).with_owner("a");
        let lease = engine.reserve(&request, now_ms).unwrap();
        let budget = dispatch(&mut engine, &lease);
        engine
            .settle(
                lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::not_sent(),
                    health: HealthObservation::CountableTransient {
                        provider_not_before_ms: None,
                    },
                },
                &budget,
                now_ms,
            )
            .unwrap();
    }
    assert_eq!(engine.circuit("a", "responses:gpt").failure_streak, 1);
    let request = request(2).with_owner("a");
    let lease = engine.reserve(&request, 10_000).unwrap();
    let budget = dispatch(&mut engine, &lease);
    engine
        .settle(
            lease.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::not_sent(),
                health: HealthObservation::Success,
            },
            &budget,
            10_000,
        )
        .unwrap();
    assert_eq!(
        engine.circuit("a", "responses:gpt").state,
        CircuitState::Closed
    );
    assert_eq!(engine.circuit("a", "responses:gpt").failure_streak, 0);
}

#[test]
fn recovery_has_one_real_request_permit_and_cancel_does_not_count_as_failure() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    for (request_id, now_ms) in [(1, 0), (2, 5_000), (3, 10_000)] {
        let request = request(request_id).with_owner("a");
        let lease = engine.reserve(&request, now_ms).unwrap();
        let budget = dispatch(&mut engine, &lease);
        engine
            .settle(
                lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::not_sent(),
                    health: HealthObservation::CountableTransient {
                        provider_not_before_ms: None,
                    },
                },
                &budget,
                now_ms,
            )
            .unwrap();
    }
    let due = engine.circuit("a", "responses:gpt").not_before_ms.unwrap();
    let first_request = request(4).with_owner("a");
    let first = engine.reserve(&first_request, due).unwrap();
    assert!(first.recovery);
    assert_eq!(
        engine.reserve(&request(5).with_owner("a"), due),
        Err(AdmissionError::NoEligibleCandidate)
    );
    let budget = RequestBudget::default_for(first_request.request_id);
    engine.cancel(first.lease_id, &budget, due).unwrap();
    assert_eq!(engine.circuit("a", "responses:gpt").failure_streak, 3);
    assert!(engine.reserve(&request(6).with_owner("a"), due).is_err());
    let second = engine
        .reserve(&request(6).with_owner("a"), due + FIRST_TRANSIENT_PACING_MS)
        .unwrap();
    assert!(second.recovery);
}

#[test]
fn stale_lease_release_cannot_release_a_new_reservation() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    let first = engine.reserve(&request(1), 0).unwrap();
    let budget = RequestBudget::default_for(RequestId(1));
    engine.cancel(first.lease_id, &budget, 0).unwrap();
    let second = engine.reserve(&request(2), 0).unwrap();
    assert_eq!(engine.in_flight("a"), 1);
    assert_eq!(
        engine.settle(
            first.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::unknown(),
                health: HealthObservation::Unknown,
            },
            &budget,
            0,
        ),
        Err(SettlementError::UnknownLease)
    );
    assert_eq!(engine.in_flight("a"), 1);
    let second_budget = RequestBudget::default_for(RequestId(2));
    engine.cancel(second.lease_id, &second_budget, 0).unwrap();
    assert_eq!(engine.in_flight("a"), 0);
}

#[test]
fn dispatch_budget_is_charged_at_start_and_not_refunded_by_cancel() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    let request = request(19);
    let mut budget = RequestBudget::new(request.request_id, 1).unwrap();
    let lease = engine.reserve(&request, 0).unwrap();
    let mut wrong = RequestBudget::default_for(RequestId(20));
    assert_eq!(
        engine.begin_dispatch(lease.lease_id, &mut wrong),
        Err(DispatchStartError::BudgetRequestMismatch)
    );
    assert_eq!(
        engine.begin_dispatch(lease.lease_id, &mut budget),
        Ok(AttemptId(1))
    );
    assert_eq!(
        engine.begin_dispatch(lease.lease_id, &mut budget),
        Err(DispatchStartError::AlreadyStarted)
    );
    assert_eq!(
        engine.cancel(lease.lease_id, &wrong, 1),
        Err(SettlementError::BudgetRequestMismatch)
    );
    assert_eq!(engine.in_flight("a"), 1);
    engine.cancel(lease.lease_id, &budget, 1).unwrap();
    assert_eq!(budget.remaining(), 0);
    assert_eq!(engine.in_flight("a"), 0);
}

#[test]
fn policy_upsert_does_not_erase_an_active_reservation() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    let first_request = request(1);
    let lease = engine.reserve(&first_request, 0).unwrap();
    let mut changed = candidate("a", 0, 3);
    changed.max_concurrency = 1;
    engine.upsert(changed).unwrap();
    assert_eq!(engine.in_flight("a"), 1);
    assert_eq!(engine.select(&request(2), 0), None);
    engine
        .cancel(
            lease.lease_id,
            &RequestBudget::default_for(first_request.request_id),
            0,
        )
        .unwrap();
    assert_eq!(engine.in_flight("a"), 0);
}

#[test]
fn late_success_does_not_clear_a_newer_failure() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    let first = request(1).with_owner("a");
    let second = request(2).with_owner("a");
    let old = engine.reserve(&first, 0).unwrap();
    let new = engine.reserve(&second, 0).unwrap();
    let old_budget = dispatch(&mut engine, &old);
    let new_budget = dispatch(&mut engine, &new);
    engine
        .settle(
            new.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::not_sent(),
                health: HealthObservation::CountableTransient {
                    provider_not_before_ms: None,
                },
            },
            &new_budget,
            1,
        )
        .unwrap();
    let before = engine.circuit("a", "responses:gpt");
    engine
        .settle(
            old.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::accepted(),
                health: HealthObservation::Success,
            },
            &old_budget,
            2,
        )
        .unwrap();
    assert_eq!(engine.circuit("a", "responses:gpt"), before);
    assert_eq!(engine.in_flight("a"), 0);
}

#[test]
fn transient_pacing_is_short_and_open_recovery_backoff_is_bounded() {
    assert_eq!(failure_backoff_ms(1), 250);
    assert_eq!(failure_backoff_ms(2), 500);
    assert_eq!(failure_backoff_ms(3), 2_000);
    assert_eq!(failure_backoff_ms(4), 4_000);
    assert_eq!(failure_backoff_ms(20), 60_000);
}

#[test]
fn concurrent_failures_each_vote_but_a_late_success_does_not_close_them() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    let mut leases = Vec::new();
    for id in 1..=3 {
        leases.push(engine.reserve(&request(id).with_owner("a"), 0).unwrap());
    }
    let budgets: Vec<_> = leases
        .iter()
        .map(|lease| dispatch(&mut engine, lease))
        .collect();
    for (lease, budget) in leases.into_iter().zip(budgets) {
        engine
            .settle(
                lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::not_sent(),
                    health: HealthObservation::CountableTransient {
                        provider_not_before_ms: None,
                    },
                },
                &budget,
                0,
            )
            .unwrap();
    }
    assert_eq!(engine.circuit("a", "responses:gpt").failure_streak, 3);
    assert_eq!(
        engine.circuit("a", "responses:gpt").state,
        CircuitState::Open
    );
}

#[test]
fn physical_capacity_is_shared_by_verified_aliases() {
    let mut alias_a = candidate("alias-a", 0, 1);
    alias_a.max_concurrency = 1;
    alias_a.capacity_key = "physical-a".to_owned();
    let mut alias_b = candidate("alias-b", 0, 1);
    alias_b.max_concurrency = 1;
    alias_b.capacity_key = "physical-a".to_owned();
    let mut engine = engine_with([alias_a, alias_b]);
    let first = engine.reserve(&request(1), 0).unwrap();
    assert_eq!(engine.capacity_in_flight("physical-a"), 1);
    assert!(matches!(
        engine.candidate_availability(&request(2), "alias-b", 0),
        CandidateAvailability::Busy(CandidateBlockReason::CapacityBusy)
    ));
    engine
        .cancel(first.lease_id, &RequestBudget::default_for(RequestId(1)), 0)
        .unwrap();
    assert_eq!(engine.capacity_in_flight("physical-a"), 0);
    assert!(engine.reserve(&request(2), 0).is_ok());
}

#[test]
fn operation_and_recovery_policy_keep_healthy_traffic_bounded() {
    let mut failed = candidate("failed", 0, 1).with_operation(RotationOperation::Text);
    failed.max_concurrency = 1;
    let healthy = candidate("healthy", 0, 1).with_operation(RotationOperation::Text);
    let image = candidate("image", 0, 1).with_operation(RotationOperation::Image);
    let mut engine = engine_with([failed, healthy, image]);
    for id in 1..=3 {
        let lease = engine
            .reserve(&request(id).with_owner("failed"), id * 5_000)
            .unwrap();
        let budget = dispatch(&mut engine, &lease);
        engine
            .settle(
                lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::not_sent(),
                    health: HealthObservation::CountableTransient {
                        provider_not_before_ms: None,
                    },
                },
                &budget,
                id * 5_000,
            )
            .unwrap();
    }
    let due = engine
        .circuit("failed", "responses:gpt")
        .not_before_ms
        .unwrap();
    let recovery = engine.reserve(&request(10), due).unwrap();
    assert!(recovery.recovery);
    let budget = RequestBudget::default_for(RequestId(10));
    let mut dispatch_budget = budget;
    engine
        .begin_dispatch(recovery.lease_id, &mut dispatch_budget)
        .unwrap();
    assert_eq!(engine.recovery_credits(), 0);
    engine
        .settle(
            recovery.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::not_sent(),
                health: HealthObservation::Success,
            },
            &dispatch_budget,
            due,
        )
        .unwrap();
    assert!(matches!(
        engine.candidate_availability(
            &request(11).with_operation(RotationOperation::Image),
            "healthy",
            due
        ),
        CandidateAvailability::Blocked(CandidateBlockReason::OperationUnavailable)
    ));
}

#[test]
fn remove_and_readd_fences_old_capacity_release() {
    let mut original = candidate("source", 0, 1).with_capacity("physical", 1);
    original.max_concurrency = 1;
    let mut engine = engine_with([original]);
    let old = engine.reserve(&request(1), 0).unwrap();
    let removed = engine.remove("source").unwrap();
    assert_eq!(removed.id, "source");
    let replacement = candidate("source", 0, 1).with_capacity("physical", 1);
    engine.upsert(replacement).unwrap();
    assert_eq!(engine.capacity_in_flight("physical"), 1);
    assert!(matches!(
        engine.candidate_availability(&request(2), "source", 0),
        CandidateAvailability::Busy(CandidateBlockReason::CapacityBusy)
    ));
    engine
        .cancel(old.lease_id, &RequestBudget::default_for(RequestId(1)), 0)
        .unwrap();
    assert_eq!(engine.capacity_in_flight("physical"), 0);
    assert!(engine.reserve(&request(2), 0).is_ok());
}

#[test]
fn stale_quota_observations_cannot_clear_a_newer_block() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    assert!(engine.set_quota_if_newer("a", QuotaState::Exhausted { reset_at_ms: None }, 2));
    assert!(!engine.set_quota_if_newer("a", QuotaState::Available, 1));
    assert_eq!(
        engine.candidate("a").map(|candidate| candidate.quota),
        Some(QuotaState::Exhausted { reset_at_ms: None })
    );
}

#[test]
fn reserving_a_recovery_does_not_spend_credit_until_dispatch() {
    let failed = candidate("failed", 0, 1);
    let healthy = candidate("healthy", 0, 1);
    let mut engine = engine_with([failed.clone(), healthy]);
    for id in 1..=3 {
        let lease = engine
            .reserve(&request(id).with_owner("failed"), id * 5_000)
            .unwrap();
        let budget = dispatch(&mut engine, &lease);
        engine
            .settle(
                lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::not_sent(),
                    health: HealthObservation::CountableTransient {
                        provider_not_before_ms: None,
                    },
                },
                &budget,
                id * 5_000,
            )
            .unwrap();
    }
    let due = engine
        .circuit("failed", "responses:gpt")
        .not_before_ms
        .unwrap();
    let recovery = engine.reserve(&request(4), due).unwrap();
    assert_eq!(engine.recovery_credits(), 1);
    engine
        .cancel(
            recovery.lease_id,
            &RequestBudget::default_for(RequestId(4)),
            due,
        )
        .unwrap();
    assert_eq!(engine.recovery_credits(), 1);
}
