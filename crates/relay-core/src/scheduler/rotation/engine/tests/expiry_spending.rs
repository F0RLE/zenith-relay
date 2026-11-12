use super::*;
use crate::quota::QuotaSpendDeadline;

fn spending_candidate(id: &str, remaining: u64, expires_in_ms: Option<u64>) -> RotationCandidate {
    let mut member = candidate(id, 0, 1);
    member.quota = QuotaState::Available;
    member.quota_remaining_basis_points = Some(remaining);
    member.quota_observed_at_ms = Some(100);
    member.spend_deadline = expires_in_ms.map(|remaining_ms| QuotaSpendDeadline {
        expires_at_ms: 100 + remaining_ms,
        observed_at_ms: 100,
        valid_until_ms: 1_000,
    });
    member
}

fn complete(engine: &mut RotationEngine, id: u64) -> String {
    let request = request(id);
    let lease = engine.reserve(&request, 100).unwrap();
    let mut budget = RequestBudget::default_for(request.request_id);
    engine.begin_dispatch(lease.lease_id, &mut budget).unwrap();
    engine
        .settle(
            lease.lease_id,
            AttemptObservation {
                execution: ExecutionObservation::committed(),
                health: HealthObservation::Success,
            },
            &budget,
            100,
        )
        .unwrap();
    lease.candidate_id
}

#[test]
fn expiring_allowance_receives_more_independent_work_without_starving_other_members() {
    let day = 24 * 60 * 60 * 1_000;
    let mut engine = engine_with([
        spending_candidate("expiring", 6_000, Some(day)),
        spending_candidate("ordinary", 10_000, None),
    ]);
    assert_eq!(
        engine.select(&request(1), 100).unwrap().reason,
        RotationSelectionReason::WeightedRotation
    );
    let served = (1..=60)
        .filter(|id| complete(&mut engine, *id) == "expiring")
        .count();
    assert_eq!(
        served, 50,
        "bounded five-to-one weighting must use the ordinary rotation credits"
    );
    assert_eq!(engine.active_leases(), 0);
}

#[test]
fn expiry_bias_preserves_primary_warm_alternatives_and_hard_owners() {
    let engine = engine_with([
        spending_candidate("expiring", 10_000, Some(60_000)),
        spending_candidate("ordinary", 10_000, None),
    ]);
    assert_eq!(
        engine
            .select(&preferred_request(1, "ordinary"), 100)
            .unwrap()
            .candidate_id,
        "ordinary"
    );
    assert_eq!(
        engine
            .select(&request(2).with_owner("ordinary"), 100)
            .unwrap()
            .candidate_id,
        "ordinary"
    );
    let mut warm = request(3);
    warm.warm_fallbacks = vec!["ordinary".into()];
    assert_eq!(engine.select(&warm, 100).unwrap().candidate_id, "ordinary");
    assert!(!engine.has_material_resource_lead("expiring", "ordinary", 100));
}

#[test]
fn expiry_bias_never_overrides_load_capacity_quota_auth_or_provider_waits() {
    for blocked in 0..7 {
        let mut expiring = spending_candidate("expiring", 6_000, Some(60_000));
        match blocked {
            0 => expiring.enabled = false,
            1 => expiring.draining = true,
            2 => expiring.auth = AuthState::Blocked,
            3 => {
                expiring.quota = QuotaState::Exhausted {
                    reset_at_ms: Some(5_000),
                }
            }
            4 => {
                expiring.quota = QuotaState::CreditFallback {
                    reset_at_ms: Some(5_000),
                }
            }
            5 => {
                expiring.rate = RateState::Limited {
                    not_before_ms: 5_000,
                }
            }
            _ => expiring.max_concurrency = 1,
        }
        let mut engine = engine_with([expiring, spending_candidate("ordinary", 10_000, None)]);
        let busy = (blocked == 6).then(|| {
            engine
                .reserve(&request(1).with_owner("expiring"), 100)
                .unwrap()
        });
        assert_eq!(
            engine.select(&request(2), 100).unwrap().candidate_id,
            "ordinary",
            "gate {blocked}"
        );
        if let Some(busy) = busy {
            assert!(engine.release_unstarted(busy.lease_id));
        }
    }
    let mut engine = engine_with([
        spending_candidate("expiring", 10_000, Some(60_000)),
        spending_candidate("ordinary", 10_000, None),
    ]);
    let busy = engine
        .reserve(&request(1).with_owner("expiring"), 100)
        .unwrap();
    assert_eq!(
        engine.select(&request(2), 100).unwrap().candidate_id,
        "ordinary"
    );
    assert!(engine.release_unstarted(busy.lease_id));
}

#[test]
fn stale_missing_expired_and_distant_deadlines_keep_ordinary_quota_ranking() {
    for deadline in [
        None,
        Some(QuotaSpendDeadline {
            expires_at_ms: 5_000,
            observed_at_ms: 101,
            valid_until_ms: 1_000,
        }),
        Some(QuotaSpendDeadline {
            expires_at_ms: 5_000,
            observed_at_ms: 50,
            valid_until_ms: 99,
        }),
        Some(QuotaSpendDeadline {
            expires_at_ms: 100,
            observed_at_ms: 50,
            valid_until_ms: 1_000,
        }),
        Some(QuotaSpendDeadline {
            expires_at_ms: u64::MAX,
            observed_at_ms: 50,
            valid_until_ms: 1_000,
        }),
    ] {
        let mut expiring = spending_candidate("expiring", 6_000, None);
        expiring.spend_deadline = deadline;
        let engine = engine_with([expiring, spending_candidate("ordinary", 10_000, None)]);
        assert_eq!(
            engine.select(&request(1), 100).unwrap().candidate_id,
            "ordinary"
        );
    }
}

#[test]
fn expiry_requires_fresh_positive_quota_and_leaves_manual_modes_unchanged() {
    for observed in [None, Some(101), Some(1)] {
        let mut expiring = spending_candidate("expiring", 6_000, Some(60_000));
        expiring.quota_observed_at_ms = observed;
        let mut engine = engine_with([expiring, spending_candidate("ordinary", 10_000, None)]);
        engine.set_quota_stale_after_ms(50);
        assert_eq!(
            engine.select(&request(1), 100).unwrap().candidate_id,
            "ordinary"
        );
    }
    for mode in [
        RotationMode::RoundRobin,
        RotationMode::WeightedRoundRobin,
        RotationMode::Failover,
    ] {
        let mut engine = engine_with([
            spending_candidate("a-ordinary", 10_000, None),
            spending_candidate("z-expiring", 10_000, Some(60_000)),
        ]);
        engine.set_mode(mode);
        assert_eq!(
            engine.select(&request(1), 100).unwrap().candidate_id,
            "a-ordinary"
        );
    }
}

#[test]
fn replacing_or_removing_deadline_evidence_immediately_removes_the_soft_bias() {
    let mut engine = engine_with([
        spending_candidate("expiring", 6_000, Some(60_000)),
        spending_candidate("ordinary", 10_000, None),
    ]);
    assert_eq!(
        engine.select(&request(1), 100).unwrap().candidate_id,
        "expiring"
    );
    engine
        .upsert(spending_candidate("expiring", 6_000, None))
        .unwrap();
    assert_eq!(
        engine.select(&request(2), 100).unwrap().candidate_id,
        "ordinary"
    );
}
