use super::*;

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
        .settle_with_latency(lease.lease_id, success, &budget, 20, None)
        .unwrap();
    assert!(
        engine
            .settle_with_latency(lease.lease_id, success, &budget, 20, None)
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
    let lease = engine.reserve(&request(1), 10).unwrap();
    let budget = dispatch(&mut engine, &lease);
    member.priority = 10;
    engine.upsert(member.clone()).unwrap();
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
        )
        .unwrap();
    assert!(!settlement.observation_current);
    let diagnostics = engine.latency_diagnostics("a", "responses:gpt", 20);
    assert_eq!(diagnostics.state, RouteLatencyState::Unknown);
    assert_eq!(
        (diagnostics.sample_count, diagnostics.unknown_count),
        (0, 0)
    );
    // Diagnostic invalidation leaves the existing selection policy untouched.
    assert_eq!(
        engine.health[&("a".into(), "responses:gpt".into())].first_output_latency_ms,
        Some(100)
    );
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
