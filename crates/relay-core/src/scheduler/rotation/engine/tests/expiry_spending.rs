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
        reset_credits: None,
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
fn subscription_expiry_orders_exact_deadlines_within_an_automatic_band() {
    let day = 24 * 60 * 60 * 1_000;
    let mut engine = engine_with([
        spending_candidate("later", 10_000, Some(2 * day)),
        spending_candidate("earlier", 6_000, Some(day)),
    ]);
    assert_eq!(
        engine.expiry_priority_key("earlier", 100),
        engine.expiry_priority_key("later", 100)
    );
    engine.set_mode(RotationMode::SubscriptionExpiry);
    assert!(engine.expiry_priority_key("earlier", 100) < engine.expiry_priority_key("later", 100));
    for id in 1..=40 {
        assert_eq!(complete(&mut engine, id), "earlier");
    }
    assert_eq!(engine.active_leases(), 0);
}

#[test]
fn subscription_expiry_requires_fresh_dates_and_available_quota() {
    let mut stale = spending_candidate("stale", 900, Some(1_000));
    stale.spend_deadline.as_mut().unwrap().valid_until_ms = 99;
    let mut exhausted = spending_candidate("exhausted", 0, Some(500));
    exhausted.quota = QuotaState::Exhausted { reset_at_ms: None };
    let mut engine = engine_with([
        stale,
        exhausted,
        spending_candidate("eligible", 7_000, Some(60_000)),
        spending_candidate("undated", 10_000, None),
    ]);
    engine.set_mode(RotationMode::SubscriptionExpiry);
    assert_eq!(complete(&mut engine, 1), "eligible");
    assert_eq!(engine.expiry_priority_key("stale", 100), None);
    assert_eq!(engine.expiry_priority_key("exhausted", 100), None);
}

#[test]
fn subscription_expiry_preserves_warm_and_hard_owners() {
    let mut engine = engine_with([
        spending_candidate("expiring", 10_000, Some(60_000)),
        spending_candidate("warm", 4_600, Some(600_000)),
    ]);
    engine.set_mode(RotationMode::SubscriptionExpiry);
    assert_eq!(
        engine
            .select(&preferred_request(1, "warm"), 100)
            .unwrap()
            .candidate_id,
        "warm"
    );
    assert_eq!(
        engine
            .select(&request(2).with_owner("warm"), 100)
            .unwrap()
            .candidate_id,
        "warm"
    );
    assert!(!engine.has_material_resource_lead("expiring", "warm", 100));
    let diagnostics = engine
        .cache_affinity_diagnostics(&preferred_request(3, "warm"), Some("warm"), "warm", 100)
        .unwrap();
    assert_eq!(diagnostics.quota_low_water_basis_points, Some(1_000));
}

#[test]
fn subscription_expiry_keeps_low_quota_rebalancing_and_undated_fallback() {
    let mut engine = engine_with([
        spending_candidate("expiring", 1_000, Some(60_000)),
        spending_candidate("alternative", 5_000, None),
    ]);
    engine.set_mode(RotationMode::SubscriptionExpiry);
    assert_eq!(
        engine
            .select(&preferred_request(1, "expiring"), 100)
            .unwrap()
            .candidate_id,
        "alternative"
    );
    assert!(engine.has_material_resource_lead("alternative", "expiring", 100));
    let mut fallback = engine_with([
        spending_candidate("lower", 2_000, None),
        spending_candidate("higher", 8_000, None),
    ]);
    fallback.set_mode(RotationMode::SubscriptionExpiry);
    assert_eq!(complete(&mut fallback, 2), "higher");
}

#[test]
fn expiry_replacement_without_a_material_quota_lead_keeps_the_warm_owner() {
    let day = 86_400_000;
    for mode in [RotationMode::Automatic, RotationMode::SubscriptionExpiry] {
        let mut engine = engine_with([
            spending_candidate("warm", 1_000, Some(day / 2)),
            spending_candidate("lower", 900, Some(day)),
            spending_candidate("slightly-higher", 1_300, Some(day)),
            spending_candidate("distant-best", 5_300, Some(12 * day)),
        ]);
        engine.set_mode(mode);
        for id in 1..=40 {
            let request = preferred_request(id, "warm");
            let selected = engine.select(&request, 100).unwrap();
            assert_eq!(selected.candidate_id, "warm", "mode {mode:?}");
            assert_ne!(selected.reason, RotationSelectionReason::SubscriptionExpiry);
            let trace = engine.decision_trace(&request, 100);
            assert_eq!(trace.selected_candidate_id.as_deref(), Some("warm"));
            assert_eq!(
                trace
                    .candidates
                    .iter()
                    .find(|candidate| candidate.candidate_id == "warm")
                    .unwrap()
                    .eliminated_at,
                None
            );
            assert_eq!(
                engine
                    .cache_affinity_diagnostics(&request, Some("warm"), "warm", 100)
                    .unwrap()
                    .reason,
                CacheAffinityReason::Retained
            );
            let lease = engine.reserve(&request, 100).unwrap();
            assert_eq!(lease.candidate_id, "warm");
            assert!(engine.release_unstarted(lease.lease_id));
        }
    }
}

#[test]
fn expiry_replacement_requires_each_selected_member_to_offer_the_quota_margin() {
    let day = 86_400_000;
    for mode in [RotationMode::Automatic, RotationMode::SubscriptionExpiry] {
        let mut weak = spending_candidate("weak", 1_600, Some(day));
        weak.weight = 100;
        let mut engine = engine_with([
            spending_candidate("warm", 1_000, Some(day / 2)),
            spending_candidate("lower", 900, Some(day)),
            spending_candidate("better", 2_000, Some(day)),
            weak,
            spending_candidate("distant-best", 5_300, Some(12 * day)),
        ]);
        engine.set_mode(mode);
        for id in 1..=40 {
            let request = preferred_request(id, "warm");
            let selected = engine.select(&request, 100).unwrap();
            assert_eq!(selected.candidate_id, "better", "mode {mode:?}");
            assert_eq!(selected.reason, RotationSelectionReason::SubscriptionExpiry);
            assert!(engine.has_material_resource_lead(&selected.candidate_id, "warm", 100));
            let diagnostics = engine
                .cache_affinity_diagnostics(&request, Some("warm"), "better", 100)
                .unwrap();
            assert_eq!(diagnostics.reason, CacheAffinityReason::QuotaHeadroom);
            assert_eq!(diagnostics.best_quota_remaining_basis_points, Some(2_000));
            let trace = engine.decision_trace(&request, 100);
            assert_eq!(
                trace
                    .candidates
                    .iter()
                    .find(|candidate| candidate.candidate_id == "warm")
                    .unwrap()
                    .eliminated_at,
                Some(RotationDecisionStageKind::ResourceHeadroom)
            );
            let lease = engine.reserve(&request, 100).unwrap();
            assert_eq!(lease.candidate_id, "better");
            assert!(engine.release_unstarted(lease.lease_id));
        }
    }
}

#[test]
fn filtered_warm_routes_do_not_suppress_expiry_priority_for_their_replacement() {
    let day = 24 * 60 * 60 * 1_000;
    for mode in [RotationMode::Automatic, RotationMode::SubscriptionExpiry] {
        let mut engine = engine_with([
            spending_candidate("warm", 500, Some(day / 2)),
            spending_candidate("expiring", 3_000, Some(day)),
            spending_candidate("later", 9_000, Some(12 * day)),
        ]);
        engine.set_mode(mode);
        for alternative in [false, true] {
            let mut request = request(1);
            if alternative {
                request.warm_fallbacks = vec!["warm".into()];
            } else {
                request.preferred = Some("warm".into());
            }
            let selected = engine.select(&request, 100).unwrap();
            assert_eq!(selected.candidate_id, "expiring");
            assert_eq!(selected.reason, RotationSelectionReason::SubscriptionExpiry);
            let trace = engine.decision_trace(&request, 100);
            assert_eq!(trace.selected_candidate_id.as_deref(), Some("expiring"));
            assert_eq!(
                trace
                    .candidates
                    .iter()
                    .find(|candidate| candidate.candidate_id == "warm")
                    .unwrap()
                    .eliminated_at,
                Some(RotationDecisionStageKind::ResourceHeadroom)
            );
            let lease = engine.reserve(&request, 100).unwrap();
            assert_eq!(lease.candidate_id, "expiring");
            assert!(engine.release_unstarted(lease.lease_id));
        }
    }
}

#[test]
fn expiry_replacement_preserves_load_and_health_exclusions_for_warm_routes() {
    let day = 24 * 60 * 60 * 1_000;
    for (mode, remaining) in [RotationMode::Automatic, RotationMode::SubscriptionExpiry]
        .into_iter()
        .flat_map(|mode| [1_000, 9_000].map(|remaining| (mode, remaining)))
    {
        for unhealthy in [false, true] {
            let mut engine = engine_with([
                spending_candidate("warm", remaining, Some(day / 2)),
                spending_candidate("expiring", 900, Some(day)),
                spending_candidate("later", 9_000, Some(12 * day)),
            ]);
            engine.set_mode(mode);
            let mut leases = Vec::new();
            if unhealthy {
                record_ready_latency(&mut engine, "warm", 5_000, 100);
                record_ready_latency(&mut engine, "expiring", 100, 100);
                record_ready_latency(&mut engine, "later", 100, 100);
            } else {
                for id in 10..12 {
                    leases.push(
                        engine
                            .reserve(&request(id).with_owner("warm"), 100)
                            .unwrap(),
                    );
                }
            }
            let request = preferred_request(1, "warm");
            let selected = engine.select(&request, 100).unwrap();
            assert_eq!(selected.candidate_id, "expiring");
            assert_eq!(
                engine
                    .cache_affinity_diagnostics(&request, Some("warm"), "expiring", 100)
                    .unwrap()
                    .reason,
                if unhealthy {
                    CacheAffinityReason::Health
                } else {
                    CacheAffinityReason::ParallelLoad
                }
            );
            let trace = engine.decision_trace(&request, 100);
            assert_eq!(trace.selected_candidate_id.as_deref(), Some("expiring"));
            assert_eq!(
                trace
                    .candidates
                    .iter()
                    .find(|candidate| candidate.candidate_id == "warm")
                    .unwrap()
                    .eliminated_at,
                Some(if unhealthy {
                    RotationDecisionStageKind::Health
                } else {
                    RotationDecisionStageKind::ParallelLoad
                })
            );
            for lease in leases {
                assert!(engine.release_unstarted(lease.lease_id));
            }
        }
    }
}

#[test]
fn nearest_at_risk_group_receives_independent_work_until_unavailable() {
    let day = 24 * 60 * 60 * 1_000;
    let mut engine = engine_with([
        spending_candidate("expiring", 6_000, Some(day)),
        spending_candidate("ordinary", 10_000, None),
    ]);
    assert_eq!(
        engine.select(&request(1), 100).unwrap().reason,
        RotationSelectionReason::SubscriptionExpiry
    );
    let served = (1..=60)
        .filter(|id| complete(&mut engine, *id) == "expiring")
        .count();
    assert_eq!(
        served, 60,
        "independent work should use the nearer available subscription"
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
fn expiry_priority_preserves_capacity_quota_auth_and_provider_waits() {
    for (mode, blocked) in [RotationMode::Automatic, RotationMode::SubscriptionExpiry]
        .into_iter()
        .flat_map(|mode| (0..7).map(move |blocked| (mode, blocked)))
    {
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
        engine.set_mode(mode);
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
        "expiring",
        "a non-full urgent member stays ahead of a later idle group"
    );
    assert!(engine.release_unstarted(busy.lease_id));
}

#[test]
fn stale_missing_future_observed_and_expired_deadlines_keep_ordinary_quota_ranking() {
    for deadline in [
        None,
        Some(QuotaSpendDeadline {
            expires_at_ms: 5_000,
            observed_at_ms: 101,
            valid_until_ms: 1_000,
            reset_credits: None,
        }),
        Some(QuotaSpendDeadline {
            expires_at_ms: 5_000,
            observed_at_ms: 50,
            valid_until_ms: 99,
            reset_credits: None,
        }),
        Some(QuotaSpendDeadline {
            expires_at_ms: 100,
            observed_at_ms: 50,
            valid_until_ms: 1_000,
            reset_credits: None,
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
fn earlier_nonurgent_subscription_precedes_later_full_quota_and_reset_budgets() {
    use crate::quota::QuotaSpendResets;

    for reset_count in [None, Some(1)] {
        let mut earlier = spending_candidate("earlier", 1_100, Some(9 * 86_400_000));
        earlier.spend_deadline.as_mut().unwrap().reset_credits =
            reset_count.map(|count| QuotaSpendResets {
                count,
                observed_at_ms: 100,
                valid_until_ms: 1_000,
            });
        assert_eq!(
            earlier.spend_deadline.unwrap().rotation_weight(1_100, 100),
            1
        );
        let mut later = spending_candidate("later", 10_000, Some(17 * 86_400_000));
        later.spend_deadline.as_mut().unwrap().reset_credits = Some(QuotaSpendResets {
            count: 3,
            observed_at_ms: 100,
            valid_until_ms: 1_000,
        });
        let engine = engine_with([earlier, later]);
        assert_eq!(engine.expiry_priority_band("earlier", 100), Some(2));
        assert_eq!(engine.expiry_priority_band("later", 100), Some(5));
        let selected = engine.select(&request(1), 100).unwrap();
        assert_eq!(selected.candidate_id, "earlier");
        assert_eq!(selected.reason, RotationSelectionReason::SubscriptionExpiry);
        for warm in [
            preferred_request(2, "later"),
            request(3).with_owner("later"),
        ] {
            assert_eq!(engine.select(&warm, 100).unwrap().candidate_id, "later");
        }
    }
}

#[test]
fn far_future_deadline_stays_behind_a_nearer_nonurgent_group() {
    let mut distant = spending_candidate("distant", 10_000, None);
    distant.spend_deadline = Some(QuotaSpendDeadline {
        expires_at_ms: u64::MAX,
        observed_at_ms: 50,
        valid_until_ms: 1_000,
        reset_credits: None,
    });
    let engine = engine_with([
        distant,
        spending_candidate("nearer", 1_100, Some(17 * 86_400_000)),
    ]);
    assert_eq!(
        engine.select(&request(1), 100).unwrap().candidate_id,
        "nearer"
    );
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

#[test]
fn five_near_accounts_share_work_before_six_later_accounts() {
    let day = 86_400_000;
    let members = (0..11).map(|index| {
        spending_candidate(
            &format!("member-{index:02}"),
            10_000,
            Some(if index < 5 { 4 * day } else { 12 * day }),
        )
    });
    let mut engine = engine_with(members);
    let mut counts = std::collections::BTreeMap::new();
    for id in 1..=100 {
        *counts.entry(complete(&mut engine, id)).or_insert(0) += 1;
    }
    assert_eq!(counts.len(), 5);
    assert!(counts.values().all(|count| *count == 20));
    assert_eq!(engine.active_leases(), 0);
}

#[test]
fn urgent_group_does_not_lose_its_last_positive_quota_before_reset() {
    let engine = engine_with([
        spending_candidate("urgent", 1, Some(2 * 86_400_000)),
        spending_candidate("later", 10_000, Some(12 * 86_400_000)),
    ]);
    assert_eq!(
        engine.select(&request(1), 100).unwrap().candidate_id,
        "urgent"
    );
}

#[test]
fn enabled_fresh_reset_budget_weights_only_the_nearest_group() {
    use crate::quota::QuotaSpendResets;
    let mut first = spending_candidate("first", 5_000, Some(4 * 86_400_000));
    let mut second = spending_candidate("second", 5_000, Some(4 * 86_400_000));
    for (candidate, count) in [(&mut first, 2), (&mut second, 1)] {
        candidate.spend_deadline.as_mut().unwrap().reset_credits = Some(QuotaSpendResets {
            count,
            observed_at_ms: 100,
            valid_until_ms: 1_000,
        });
    }
    let mut engine = engine_with([first, second]);
    assert_eq!(
        (1..=80)
            .filter(|id| complete(&mut engine, *id) == "first")
            .count(),
        50
    );
}
