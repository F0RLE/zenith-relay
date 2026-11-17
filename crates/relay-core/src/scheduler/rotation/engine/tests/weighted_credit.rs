use super::*;

#[test]
fn unrelated_route_edits_do_not_restart_weighted_rotation() {
    for edit in 0..4 {
        let mut unrelated = if edit == 3 {
            candidate("a", 0, 1)
        } else {
            RotationCandidate::new("other", "other-route", "other-model")
        };
        let mut engine = engine_with([
            candidate("a", 0, 1),
            candidate("b", 0, 3),
            unrelated.clone(),
        ]);
        engine.set_mode(RotationMode::WeightedRoundRobin);
        let mut selected_a = 0;
        for id in 1..=40 {
            match edit {
                0 => unrelated.weight = 1 + (id % 2) as u32,
                1 | 3 => unrelated.add_route(
                    "other-route",
                    if id % 2 == 0 {
                        "other-model"
                    } else {
                        "replacement-model"
                    },
                    RotationOperation::Text,
                ),
                _ => {
                    engine.remove("other").unwrap();
                }
            }
            engine.upsert(unrelated.clone()).unwrap();
            selected_a += usize::from(reserve_and_release(&mut engine, &request(id)) == "a");
        }
        assert_eq!(
            selected_a, 10,
            "unrelated edit {edit} must preserve the 1:3 request shares"
        );
    }
}

fn reserve_and_release(engine: &mut RotationEngine, request: &RotationRequest) -> String {
    let lease = engine.reserve(request, 100).unwrap();
    assert!(engine.release_unstarted(lease.lease_id));
    lease.candidate_id
}

#[test]
fn sticky_traffic_keeps_affinity_without_accumulating_unbounded_rotation_debt() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 1)]);
    for id in 1..=10_000 {
        assert_eq!(
            reserve_and_release(&mut engine, &preferred_request(id, "a")),
            "a"
        );
    }
    assert_eq!(engine.rotation_credit.len(), 2);
    assert!(
        engine
            .rotation_credit
            .values()
            .all(|credit| credit.abs() <= 64)
    );
    let first_return =
        (10_001..=10_066).position(|id| reserve_and_release(&mut engine, &request(id)) == "a");
    assert!(
        first_return.is_some(),
        "one busy chat must not starve later independent work"
    );
    assert_eq!(engine.active_leases(), 0);
}

#[test]
fn weighted_selection_clamps_old_credit_to_the_current_pool_weight() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 3)]);
    engine.set_mode(RotationMode::WeightedRoundRobin);
    engine
        .rotation_credit
        .insert(("responses:gpt".into(), "a".into()), i64::MAX);
    engine
        .rotation_credit
        .insert(("responses:gpt".into(), "b".into()), i64::MAX / 2);
    assert_eq!(engine.select(&request(1), 100).unwrap().candidate_id, "b");
    assert_eq!(reserve_and_release(&mut engine, &request(1)), "b");
    assert!(
        engine
            .rotation_credit
            .values()
            .all(|credit| credit.abs() <= 128)
    );
}

#[test]
fn bounded_credit_preserves_normal_weights_and_physical_member_votes() {
    for aliases in [false, true] {
        let mut members = vec![candidate("a", 0, 1), candidate("b", 0, 3)];
        if aliases {
            members.push(candidate("b-alias", 0, 3).with_capacity("b", 1));
        }
        let mut engine = engine_with(members);
        engine.set_mode(RotationMode::WeightedRoundRobin);
        let count = (1..=400)
            .filter(|id| reserve_and_release(&mut engine, &request(*id)) == "b")
            .count();
        assert_eq!(count, 300);
        assert_eq!(engine.rotation_credit.len(), 2);
    }
}

#[test]
fn rotation_credit_does_not_override_round_robin_or_hard_ownership() {
    let mut engine = engine_with([candidate("a", 0, 1), candidate("b", 0, 3)]);
    engine.set_mode(RotationMode::RoundRobin);
    for id in 1..=8 {
        assert_eq!(
            reserve_and_release(&mut engine, &request(id)),
            if id % 2 == 1 { "a" } else { "b" }
        );
    }
    assert!(engine.rotation_credit.is_empty());
    engine.set_mode(RotationMode::Automatic);
    assert_eq!(
        reserve_and_release(&mut engine, &request(9).with_owner("b")),
        "b"
    );
    engine.set_auth("b", AuthState::NeedsRefresh);
    assert!(engine.select(&request(10).with_owner("b"), 100).is_none());
}

#[test]
fn round_robin_tracks_the_physical_member_when_its_route_alias_changes() {
    let mut engine = engine_with([
        candidate("a-route", 0, 1).with_capacity("z-member", 1),
        candidate("z-route", 0, 1).with_capacity("z-member", 1),
        candidate("b-route", 0, 1).with_capacity("b-member", 1),
    ]);
    engine.set_mode(RotationMode::RoundRobin);
    let mut request = request(1);
    request.allowed_candidates = Some(BTreeSet::from(["b-route".into(), "z-route".into()]));
    assert_eq!(reserve_and_release(&mut engine, &request), "b-route");
    request.request_id = RequestId(2);
    assert_eq!(reserve_and_release(&mut engine, &request), "z-route");
    request.request_id = RequestId(3);
    request.allowed_candidates = Some(BTreeSet::from(["a-route".into(), "b-route".into()]));
    assert_eq!(reserve_and_release(&mut engine, &request), "b-route");
    request.request_id = RequestId(4);
    assert_eq!(reserve_and_release(&mut engine, &request), "a-route");
    request.request_id = RequestId(5);
    request.allowed_candidates = Some(BTreeSet::from(["b-route".into(), "z-route".into()]));
    assert_eq!(reserve_and_release(&mut engine, &request), "b-route");
}
