use super::*;

#[test]
fn reassigned_member_rejects_late_outcomes_from_its_previous_capacity_owner() {
    for health in [
        HealthObservation::Success,
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
    ] {
        let member = candidate("a", 0, 1).with_capacity("old-member", 1);
        let mut engine = engine_with([member.clone()]);
        engine.recovery_credits = 0;
        engine.successful_requests_since_recovery =
            engine.recovery_policy.successful_requests_per_credit - 1;
        let old_lease = engine.reserve(&request(1), 100).unwrap();
        let budget = dispatch(&mut engine, &old_lease);
        engine
            .upsert(member.with_capacity("new-member", 1))
            .unwrap();
        let current_lease = engine.reserve(&request(2), 100).unwrap();
        let settlement = engine
            .settle_with_latency(
                old_lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::accepted(),
                    health,
                },
                &budget,
                200,
                Some(500),
                true,
            )
            .unwrap();
        assert!(!settlement.observation_current);
        assert_eq!(engine.capacity_in_flight("old-member"), 0);
        assert_eq!(engine.capacity_in_flight("new-member"), 1);
        assert_eq!(engine.in_flight("a"), 1);
        assert_eq!(engine.circuit("a", "responses:gpt").failure_streak, 0);
        assert!(
            !engine
                .health
                .contains_key(&("a".into(), "responses:gpt".into()))
        );
        assert_eq!(engine.recovery_credits, 0);
        assert!(engine.release_unstarted(current_lease.lease_id));
        assert_eq!(engine.active_leases(), 0);
    }
}

#[test]
fn stale_or_future_latency_cannot_bias_a_fresh_measurement_window() {
    let previous_at = HEALTH_HALF_LIFE_MS * 2;
    for now_ms in [previous_at + HEALTH_HALF_LIFE_MS + 1, previous_at - 1] {
        let mut engine = engine_with([candidate("warm", 0, 1), candidate("other", 0, 1)]);
        record_ready_latency(&mut engine, "warm", 60_000, previous_at);
        record_ready_latency(&mut engine, "warm", 100, now_ms);
        record_ready_latency(&mut engine, "other", 100, now_ms);
        let diagnostics = engine.latency_diagnostics("warm", "responses:gpt", now_ms);
        assert_eq!(diagnostics.state, RouteLatencyState::Ready);
        assert_eq!(diagnostics.p95_ms, Some(100));
        assert_eq!(
            engine
                .select(&preferred_request(1, "warm"), now_ms)
                .unwrap()
                .candidate_id,
            "warm",
            "only current latency evidence may displace a ready cache owner"
        );
    }
}

#[test]
fn superseded_authorization_releases_capacity_without_health_latency_or_recovery_credit() {
    for health in [
        HealthObservation::Success,
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
        HealthObservation::Cancelled,
        HealthObservation::Unknown,
    ] {
        let mut engine = engine_with([candidate("a", 0, 1)]);
        engine.recovery_credits = 0;
        engine.successful_requests_since_recovery = engine
            .recovery_policy
            .successful_requests_per_credit
            .saturating_sub(1);
        let lease = engine.reserve(&request(1), 0).unwrap();
        let budget = dispatch(&mut engine, &lease);
        let settlement = engine
            .settle_with_latency(
                lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::committed(),
                    health,
                },
                &budget,
                10,
                Some(100),
                false,
            )
            .unwrap();
        assert!(!settlement.observation_current);
        assert_eq!(engine.candidates["a"].in_flight, 0);
        assert_eq!(engine.circuit("a", "responses:gpt").failure_streak, 0);
        assert_eq!(
            engine
                .latency_diagnostics("a", "responses:gpt", 10)
                .sample_count,
            0
        );
        assert_eq!(engine.recovery_credits, 0);
    }
}

#[test]
fn latency_diagnostics_isolate_candidates_and_routes() {
    let mut a = candidate("a", 0, 1);
    a.add_route("image:gpt", "gpt-image", RotationOperation::Image);
    let mut engine = engine_with([a, candidate("b", 0, 1)]);
    for value in 1..=20 {
        engine.observe_first_output_latency("a", "responses:gpt", Some(value), 100);
    }
    engine.observe_first_output_latency("a", "responses:gpt", None, 100);
    let ready = engine.latency_diagnostics("a", "responses:gpt", 100);
    assert_eq!(ready.state, RouteLatencyState::Ready);
    assert_eq!((ready.sample_count, ready.unknown_count), (20, 1));
    for (candidate, route) in [("b", "responses:gpt"), ("a", "image:gpt")] {
        let missing = engine.latency_diagnostics(candidate, route, 100);
        assert_eq!(missing.state, RouteLatencyState::Unknown);
        assert_eq!(missing.sample_count, 0);
    }
    engine.remove("a").unwrap();
    engine.upsert(candidate("a", 0, 1)).unwrap();
    assert_eq!(
        engine
            .latency_diagnostics("a", "responses:gpt", 100)
            .sample_count,
        0
    );
}

#[test]
fn actual_dispatch_uses_the_current_incident_after_another_request_succeeds() {
    for retained in [false, true] {
        let mut engine = engine_with([candidate("a", 0, 1)]);
        let pending = engine.reserve(&request(1), 0).unwrap();
        let mut budget = RequestBudget::new(pending.request_id, 2).unwrap();
        if retained {
            engine
                .begin_dispatch(pending.lease_id, &mut budget)
                .unwrap();
        }
        let peer = engine.reserve(&request(2), 0).unwrap();
        let peer_budget = dispatch(&mut engine, &peer);
        engine
            .settle(
                peer.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::committed(),
                    health: HealthObservation::Success,
                },
                &peer_budget,
                10,
            )
            .unwrap();
        if retained {
            engine
                .begin_transport_dispatch(pending.lease_id, &mut budget)
                .unwrap();
        } else {
            engine
                .begin_dispatch(pending.lease_id, &mut budget)
                .unwrap();
        }
        engine
            .settle(
                pending.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::not_sent(),
                    health: HealthObservation::CountableTransient {
                        provider_not_before_ms: Some(500),
                    },
                },
                &budget,
                20,
            )
            .unwrap();
        let circuit = engine.circuit("a", "responses:gpt");
        assert_eq!(circuit.failure_streak, 1, "retained lease: {retained}");
        assert_eq!(circuit.state, CircuitState::Degraded);
        assert_eq!(circuit.not_before_ms, Some(500));
        assert_eq!(engine.active_leases(), 0);
    }
}

#[test]
fn concurrent_successes_all_contribute_latency_and_recovery_credit() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    engine.recovery_credits = 0;
    engine.recovery_policy.successful_requests_per_credit = 20;
    let attempts = (1..=20)
        .map(|id| {
            let lease = engine.reserve(&request(id), 0).unwrap();
            let budget = dispatch(&mut engine, &lease);
            (lease, budget)
        })
        .collect::<Vec<_>>();
    for (lease, budget) in attempts {
        engine
            .settle_with_latency(
                lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::committed(),
                    health: HealthObservation::Success,
                },
                &budget,
                100,
                Some(200),
                true,
            )
            .unwrap();
    }
    let latency = engine.latency_diagnostics("a", "responses:gpt", 100);
    assert_eq!(latency.sample_count, 20);
    assert_eq!(latency.state, RouteLatencyState::Ready);
    assert_eq!(engine.recovery_credits(), 1);
    assert_eq!(engine.active_leases(), 0);
}

#[test]
fn intervening_failure_and_recovery_fence_an_old_concurrent_success() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    let old_lease = engine.reserve(&request(1), 0).unwrap();
    let old_budget = dispatch(&mut engine, &old_lease);
    for (id, now, health) in [
        (2, 1, HealthObservation::Success),
        (
            3,
            2,
            HealthObservation::CountableTransient {
                provider_not_before_ms: None,
            },
        ),
        (4, 1_000, HealthObservation::Success),
    ] {
        let lease = engine.reserve(&request(id), now).unwrap();
        let budget = dispatch(&mut engine, &lease);
        engine
            .settle_with_latency(
                lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::committed(),
                    health,
                },
                &budget,
                now,
                Some(200),
                true,
            )
            .unwrap();
    }
    let before = engine.latency_diagnostics("a", "responses:gpt", 1_000);
    assert_eq!(before.sample_count, 2);
    let circuit = engine.circuit("a", "responses:gpt");
    let successes = engine.successful_requests_since_recovery;
    engine
        .settle_with_latency(
            old_lease.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::committed(),
                health: HealthObservation::Success,
            },
            &old_budget,
            1_001,
            Some(1),
            true,
        )
        .unwrap();
    assert_eq!(engine.circuit("a", "responses:gpt"), circuit);
    assert_eq!(
        engine
            .latency_diagnostics("a", "responses:gpt", 1_001)
            .sample_count,
        before.sample_count
    );
    assert_eq!(engine.successful_requests_since_recovery, successes);
    assert_eq!(engine.active_leases(), 0);
}

#[test]
fn successful_unknown_ttft_is_counted_once_and_keeps_selection_ewma() {
    let mut engine = engine_with([candidate("a", 0, 1)]);
    engine.observe_first_output_latency("a", "responses:gpt", Some(100), 0);
    let mut budget = RequestBudget::new(RequestId(1), 1).unwrap();
    let lease = engine.reserve(&request(1), 10).unwrap();
    engine.begin_dispatch(lease.lease_id, &mut budget).unwrap();
    let success = AttemptObservation {
        execution: ExecutionObservation::accepted(),
        health: HealthObservation::Success,
    };
    engine
        .settle_with_latency(lease.lease_id, success, &budget, 20, None, true)
        .unwrap();
    assert!(
        engine
            .settle_with_latency(lease.lease_id, success, &budget, 20, None, true)
            .is_err()
    );
    let diagnostics = engine.latency_diagnostics("a", "responses:gpt", 20);
    assert_eq!(
        (diagnostics.sample_count, diagnostics.unknown_count),
        (1, 1)
    );
    let health = &engine.health[&("a".into(), "responses:gpt".into())];
    assert_eq!(health.first_output_latency_ms, Some(100));
    assert_eq!(health.latency_observed_at_ms, Some(0));
}

#[test]
fn route_replacement_clears_samples_and_fences_the_previous_route() {
    let mut member = candidate("a", 0, 1);
    let mut engine = engine_with([member.clone()]);
    engine.observe_first_output_latency("a", "responses:gpt", Some(100), 0);
    engine.observe_health(
        "a",
        "responses:gpt",
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
        0,
    );
    let lease = engine.reserve(&request(1), 10).unwrap();
    let budget = dispatch(&mut engine, &lease);
    member.priority = 10;
    engine.upsert(member.clone()).unwrap();
    assert_eq!(
        engine.health[&("a".into(), "responses:gpt".into())].failure_score_milli,
        HEALTH_EWMA_ALPHA_MILLI
    );
    assert_eq!(
        engine
            .latency_diagnostics("a", "responses:gpt", 10)
            .sample_count,
        1
    );

    member.add_route("responses:gpt", "gpt-other", RotationOperation::Text);
    engine.upsert(member).unwrap();
    let settlement = engine
        .settle_with_latency(
            lease.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::accepted(),
                health: HealthObservation::Success,
            },
            &budget,
            20,
            Some(200),
            true,
        )
        .unwrap();
    assert!(!settlement.observation_current);
    let diagnostics = engine.latency_diagnostics("a", "responses:gpt", 20);
    assert_eq!(diagnostics.state, RouteLatencyState::Unknown);
    assert_eq!(
        (diagnostics.sample_count, diagnostics.unknown_count),
        (0, 0)
    );
    assert!(
        !engine
            .health
            .contains_key(&("a".into(), "responses:gpt".into()))
    );
    let replacement_request = RotationRequest::new(RequestId(2), "responses:gpt", "gpt-other");
    assert!(engine.select(&replacement_request, 20).is_some());
    engine.observe_first_output_latency("a", "responses:gpt", Some(200), 20);
    let health = &engine.health[&("a".into(), "responses:gpt".into())];
    assert_eq!(health.first_output_latency_ms, Some(200));
    assert_eq!(health.failure_score_milli, 0);
}

#[test]
fn unsuccessful_attempts_do_not_become_latency_samples() {
    for health in [
        HealthObservation::Cancelled,
        HealthObservation::Unknown,
        HealthObservation::LocalError,
        HealthObservation::ClientError,
        HealthObservation::CountableTransient {
            provider_not_before_ms: None,
        },
    ] {
        let mut engine = engine_with([candidate("a", 0, 1)]);
        let lease = engine.reserve(&request(1), 0).unwrap();
        let budget = dispatch(&mut engine, &lease);
        engine
            .settle_with_latency(
                lease.lease_id,
                AttemptObservation {
                    execution: ExecutionObservation::unknown(),
                    health,
                },
                &budget,
                10,
                Some(5),
                true,
            )
            .unwrap();
        let diagnostics = engine.latency_diagnostics("a", "responses:gpt", 10);
        assert_eq!(diagnostics.state, RouteLatencyState::Unknown);
        assert_eq!(
            (diagnostics.sample_count, diagnostics.unknown_count),
            (0, 0)
        );
    }
}
