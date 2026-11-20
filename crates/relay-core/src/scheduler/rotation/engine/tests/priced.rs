use super::*;

fn costs(a: u64, b: u64) -> BTreeMap<String, u64> {
    [("a".into(), a), ("b".into(), b)].into()
}

fn pick(
    engine: &mut RotationEngine,
    request: &RotationRequest,
    bound: &BTreeMap<String, u64>,
    now: u64,
) -> String {
    let lease = engine.reserve_priced(request, bound, now).unwrap();
    assert!(engine.release_unstarted(lease.lease_id));
    lease.candidate_id
}

#[test]
fn whole_request_price_follows_both_cost_bands_and_existing_tie_breaks() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 1)]);
    assert_eq!(pick(&mut engine, &request(1), &costs(2300, 1600), 1), "b");
    assert_eq!(pick(&mut engine, &request(2), &costs(3200, 6100), 2), "a");
    assert_eq!(
        pick(&mut engine, &preferred_request(3, "a"), &costs(2, 1), 3),
        "b"
    );
    assert_eq!(
        pick(&mut engine, &preferred_request(4, "a"), &costs(2, 2), 4),
        "a"
    );
}

#[test]
fn price_cannot_override_health_capacity_or_a_hard_owner() {
    let mut engine = engine_with([
        candidate("a", 0, 1),
        candidate("b", 1, 1).with_capacity("same-connection", 1),
        candidate("alias", 0, 1).with_capacity("same-connection", 1),
    ]);
    let prices = costs(2, 1);
    let scoped = request(1).with_allowed_candidates(["a".into()].into());
    assert_eq!(pick(&mut engine, &scoped, &prices, 10), "a");
    let pinned = request(2).with_owner("b");
    assert_eq!(pick(&mut engine, &pinned, &costs(1, 2), 10), "b");
    assert!(engine.sync_candidate_route_rate(
        "b",
        "responses:gpt",
        RateState::Limited { not_before_ms: 40 }
    ));
    assert_eq!(pick(&mut engine, &request(3), &prices, 20), "a");
    assert_eq!(
        engine.reserve_priced(&pinned, &prices, 20),
        Err(AdmissionError::NoEligibleCandidate)
    );

    let busy = engine.reserve(&request(4).with_owner("alias"), 40).unwrap();
    assert_eq!(pick(&mut engine, &request(5), &prices, 40), "a");
    assert!(engine.release_unstarted(busy.lease_id));
    assert_eq!(pick(&mut engine, &request(6), &prices, 41), "b");

    record_ready_latency(&mut engine, "a", 100, 50);
    record_ready_latency(&mut engine, "b", 1_000, 50);
    assert_eq!(pick(&mut engine, &request(7), &prices, 50), "a");
}

#[test]
fn explicit_modes_do_not_order_by_price() {
    let mut engine = engine_with([candidate("a", 1, 1), candidate("b", 0, 1)]);
    let prices = costs(2, 1);
    engine.set_mode(RotationMode::Failover);
    assert_eq!(pick(&mut engine, &request(1), &prices, 1), "a");
    engine.set_mode(RotationMode::RoundRobin);
    assert_eq!(pick(&mut engine, &request(2), &prices, 2), "a");
    assert_eq!(pick(&mut engine, &request(3), &prices, 3), "b");
}
