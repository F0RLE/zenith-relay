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

fn preferred_request(id: u64, candidate_id: &str) -> RotationRequest {
    let mut request = request(id);
    request.preferred = Some(candidate_id.to_owned());
    request
}

#[test]
fn balanced_quota_margin_preserves_affinity_without_hiding_a_material_lead() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 1)]);
    for (id, remaining) in [("a", 8_100), ("b", 8_000)] {
        assert!(engine.sync_candidate_state(
            id,
            true,
            false,
            AuthState::Ready,
            QuotaState::Available,
            RateState::Ready
        ));
        assert!(engine.set_quota_remaining(id, Some(remaining), Some(100)));
    }
    let selected = engine.select(&preferred_request(1, "b"), 100).unwrap();
    assert_eq!(selected.candidate_id, "b");
    assert_eq!(selected.reason, RotationSelectionReason::WeightedRotation);
    assert!(engine.set_quota_remaining("a", Some(9_000), Some(100)));
    assert_eq!(
        engine
            .select(&preferred_request(2, "b"), 100)
            .unwrap()
            .candidate_id,
        "a"
    );
}

#[test]
fn cache_owner_rebalance_uses_fresh_quota_margins_not_completion_order() {
    for (remaining, owner_remaining, expected) in [
        (Some(8_000), Some(8_100), false),
        (Some(8_100), Some(8_000), false),
        (Some(8_499), Some(8_000), false),
        (Some(8_500), Some(8_000), true),
        (Some(9_000), Some(8_000), true),
        (Some(9_000), None, true),
        (None, Some(8_000), false),
        (None, None, false),
    ] {
        let mut engine = engine_with([candidate("owner", 0, 1), candidate("next", 0, 1)]);
        for (id, quota) in [("owner", owner_remaining), ("next", remaining)] {
            assert!(engine.sync_candidate_state(
                id,
                true,
                false,
                AuthState::Ready,
                if quota.is_some() {
                    QuotaState::Available
                } else {
                    QuotaState::Unknown
                },
                RateState::Ready,
            ));
            assert!(engine.set_quota_remaining(id, quota, Some(100)));
        }
        assert_eq!(
            engine.has_material_resource_lead("next", "owner", 100),
            expected
        );
        assert!(!engine.has_material_resource_lead("next", "owner", 99));
        assert!(!engine.has_material_resource_lead(
            "next",
            "owner",
            101 + engine.quota_stale_after_ms
        ));
        engine.set_mode(RotationMode::RoundRobin);
        assert!(!engine.has_material_resource_lead("next", "owner", 100));
    }
}

#[test]
fn cache_owner_rebalance_uses_credit_margin_without_overriding_known_quota() {
    for (next_credits, owner_credits, expected) in [
        (Some(500_000_000), Some(499_000_000), false),
        (Some(500_000_000), Some(485_000_001), false),
        (Some(500_000_000), Some(485_000_000), true),
        (Some(500_000_000), None, true),
        (None, Some(500_000_000), false),
        (None, None, false),
    ] {
        let mut engine = engine_with([candidate("owner", 0, 1), candidate("next", 0, 1)]);
        for (id, credits) in [("owner", owner_credits), ("next", next_credits)] {
            assert!(engine.set_provider_credits(id, credits, false, Some(100)));
        }
        assert_eq!(
            engine.has_material_resource_lead("next", "owner", 100),
            expected
        );
        assert!(!engine.has_material_resource_lead("next", "owner", 99));
        assert!(!engine.has_material_resource_lead(
            "next",
            "owner",
            101 + engine.quota_stale_after_ms
        ));
        assert!(engine.sync_candidate_state(
            "owner",
            true,
            false,
            AuthState::Ready,
            QuotaState::Available,
            RateState::Ready,
        ));
        assert!(engine.set_quota_remaining("owner", Some(8_000), Some(100)));
        assert!(!engine.has_material_resource_lead("next", "owner", 100));
    }
}

#[test]
fn balanced_small_quota_differences_do_not_starve_equal_request_shares() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 1)]);
    for (id, remaining) in [("a", 8_100), ("b", 8_000)] {
        assert!(engine.sync_candidate_state(
            id,
            true,
            false,
            AuthState::Ready,
            QuotaState::Available,
            RateState::Ready
        ));
        assert!(engine.set_quota_remaining(id, Some(remaining), Some(100)));
    }
    let mut counts = BTreeMap::new();
    for id in 1..=20 {
        let lease = engine.reserve(&request(id), 100).unwrap();
        *counts.entry(lease.candidate_id.clone()).or_insert(0) += 1;
        assert!(engine.release_unstarted(lease.lease_id));
    }
    assert_eq!(counts.get("a"), Some(&10));
    assert_eq!(counts.get("b"), Some(&10));
}

fn dispatch(engine: &mut RotationEngine, lease: &RotationLease) -> RequestBudget {
    let mut budget = RequestBudget::default_for(lease.request_id);
    engine.begin_dispatch(lease.lease_id, &mut budget).unwrap();
    budget
}

fn settle_owner(
    engine: &mut RotationEngine,
    candidate_id: &str,
    request_id: u64,
    now_ms: u64,
    health: HealthObservation,
) {
    let request = request(request_id).with_owner(candidate_id);
    let lease = engine.reserve(&request, now_ms).unwrap();
    let budget = dispatch(engine, &lease);
    engine
        .settle(
            lease.lease_id,
            AttemptObservation {
                execution: if matches!(health, HealthObservation::Success) {
                    ExecutionObservation::accepted()
                } else {
                    ExecutionObservation::not_sent()
                },
                health,
            },
            &budget,
            now_ms,
        )
        .unwrap();
}

#[test]
fn automatic_selection_softly_prefers_the_candidate_with_fewer_recent_failures() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 1)]);
    settle_owner(
        &mut engine,
        "a",
        1,
        0,
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
    );

    assert_eq!(engine.select(&request(2), 0).unwrap().candidate_id, "b");
    // Due recovery trials retain their existing permit and priority.
    assert_eq!(
        engine
            .select(&request(3), FIRST_TRANSIENT_PACING_MS)
            .unwrap()
            .candidate_id,
        "a"
    );
}

#[test]
fn successful_traffic_reduces_the_soft_failure_score() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    settle_owner(
        &mut engine,
        "a",
        1,
        0,
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
    );
    let failed_score = engine
        .health
        .get(&("a".to_owned(), "responses:gpt".to_owned()))
        .unwrap()
        .failure_score_milli;

    settle_owner(
        &mut engine,
        "a",
        2,
        FIRST_TRANSIENT_PACING_MS,
        HealthObservation::Success,
    );
    let recovered_score = engine
        .health
        .get(&("a".to_owned(), "responses:gpt".to_owned()))
        .unwrap()
        .failure_score_milli;

    assert_eq!(failed_score, 250);
    assert_eq!(recovered_score, 187);
}

#[test]
fn soft_failure_score_decays_after_its_half_life() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    engine.observe_health(
        "a",
        "responses:gpt",
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
        0,
    );
    let health = engine
        .health
        .get(&("a".to_owned(), "responses:gpt".to_owned()))
        .copied()
        .unwrap();

    assert_eq!(health.failure_score_milli, 250);
    assert_eq!(
        engine.decayed_failure_score(&health, HEALTH_HALF_LIFE_MS),
        125
    );
}

#[test]
fn latency_is_only_used_as_a_soft_tie_breaker_in_automatic_mode() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 1)]);
    for (request_id, candidate_id, latency_ms) in [(1, "a", 100), (2, "b", 1_000)] {
        let lease = engine
            .reserve(&request(request_id).with_owner(candidate_id), 0)
            .unwrap();
        let budget = dispatch(&mut engine, &lease);
        engine
            .settle_with_latency(
                lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::committed(),
                    health: HealthObservation::Success,
                },
                &budget,
                0,
                Some(latency_ms),
            )
            .unwrap();
    }

    assert_eq!(engine.select(&request(3), 0).unwrap().candidate_id, "a");
}

#[test]
fn soft_health_uses_a_common_baseline_before_affinity_and_weights() {
    let mut engine = engine_with([
        candidate("a", 0, 1),
        candidate("b", 0, 1),
        candidate("c", 0, 100),
    ]);
    for (id, failure_score) in [("a", 0), ("b", 180), ("c", 360)] {
        engine.health.insert(
            (id.to_owned(), "responses:gpt".to_owned()),
            HealthRuntime {
                failure_score_milli: failure_score,
                observed_at_ms: Some(0),
                ..Default::default()
            },
        );
    }
    assert_eq!(
        engine
            .select(&preferred_request(1, "c"), 0)
            .unwrap()
            .candidate_id,
        "a"
    );
    // Near-equivalent feedback preserves the session's existing member.
    assert_eq!(
        engine
            .select(&preferred_request(2, "b"), 0)
            .unwrap()
            .candidate_id,
        "b"
    );
    let ready = engine.ready_candidates(&request(3), 0);
    let expected = engine.selection_group(&ready, &request(3), 0);
    let reversed = ready.into_iter().rev().collect::<Vec<_>>();
    let actual = engine.selection_group(&reversed, &request(3), 0);
    assert_eq!(
        expected
            .iter()
            .map(|candidate| &candidate.id)
            .collect::<BTreeSet<_>>(),
        actual
            .iter()
            .map(|candidate| &candidate.id)
            .collect::<BTreeSet<_>>()
    );
}

#[test]
fn missing_and_stale_latency_do_not_block_ordinary_rotation() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 1)]);
    engine.observe_first_output_latency("a", "responses:gpt", 100, 0);
    assert_eq!(
        engine
            .select(&preferred_request(1, "b"), 0)
            .unwrap()
            .candidate_id,
        "b"
    );
    engine.observe_first_output_latency("b", "responses:gpt", 1_000, 0);
    assert_eq!(
        engine
            .select(&preferred_request(2, "b"), 0)
            .unwrap()
            .candidate_id,
        "a"
    );
    assert_eq!(
        engine
            .select(&preferred_request(3, "b"), HEALTH_HALF_LIFE_MS + 1)
            .unwrap()
            .candidate_id,
        "b"
    );
}

#[test]
fn future_latency_observations_do_not_override_fresh_owner_preference() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 1)]);
    engine.observe_first_output_latency("a", "responses:gpt", 1, 101);
    engine.observe_first_output_latency("b", "responses:gpt", 1_000, 100);
    assert_eq!(
        engine
            .select(&preferred_request(1, "b"), 100)
            .unwrap()
            .candidate_id,
        "b"
    );
    assert_eq!(
        engine
            .select(&preferred_request(2, "b"), 101)
            .unwrap()
            .candidate_id,
        "a"
    );
}

#[test]
fn ignored_health_observations_do_not_mutate_the_failure_score() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    engine.observe_health(
        "a",
        "responses:gpt",
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
        0,
    );
    let before = engine.health[&("a".to_owned(), "responses:gpt".to_owned())];
    for observation in [
        HealthObservation::Cancelled,
        HealthObservation::Unknown,
        HealthObservation::LocalError,
        HealthObservation::MonitoringFailure,
    ] {
        engine.observe_health("a", "responses:gpt", observation, HEALTH_HALF_LIFE_MS * 2);
    }
    let after = engine.health[&("a".to_owned(), "responses:gpt".to_owned())];
    assert_eq!(after.failure_score_milli, before.failure_score_milli);
    assert_eq!(after.observed_at_ms, before.observed_at_ms);
}

#[test]
fn success_latency_uses_the_lease_route_and_does_not_update_sibling_operations() {
    let mut member = candidate("a", 0, 1);
    member.add_route("image:gpt", "gpt-6", RotationOperation::Image);
    let mut engine = engine_with([member]);
    let lease = engine
        .reserve(
            &RotationRequest::new(RequestId(1), "image:gpt", "gpt-6")
                .with_operation(RotationOperation::Image),
            0,
        )
        .unwrap();
    let budget = dispatch(&mut engine, &lease);
    let observation = AttemptObservation {
        execution: ExecutionObservation::committed(),
        health: HealthObservation::Success,
    };
    engine
        .settle_with_latency(lease.lease_id, observation, &budget, 10, Some(100))
        .unwrap();

    assert_eq!(
        engine.health[&("a".to_owned(), "image:gpt".to_owned())].first_output_latency_ms,
        Some(100)
    );
    assert!(!engine
        .health
        .contains_key(&("a".to_owned(), "responses:gpt".to_owned())));
    assert_eq!(
        engine.settle_with_latency(lease.lease_id, observation, &budget, 20, Some(500)),
        Err(SettlementError::UnknownLease)
    );
    assert_eq!(
        engine.health[&("a".to_owned(), "image:gpt".to_owned())].first_output_latency_ms,
        Some(100)
    );
}

#[test]
fn late_success_latency_cannot_update_a_readded_candidate() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    let lease = engine.reserve(&request(1), 0).unwrap();
    let budget = dispatch(&mut engine, &lease);
    engine.remove("a").unwrap();
    engine.upsert(candidate("a", 0, 1)).unwrap();
    let settlement = engine
        .settle_with_latency(
            lease.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::committed(),
                health: HealthObservation::Success,
            },
            &budget,
            10,
            Some(100),
        )
        .unwrap();

    assert!(!settlement.observation_current);
    assert!(engine.health.is_empty());
}

#[test]
fn round_robin_and_failover_ignore_soft_health_signals() {
    for mode in [RotationMode::RoundRobin, RotationMode::Failover] {
        let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 1)]);
        engine.set_mode(mode);
        let before = engine.select(&request(1), 0).unwrap().candidate_id;
        settle_owner(
            &mut engine,
            &before,
            2,
            0,
            HealthObservation::CountableTransient {
                provider_not_before_ms: None,
            },
        );

        assert_eq!(
            engine
                .select(&request(3), FIRST_TRANSIENT_PACING_MS)
                .unwrap()
                .candidate_id,
            before
        );
    }
}

#[test]
fn removed_candidates_do_not_transfer_soft_health_to_readded_ids() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    engine.observe_health(
        "a",
        "responses:gpt",
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
        0,
    );
    assert!(engine.remove("a").is_some());
    engine.upsert(candidate("a", 0, 1)).unwrap();

    assert!(!engine
        .health
        .contains_key(&("a".to_owned(), "responses:gpt".to_owned())));
}

#[test]
fn allowed_healthy_reserve_is_not_blocked_by_primary_priority() {
    let mut primary = candidate("primary", 100, 1);
    primary.auth = AuthState::Ready;
    let reserve = candidate("reserve", 0, 1);
    let mut engine = engine_with([primary, reserve]);
    engine.set_mode(RotationMode::Failover);
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
fn failover_does_not_send_healthy_primary_traffic_to_a_recovering_reserve() {
    let mut engine = engine_with([candidate("primary", 100, 1), candidate("reserve", 0, 1)]);
    engine.set_mode(RotationMode::Failover);
    settle_owner(
        &mut engine,
        "reserve",
        1,
        0,
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
    );
    let selected = engine
        .select(&request(2), FIRST_TRANSIENT_PACING_MS)
        .unwrap();
    assert_eq!(selected.candidate_id, "primary");
    assert!(!selected.recovery);
}

#[test]
fn weighted_preview_is_side_effect_free_and_reservation_advances_state() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 3)]);
    engine.set_mode(RotationMode::WeightedRoundRobin);
    let first_request = request(1);
    let preview = engine.select(&first_request, 0).unwrap();
    let second_preview = engine.select(&request(2), 0).unwrap();
    assert_eq!(preview.candidate_id, second_preview.candidate_id);
    let lease = engine.reserve(&first_request, 0).unwrap();
    assert_eq!(lease.candidate_id, preview.candidate_id);
    let different_candidate = engine.select(&request(3), 0).unwrap();
    assert_ne!(different_candidate.candidate_id, lease.candidate_id);
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
    assert_eq!(
        engine.health[&("a".to_owned(), "responses:gpt".to_owned())].failure_score_milli,
        250
    );
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
    let previous_lease = engine.reserve(&first, 0).unwrap();
    let newer_lease = engine.reserve(&second, 0).unwrap();
    let previous_budget = dispatch(&mut engine, &previous_lease);
    let newer_budget = dispatch(&mut engine, &newer_lease);
    engine
        .settle(
            newer_lease.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::not_sent(),
                health: HealthObservation::CountableTransient {
                    provider_not_before_ms: None,
                },
            },
            &newer_budget,
            1,
        )
        .unwrap();
    let before = engine.circuit("a", "responses:gpt");
    let health_before = engine.health[&("a".to_owned(), "responses:gpt".to_owned())];
    engine
        .settle_with_latency(
            previous_lease.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::accepted(),
                health: HealthObservation::Success,
            },
            &previous_budget,
            2,
            Some(100),
        )
        .unwrap();
    assert_eq!(engine.circuit("a", "responses:gpt"), before);
    let health_after = engine.health[&("a".to_owned(), "responses:gpt".to_owned())];
    assert_eq!(
        health_after.failure_score_milli,
        health_before.failure_score_milli
    );
    assert_eq!(health_after.observed_at_ms, health_before.observed_at_ms);
    assert_eq!(health_after.first_output_latency_ms, None);
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
    let previous_lease = engine.reserve(&request(1), 0).unwrap();
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
        .cancel(
            previous_lease.lease_id,
            &RequestBudget::default_for(RequestId(1)),
            0,
        )
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
