use super::*;

fn request(id: u64) -> RotationRequest {
    RotationRequest::new(RequestId(id), "responses:text", "text-model")
}

fn member(id: &str) -> RotationCandidate {
    RotationCandidate::new(id, "responses:text", "text-model")
}

fn engine(ids: &[&str]) -> RotationEngine {
    let mut engine = RotationEngine::default();
    for id in ids {
        engine.upsert(member(id)).unwrap();
    }
    engine
}

fn outcome(
    engine: &mut RotationEngine,
    lease: RotationLease,
    health: HealthObservation,
    now: u64,
) -> RotationSettlement {
    let mut budget = RequestBudget::default_for(lease.request_id);
    engine.begin_dispatch(lease.lease_id, &mut budget).unwrap();
    engine
        .settle(
            lease.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::not_sent(),
                health,
            },
            &budget,
            now,
        )
        .unwrap()
}

fn fail(engine: &mut RotationEngine, member: &str, id: u64, now: u64) {
    let lease = engine
        .reserve(&request(id).with_owner(member), now)
        .unwrap();
    outcome(
        engine,
        lease,
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
        now,
    );
}

#[test]
fn exact_route_binding_does_not_cross_product_model_or_operation() {
    let mut candidate = member("a");
    candidate.add_route("images:image", "image-model", RotationOperation::Image);
    let mut engine = engine(&[]);
    engine.upsert(candidate).unwrap();
    let invalid = RotationRequest::new(RequestId(1), "responses:text", "image-model")
        .with_operation(RotationOperation::Image);
    assert_eq!(engine.select(&invalid, 0), None);
    assert!(engine
        .select(
            &RotationRequest::new(RequestId(2), "images:image", "image-model")
                .with_operation(RotationOperation::Image),
            0
        )
        .is_some());
    assert_eq!(
        engine.select(&request(3).with_operation(RotationOperation::Metadata), 0),
        None
    );
}

#[test]
fn runtime_capacity_is_finite_and_shrink_preserves_every_live_lease() {
    let mut engine = engine(&["a", "b"]);
    engine.set_max_in_flight(2).unwrap();
    let a = engine.reserve(&request(1), 0).unwrap();
    let b = engine.reserve(&request(2), 0).unwrap();
    assert_eq!(engine.active_leases(), 2);
    engine.set_max_in_flight(1).unwrap();
    assert_eq!(engine.select(&request(3), 0), None);
    engine
        .cancel(a.lease_id, &RequestBudget::default_for(a.request_id), 0)
        .unwrap();
    assert_eq!(engine.select(&request(3), 0), None);
    engine
        .cancel(b.lease_id, &RequestBudget::default_for(b.request_id), 0)
        .unwrap();
    assert!(engine.select(&request(3), 0).is_some());
}

#[test]
fn old_identity_release_does_not_decrement_new_identity_load() {
    let mut engine = engine(&["a"]);
    let old = engine.reserve(&request(1), 0).unwrap();
    engine.remove("a").unwrap();
    engine
        .upsert(member("a").with_capacity("new-resource", 1))
        .unwrap();
    let new = engine.reserve(&request(2), 0).unwrap();
    engine
        .cancel(old.lease_id, &RequestBudget::default_for(old.request_id), 0)
        .unwrap();
    assert_eq!(engine.in_flight("a"), 1);
    assert_eq!(engine.capacity_in_flight("new-resource"), 1);
    assert!(engine.reserve(&request(3), 0).is_err());
    engine
        .cancel(new.lease_id, &RequestBudget::default_for(new.request_id), 0)
        .unwrap();
    assert_eq!(engine.active_leases(), 0);
}

#[test]
fn moving_alias_recomputes_the_previous_physical_limit() {
    let mut engine = engine(&[]);
    engine
        .upsert(member("a").with_capacity("shared", 1))
        .unwrap();
    engine
        .upsert(member("b").with_capacity("shared", 2))
        .unwrap();
    engine
        .upsert(member("a").with_capacity("other", 1))
        .unwrap();
    assert!(engine.reserve(&request(1).with_owner("b"), 0).is_ok());
    assert!(engine.reserve(&request(2).with_owner("b"), 0).is_ok());
}

#[test]
fn begin_dispatch_rechecks_removed_disabled_and_replaced_routes() {
    for change in 0..3 {
        let mut engine = engine(&["a"]);
        let lease = engine.reserve(&request(1), 0).unwrap();
        match change {
            0 => {
                engine.set_enabled("a", false);
            }
            1 => {
                engine.remove("a");
            }
            _ => {
                engine
                    .upsert(member("a").with_operation(RotationOperation::Image))
                    .unwrap();
            }
        }
        let mut budget = RequestBudget::default_for(lease.request_id);
        assert_eq!(
            engine.begin_dispatch(lease.lease_id, &mut budget),
            Err(DispatchStartError::CandidateChanged)
        );
        assert_eq!(budget.dispatches(), 0);
        engine.cancel(lease.lease_id, &budget, 0).unwrap();
        assert_eq!(engine.active_leases(), 0);
    }
}

#[test]
fn terminal_success_requires_a_real_dispatch() {
    let mut engine = engine(&["a"]);
    let lease = engine.reserve(&request(1), 0).unwrap();
    let budget = RequestBudget::default_for(lease.request_id);
    assert_eq!(
        engine.settle(
            lease.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::accepted(),
                health: HealthObservation::Success,
            },
            &budget,
            0
        ),
        Err(SettlementError::NotDispatched)
    );
    assert_eq!(engine.active_leases(), 1);
    engine.cancel(lease.lease_id, &budget, 0).unwrap();
}

#[test]
fn begin_dispatch_rechecks_new_quota_rate_auth_and_circuit_blocks() {
    for change in 0..5 {
        let mut engine = engine(&["a"]);
        let lease = engine.reserve(&request(1), 0).unwrap();
        match change {
            0 => {
                engine.set_quota("a", QuotaState::Exhausted { reset_at_ms: None });
            }
            1 => {
                engine.set_rate(
                    "a",
                    RateState::Limited {
                        not_before_ms: 1_000,
                    },
                );
            }
            2 => {
                engine.sync_candidate_route_rate(
                    "a",
                    &lease.route_key,
                    RateState::Limited {
                        not_before_ms: 1_000,
                    },
                );
            }
            3 => {
                engine.set_auth("a", AuthState::Blocked);
            }
            _ => {
                fail(&mut engine, "a", 2, 1);
            }
        }
        let mut budget = RequestBudget::default_for(lease.request_id);
        assert_eq!(
            engine.begin_dispatch(lease.lease_id, &mut budget),
            Err(DispatchStartError::CandidateChanged)
        );
        assert_eq!(budget.dispatches(), 0);
        assert!(engine.release_unstarted(lease.lease_id));
        assert_eq!(engine.active_leases(), 0);
    }
}

#[test]
fn another_routes_rate_change_does_not_revoke_a_reserved_operation() {
    let mut engine = engine(&["a"]);
    let lease = engine.reserve(&request(1), 0).unwrap();
    engine.sync_candidate_route_rate(
        "a",
        "other-route",
        RateState::Limited {
            not_before_ms: 1_000,
        },
    );
    let mut budget = RequestBudget::default_for(lease.request_id);
    assert!(engine.begin_dispatch(lease.lease_id, &mut budget).is_ok());
    engine.cancel(lease.lease_id, &budget, 0).unwrap();
}

#[test]
fn all_mandatory_deadlines_use_maximum_and_alternatives_use_minimum() {
    let mut engine = engine(&["a", "b", "denied"]);
    engine.set_quota(
        "a",
        QuotaState::Exhausted {
            reset_at_ms: Some(100),
        },
    );
    engine.set_rate("a", RateState::Limited { not_before_ms: 300 });
    engine.set_rate("b", RateState::Limited { not_before_ms: 200 });
    engine.set_rate("denied", RateState::Limited { not_before_ms: 1 });
    let request = request(1).with_allowed_candidates(BTreeSet::from(["a".into(), "b".into()]));
    assert_eq!(
        engine.candidate_availability(&request, "a", 0),
        CandidateAvailability::WaitUntil {
            at_ms: 300,
            reason: CandidateBlockReason::RateLimited,
        }
    );
    assert_eq!(engine.next_wakeup(&request, 0), Some(200));
}

#[test]
fn upsert_and_equal_or_older_quota_revision_cannot_clear_provider_blocks() {
    let mut engine = engine(&["a"]);
    engine.set_auth("a", AuthState::Blocked);
    engine.set_quota_if_newer("a", QuotaState::Exhausted { reset_at_ms: None }, 42);
    engine.set_rate("a", RateState::Limited { not_before_ms: 800 });
    let mut changed = member("a");
    changed.weight = 10;
    engine.upsert(changed).unwrap();
    assert!(!engine.set_quota_if_newer("a", QuotaState::Available, 42));
    assert!(!engine.set_quota_if_newer("a", QuotaState::Available, 41));
    let candidate = engine.candidate("a").unwrap();
    assert_eq!(candidate.auth, AuthState::Blocked);
    assert_eq!(candidate.quota, QuotaState::Exhausted { reset_at_ms: None });
    assert_eq!(candidate.rate, RateState::Limited { not_before_ms: 800 });
}

#[test]
fn automatic_uses_normalized_load_and_in_order_skips_busy_members() {
    let mut engine = engine(&[]);
    engine.upsert(member("a").with_capacity("a", 1)).unwrap();
    engine.upsert(member("b").with_capacity("b", 4)).unwrap();
    engine.reserve(&request(1).with_owner("a"), 0).unwrap();
    engine.reserve(&request(2).with_owner("b"), 0).unwrap();
    assert_eq!(engine.select(&request(3), 0).unwrap().candidate_id, "b");
    engine.set_mode(RotationMode::InOrder);
    assert_eq!(engine.select(&request(3), 0).unwrap().candidate_id, "b");
    assert_eq!(
        engine.select(
            &request(3).with_allowed_candidates(BTreeSet::from(["a".into()])),
            0
        ),
        None
    );
}

#[test]
fn aliases_neither_double_weight_nor_accumulate_absent_debt() {
    let mut engine = engine(&["a", "b"]);
    engine.set_mode(RotationMode::RoundRobin);
    engine
        .upsert(member("a-alias").with_capacity("a", 0))
        .unwrap();
    let mut counts = BTreeMap::new();
    for id in 1..=120 {
        let lease = engine.reserve(&request(id), 0).unwrap();
        *counts.entry(lease.candidate_id.clone()).or_insert(0) += 1;
        engine
            .cancel(
                lease.lease_id,
                &RequestBudget::default_for(lease.request_id),
                0,
            )
            .unwrap();
    }
    assert_eq!(
        counts,
        BTreeMap::from([("a".to_owned(), 60), ("b".to_owned(), 60)])
    );
    assert_eq!(engine.rotation_credit.len(), 2);
    engine.set_enabled("b", false);
    let lease = engine.reserve(&request(121), 0).unwrap();
    assert_eq!(engine.rotation_credit.len(), 1);
    engine
        .cancel(
            lease.lease_id,
            &RequestBudget::default_for(lease.request_id),
            0,
        )
        .unwrap();
    engine.remove("a");
    engine.remove("a-alias");
    engine.remove("b");
    assert!(engine.rotation_credit.is_empty());
    assert!(engine.candidate_generations.is_empty());
}

#[test]
fn saturated_recovery_permit_does_not_hide_healthy_alternative() {
    let mut engine = engine(&["a", "b", "healthy"]);
    fail(&mut engine, "a", 1, 0);
    fail(&mut engine, "b", 2, 0);
    let trial = engine.reserve(&request(3), 1_000).unwrap();
    assert!(trial.recovery);
    assert_eq!(
        engine.select(&request(4), 1_000).unwrap().candidate_id,
        "healthy"
    );
    let ordinary = engine.reserve(&request(4), 1_000).unwrap();
    assert!(!ordinary.recovery);
}

#[test]
fn recovery_share_is_global_bounded_and_each_due_source_gets_a_turn() {
    let mut engine = engine(&["a", "b", "c", "healthy"]);
    engine.set_recovery_policy(RecoveryPolicy {
        initial_credits: 1,
        successful_requests_per_credit: 2,
        max_in_flight: 1,
    });
    for (index, member) in ["a", "b", "c"].into_iter().enumerate() {
        fail(&mut engine, member, index as u64 + 1, 0);
    }
    let mut trials = Vec::new();
    let mut completed = 0;
    for id in 10..19 {
        let lease = engine.reserve(&request(id), id * 1_000).unwrap();
        let health = if lease.recovery {
            trials.push(lease.candidate_id.clone());
            HealthObservation::CountableTransient {
                provider_not_before_ms: None,
            }
        } else {
            completed += 1;
            HealthObservation::Success
        };
        outcome(&mut engine, lease, health, id * 1_000);
        assert!(trials.len() <= 1 + completed / 2);
    }
    assert_eq!(&trials[..3], ["a", "b", "c"]);
}

#[test]
fn long_separated_failures_do_not_accumulate_forever() {
    let mut engine = engine(&["a"]);
    fail(&mut engine, "a", 1, 0);
    fail(&mut engine, "a", 2, FAILURE_WINDOW_MS + 1);
    assert_eq!(engine.circuit("a", "responses:text").failure_streak, 1);
}

#[test]
fn outcomes_from_a_closed_incident_cannot_join_a_later_incident() {
    let mut engine = engine(&["a"]);
    let old = engine.reserve(&request(1), 0).unwrap();
    let mut old_budget = RequestBudget::default_for(old.request_id);
    engine
        .begin_dispatch(old.lease_id, &mut old_budget)
        .unwrap();
    let success = engine.reserve(&request(2), 0).unwrap();
    outcome(&mut engine, success, HealthObservation::Success, 0);
    fail(&mut engine, "a", 3, 1);
    let before = engine.circuit("a", "responses:text");
    engine
        .settle(
            old.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::not_sent(),
                health: HealthObservation::CountableTransient {
                    provider_not_before_ms: None,
                },
            },
            &old_budget,
            2,
        )
        .unwrap();
    assert_eq!(engine.circuit("a", "responses:text"), before);
}
