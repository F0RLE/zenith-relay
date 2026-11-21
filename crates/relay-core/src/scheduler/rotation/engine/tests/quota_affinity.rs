use super::*;

#[test]
fn credit_fallback_rejects_stale_and_future_observations_before_dispatch() {
    let now = 100_000;
    for (balance, unlimited) in [(Some(100), false), (None, true)] {
        for invalid_observation in [now - 60_001, u64::MAX] {
            let mut member = candidate("credit-only", 0, 1);
            member.quota = QuotaState::CreditFallback { reset_at_ms: None };
            let mut engine = engine_with([member]);
            engine.set_quota_stale_after_ms(60_000);
            engine.set_provider_credits("credit-only", balance, unlimited, Some(now));
            let lease = engine.reserve(&request(1), now).unwrap();
            engine.set_provider_credits(
                "credit-only",
                balance,
                unlimited,
                Some(invalid_observation),
            );
            assert!(engine.select(&request(2), now).is_none());
            let mut budget = RequestBudget::default_for(lease.request_id);
            assert_eq!(
                engine.begin_dispatch_at(lease.lease_id, &mut budget, now),
                Err(DispatchStartError::CandidateChanged)
            );
            assert_eq!(budget.dispatches(), 0);
            assert!(engine.release_unstarted(lease.lease_id));
        }
    }
}

#[test]
fn credit_fallback_expires_while_reserved_and_between_transport_sends() {
    let mut member = candidate("credit-only", 0, 1);
    member.quota = QuotaState::CreditFallback { reset_at_ms: None };
    let mut engine = engine_with([member]);
    engine.set_quota_stale_after_ms(10);
    engine.set_provider_credits("credit-only", Some(100), false, Some(100));
    let lease = engine.reserve(&request(1), 100).unwrap();
    let mut budget = RequestBudget::new(lease.request_id, 2).unwrap();
    assert!(engine.select(&request(2), 110).is_some());
    assert!(engine.select(&request(2), 111).is_none());
    assert_eq!(
        engine.begin_dispatch_at(lease.lease_id, &mut budget, 111),
        Err(DispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    engine.set_provider_credits("credit-only", Some(100), false, Some(111));
    assert_eq!(
        engine.begin_dispatch_at(lease.lease_id, &mut budget, 121),
        Ok(AttemptId(1))
    );
    assert_eq!(
        engine.begin_transport_dispatch(lease.lease_id, &mut budget, 122),
        Err(DispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 1);
    engine.set_provider_credits("credit-only", None, true, Some(122));
    assert_eq!(
        engine.begin_transport_dispatch(lease.lease_id, &mut budget, 122),
        Ok(AttemptId(2))
    );
    engine.cancel(lease.lease_id, &budget, 122).unwrap();
    assert_eq!(engine.active_leases(), 0);
}

#[test]
fn revoked_credit_fallback_blocks_an_already_reserved_dispatch() {
    for (balance, observed_at_ms) in [(Some(0), Some(200)), (None, Some(200)), (Some(100), None)] {
        let mut member = candidate("credit-only", 0, 1);
        member.quota = QuotaState::CreditFallback { reset_at_ms: None };
        member.provider_credits_micro_units = Some(100);
        member.provider_credits_observed_at_ms = Some(100);
        let mut engine = engine_with([member]);
        let request = request(1);
        let lease = engine.reserve(&request, 100).unwrap();
        engine.set_provider_credits("credit-only", balance, false, observed_at_ms);
        let mut budget = RequestBudget::default_for(request.request_id);
        assert_eq!(
            engine.begin_dispatch_at(lease.lease_id, &mut budget, 200),
            Err(DispatchStartError::CandidateChanged)
        );
        assert_eq!(budget.dispatches(), 0);
        assert!(engine.release_unstarted(lease.lease_id));
        assert_eq!(engine.active_leases(), 0);
    }
}

#[test]
fn credit_fallback_requires_fresh_positive_or_unlimited_balance() {
    for (balance, unlimited, expected_available) in [
        (Some(0), false, false),
        (None, false, false),
        (Some(10), false, true),
        (None, true, true),
    ] {
        let mut member = candidate("credit-only", 0, 1);
        member.quota = QuotaState::CreditFallback { reset_at_ms: None };
        member.provider_credits_micro_units = Some(100);
        member.provider_credits_observed_at_ms = Some(100);
        let mut engine = engine_with([member]);
        assert!(engine.select(&request(1), 100).is_some());
        engine.set_provider_credits("credit-only", balance, unlimited, Some(200));
        assert_eq!(
            engine.select(&request(2), 200).is_some(),
            expected_available,
            "a fresh timestamp alone is not spendable credit evidence"
        );
        let reserved = engine.reserve(&request(3), 200);
        assert_eq!(reserved.is_ok(), expected_available);
        if let Ok(lease) = reserved {
            assert!(engine.release_unstarted(lease.lease_id));
        }
    }
}

fn quota_candidate(id: &str, remaining: u64) -> RotationCandidate {
    let mut member = candidate(id, 0, 1);
    member.quota = QuotaState::Available;
    member.quota_remaining_basis_points = Some(remaining);
    member.quota_observed_at_ms = Some(100);
    member
}

#[test]
fn healthy_warm_owner_keeps_cache_despite_a_larger_quota_gap() {
    for (owner_remaining, other_remaining) in [
        (5_600, 7_000),
        (4_600, 8_000),
        (4_600, 6_400),
        (1_001, 10_000),
    ] {
        let engine = engine_with([
            quota_candidate("owner", owner_remaining),
            quota_candidate("other", other_remaining),
        ]);
        let warm = preferred_request(1, "owner");
        let selected = engine.select(&warm, 100).unwrap();
        assert_eq!(selected.candidate_id, "owner");
        assert!(!engine.has_material_resource_lead("other", "owner", 100));
        assert_eq!(
            engine
                .cache_affinity_diagnostics(&warm, Some("owner"), &selected.candidate_id, 100)
                .unwrap()
                .reason,
            CacheAffinityReason::Retained
        );
        assert_eq!(
            engine.select(&request(2), 100).unwrap().candidate_id,
            "other",
            "independent work still uses the larger available quota"
        );
    }
}

#[test]
fn low_quota_warm_owner_yields_only_at_a_ten_point_lead() {
    for (owner_remaining, remaining, expected) in [
        (1_000, 1_500, "owner"),
        (1_000, 1_999, "owner"),
        (1_000, 2_000, "other"),
        (1, 1_000, "owner"),
        (1, 1_001, "other"),
    ] {
        let engine = engine_with([
            quota_candidate("owner", owner_remaining),
            quota_candidate("other", remaining),
        ]);
        let request = preferred_request(1, "owner");
        let selected = engine.select(&request, 100).unwrap();
        assert_eq!(selected.candidate_id, expected);
        assert_eq!(
            selected.reason,
            if expected == "owner" {
                RotationSelectionReason::WeightedRotation
            } else {
                RotationSelectionReason::QuotaHeadroom
            }
        );
        assert_eq!(
            engine.has_material_resource_lead("other", "owner", 100),
            expected == "other"
        );
    }
}

#[test]
fn warm_owner_with_linked_quota_does_not_rebalance_early() {
    for (remaining, valid_until, quota, expected) in [
        (900, 200, QuotaState::Unknown, "owner"),
        (1, 200, QuotaState::Unknown, "owner"),
        (0, 200, QuotaState::Unknown, "other"),
        (900, 100, QuotaState::Unknown, "other"),
        (900, 200, QuotaState::Available, "other"),
        (
            900,
            200,
            QuotaState::Exhausted { reset_at_ms: None },
            "other",
        ),
    ] {
        let mut owner = quota_candidate("owner", 900);
        owner.quota = quota;
        owner.quota_reference = Some(crate::CandidateQuotaReference {
            source_account_id: "synthetic-linked".into(),
            remaining_basis_points: remaining,
            observed_at_ms: 100,
            valid_until_ms: Some(valid_until),
        });
        let engine = engine_with([owner, quota_candidate("other", 9_800)]);
        let warm = preferred_request(1, "owner");
        let selected = engine.select(&warm, 100).unwrap();
        assert_eq!(selected.candidate_id, expected);
        let diagnostics = engine
            .cache_affinity_diagnostics(&warm, Some("owner"), expected, 100)
            .unwrap();
        assert_eq!(
            diagnostics.quota_low_water_basis_points,
            Some(if quota == QuotaState::Unknown {
                0
            } else {
                1_000
            })
        );
        if expected == "owner" {
            assert!(!engine.has_material_resource_lead("other", "owner", 100));
        }
        assert_eq!(
            engine.select(&request(2), 100).unwrap().candidate_id,
            "other"
        );
    }
}

#[test]
fn warm_siwc_quota_comparison_distinguishes_missing_evidence_from_fresh_zero() {
    for mode in [RotationMode::Automatic, RotationMode::SubscriptionExpiry] {
        for (reference, should_keep) in [
            (None, true),
            (Some((900, 200)), true),
            (Some((201, 200)), true),
            (Some((200, 200)), false),
            (Some((100, 200)), false),
            (Some((900, 100)), true),
            (Some((100, 100)), true),
            (Some((0, 100)), true),
            (Some((0, 200)), false),
        ] {
            let mut owner = candidate("owner", 0, 1);
            owner.keep_unknown_quota_affinity = true;
            owner.quota_reference =
                reference.map(|(remaining, valid_until)| crate::CandidateQuotaReference {
                    source_account_id: "synthetic-linked".into(),
                    remaining_basis_points: remaining,
                    observed_at_ms: 100,
                    valid_until_ms: Some(valid_until),
                });
            let mut engine = engine_with([owner, quota_candidate("other", 9_800)]);
            engine.set_mode(mode);
            let warm = preferred_request(1, "owner");
            let expected = if should_keep { "owner" } else { "other" };
            assert_eq!(engine.select(&warm, 100).unwrap().candidate_id, expected);
            let diagnostics = engine
                .cache_affinity_diagnostics(&warm, Some("owner"), expected, 100)
                .unwrap();
            assert_eq!(diagnostics.quota_low_water_basis_points, Some(200));
            assert_eq!(
                engine.has_material_resource_lead("other", "owner", 100),
                !should_keep
            );
            assert_eq!(
                engine.select(&request(2), 100).unwrap().candidate_id,
                "other"
            );
        }
    }
}

#[test]
fn low_siwc_estimate_is_not_an_admission_block() {
    let mut owner = candidate("owner", 10, 1);
    owner.keep_unknown_quota_affinity = true;
    owner.quota_reference = Some(crate::CandidateQuotaReference {
        source_account_id: "synthetic-linked".into(),
        remaining_basis_points: 100,
        observed_at_ms: 100,
        valid_until_ms: Some(200),
    });
    for (remaining, expected) in [(1_099, "owner"), (1_100, "other"), (7_700, "other")] {
        let engine = engine_with([owner.clone(), quota_candidate("other", remaining)]);
        let warm = preferred_request(1, "owner");
        assert_eq!(engine.select(&warm, 100).unwrap().candidate_id, expected);
        let only = warm
            .clone()
            .with_allowed_candidates(BTreeSet::from(["owner".to_owned()]));
        assert_eq!(engine.select(&only, 100).unwrap().candidate_id, "owner");
        let opaque = warm.with_owner("owner");
        assert_eq!(engine.select(&opaque, 100).unwrap().candidate_id, "owner");
    }
}

fn quota_wait_with_spare(
    linked: Option<(u64, u64)>,
    siwc: bool,
) -> (RotationEngine, RotationRequest) {
    let mut owner = candidate("owner", 0, 1);
    owner.quota = QuotaState::Exhausted { reset_at_ms: None };
    let mut spare = candidate("spare", 0, 1);
    spare.keep_unknown_quota_affinity = siwc;
    spare.quota_reference = linked.map(|(remaining, until)| crate::CandidateQuotaReference {
        source_account_id: "linked".into(),
        remaining_basis_points: remaining,
        observed_at_ms: 100,
        valid_until_ms: Some(until),
    });
    let mut cold = quota_candidate("cold", 9_400);
    cold.spend_deadline = Some(crate::quota::QuotaSpendDeadline {
        expires_at_ms: 1_000,
        observed_at_ms: 100,
        valid_until_ms: 200,
        reset_credits: None,
    });
    let mut chat = preferred_request(1, "owner");
    chat.warm_fallbacks = vec!["spare".into()];
    (engine_with([owner, spare, cold]), chat)
}

#[test]
fn a_measured_linked_zero_needs_a_full_ten_point_gain_to_move_a_warm_chat() {
    for mode in [RotationMode::Automatic, RotationMode::SubscriptionExpiry] {
        for spare in [false, true] {
            for (remaining, should_switch) in [(100, false), (999, false), (1_000, true)] {
                let (mut engine, chat) = quota_wait_with_spare(Some((0, 200)), true);
                engine.set_mode(mode);
                engine.set_quota_remaining("cold", Some(remaining), Some(100));
                let chat = if spare {
                    chat
                } else {
                    preferred_request(1, "spare")
                };
                let expected = if should_switch { "cold" } else { "spare" };
                assert_eq!(engine.select(&chat, 100).unwrap().candidate_id, expected);
                assert_eq!(
                    engine.has_material_resource_lead("cold", "spare", 100),
                    should_switch
                );
                let trace = engine.decision_trace(&chat, 100);
                assert_eq!(trace.selected_candidate_id.as_deref(), Some(expected));
                let row = trace
                    .candidates
                    .iter()
                    .find(|row| row.candidate_id == "spare")
                    .unwrap();
                assert_eq!(row.quota_remaining_basis_points, Some(0));
                let lease = engine.reserve(&chat, 100).unwrap();
                assert_eq!(lease.candidate_id, expected);
                assert!(engine.release_unstarted(lease.lease_id));
            }
        }
    }
}

#[test]
fn a_weaker_cold_replacement_cannot_borrow_another_routes_lead_over_a_warm_spare() {
    let (mut engine, chat) = quota_wait_with_spare(Some((100, 200)), true);
    engine.set_quota_remaining("cold", Some(1_100), Some(100));
    let mut weak = quota_candidate("a-weak", 900);
    weak.weight = 100;
    weak.spend_deadline = engine.candidate("cold").unwrap().spend_deadline;
    engine.upsert(weak).unwrap();
    for id in 1..=8 {
        let mut chat = chat.clone();
        chat.request_id = RequestId(id);
        let lease = engine.reserve(&chat, 100).unwrap();
        assert_eq!(lease.candidate_id, "cold");
        assert!(engine.release_unstarted(lease.lease_id));
    }
}

#[test]
fn siwc_spare_ignores_only_an_indirect_quota_gap_during_owner_exhaustion() {
    for mode in [RotationMode::Automatic, RotationMode::SubscriptionExpiry] {
        for (linked, siwc, expected) in [
            (Some((900, 200)), true, "spare"),
            (Some((100, 200)), true, "cold"),
            (None, true, "spare"),
            (Some((0, 100)), true, "spare"),
            (Some((0, 200)), true, "cold"),
            (Some((900, 200)), false, "cold"),
        ] {
            let (mut engine, chat) = quota_wait_with_spare(linked, siwc);
            engine.set_mode(mode);
            assert_eq!(engine.select(&chat, 100).unwrap().candidate_id, expected);
            let trace = engine.decision_trace(&chat, 100);
            assert_eq!(trace.selected_candidate_id.as_deref(), Some(expected));
            let lease = engine.reserve(&chat, 100).unwrap();
            assert_eq!(lease.candidate_id, expected);
            assert!(engine.release_unstarted(lease.lease_id));
            // Even a fresh linked zero is not a provider denial for the spare.
            let alone = chat.with_allowed_candidates(BTreeSet::from(["spare".into()]));
            assert_eq!(engine.select(&alone, 100).unwrap().candidate_id, "spare");
        }
    }
}

#[test]
fn siwc_spare_still_requires_scope_load_health_and_its_own_admission() {
    let (mut engine, chat) = quota_wait_with_spare(Some((900, 200)), true);
    let hard_owner = chat.clone().with_owner("owner");
    assert!(engine.select(&hard_owner, 100).is_none());
    for excluded in ["owner", "spare"] {
        let mut tried = chat.clone();
        tried.tried.insert(excluded.into());
        assert_eq!(engine.select(&tried, 100).unwrap().candidate_id, "cold");
    }
    let scoped = chat
        .clone()
        .with_allowed_candidates(BTreeSet::from(["cold".into(), "spare".into()]));
    assert_eq!(engine.select(&scoped, 100).unwrap().candidate_id, "cold");

    let busy = engine
        .reserve(&request(2).with_owner("spare"), 100)
        .unwrap();
    assert_eq!(engine.select(&chat, 100).unwrap().candidate_id, "cold");
    assert!(engine.release_unstarted(busy.lease_id));
    record_ready_latency(&mut engine, "spare", 1_000, 100);
    record_ready_latency(&mut engine, "cold", 100, 100);
    assert_eq!(engine.select(&chat, 100).unwrap().candidate_id, "cold");
    let alone = chat.with_allowed_candidates(BTreeSet::from(["spare".into()]));
    assert!(engine.sync_candidate_state(
        "spare",
        true,
        false,
        AuthState::Ready,
        QuotaState::Unknown,
        RateState::Limited { not_before_ms: 200 },
    ));
    assert!(engine.select(&alone, 100).is_none());
}

#[test]
fn quota_rebalance_margin_applies_to_every_replacement_not_just_the_best() {
    for mode in [RotationMode::Automatic, RotationMode::SubscriptionExpiry] {
        let mut weak = quota_candidate("weak", 1_600);
        weak.weight = 100;
        let mut engine = engine_with([
            quota_candidate("owner", 1_000),
            quota_candidate("best", 2_000),
            weak,
        ]);
        engine.set_mode(mode);
        for id in 1..=40 {
            let request = preferred_request(id, "owner");
            let (ready, _) = engine.selection_candidates(&request, 100);
            let admitted = engine.selection_group(&ready, &request, 100);
            assert_eq!(admitted.len(), 1);
            assert_eq!(admitted[0].id, "best");
            let lease = engine.reserve(&request, 100).unwrap();
            assert_eq!(lease.candidate_id, "best");
            assert!(engine.release_unstarted(lease.lease_id));
        }
    }
}

#[test]
fn wider_quota_band_applies_only_to_the_primary_preference() {
    let engine = engine_with([
        quota_candidate("owner", 3_000),
        quota_candidate("alternative", 3_100),
        quota_candidate("best", 3_600),
    ]);
    let mut preferred = preferred_request(1, "owner");
    preferred.warm_fallbacks = vec!["alternative".into()];
    let (ready, _) = engine.selection_candidates(&preferred, 100);
    let admitted = engine.selection_group(&ready, &preferred, 100);
    assert_eq!(admitted.len(), 2);
    assert!(admitted.iter().any(|candidate| candidate.id == "owner"));
    assert!(admitted.iter().any(|candidate| candidate.id == "best"));
    assert_eq!(
        engine.select(&preferred, 100).unwrap().candidate_id,
        "owner"
    );

    // Neither an independent request nor a warm alternative inherits the
    // primary owner's tolerance. Their original five-point band still applies.
    for warm_fallbacks in [vec![], vec!["alternative".into(), "owner".into()]] {
        let mut independent = request(2);
        independent.warm_fallbacks = warm_fallbacks;
        let selected = engine.select(&independent, 100).unwrap();
        assert_eq!(selected.candidate_id, "best");
        assert_eq!(selected.reason, RotationSelectionReason::QuotaHeadroom);
        let diagnostics = engine
            .cache_affinity_diagnostics(&independent, None, &selected.candidate_id, 100)
            .unwrap();
        assert_eq!(diagnostics.quota_switch_margin_basis_points, 500);
        assert_eq!(diagnostics.quota_low_water_basis_points, None);
    }
}

#[test]
fn ordinary_quota_boundary_remains_five_points() {
    for (remaining, eligible) in [(3_499, 2), (3_500, 1)] {
        let engine = engine_with([
            quota_candidate("lower", 3_000),
            quota_candidate("higher", remaining),
        ]);
        let request = request(1);
        let (ready, _) = engine.selection_candidates(&request, 100);
        assert_eq!(
            engine.selection_group(&ready, &request, 100).len(),
            eligible
        );
    }
}

#[test]
fn wider_quota_band_requires_fresh_positive_owner_evidence() {
    for (state, remaining, observed, now) in [
        (QuotaState::Unknown, Some(3_000), Some(100), 100),
        (QuotaState::Stale, Some(3_000), Some(100), 100),
        (QuotaState::Available, None, Some(100), 100),
        (QuotaState::Available, Some(0), Some(100), 100),
        (QuotaState::Available, Some(3_000), None, 100),
        (QuotaState::Available, Some(3_000), Some(101), 100),
        (QuotaState::Available, Some(3_000), Some(100), 1_101),
    ] {
        let mut owner = quota_candidate("owner", 3_000);
        owner.quota = state;
        owner.quota_remaining_basis_points = remaining;
        owner.quota_observed_at_ms = observed;
        let mut other = quota_candidate("other", 3_500);
        other.quota_observed_at_ms = Some(now);
        let mut engine = engine_with([owner, other]);
        engine.quota_stale_after_ms = 1_000;
        let request = preferred_request(1, "owner");
        let selected = engine.select(&request, now).unwrap();
        assert_eq!(selected.candidate_id, "other");
        let diagnostics = engine
            .cache_affinity_diagnostics(&request, Some("owner"), "other", now)
            .unwrap();
        assert_eq!(diagnostics.reason, CacheAffinityReason::QuotaHeadroom);
        assert_eq!(
            diagnostics.preferred.unwrap().quota_remaining_basis_points,
            None
        );
    }
}

#[test]
fn wider_quota_band_never_overrides_owner_admission_gates() {
    for siwc in [false, true] {
        for gate in ["disabled", "draining", "auth", "quota", "rate"] {
            let mut owner = if siwc {
                let mut owner = candidate("owner", 0, 1);
                owner.keep_unknown_quota_affinity = true;
                owner
            } else {
                quota_candidate("owner", 3_000)
            };
            match gate {
                "disabled" => owner.enabled = false,
                "draining" => owner.draining = true,
                "auth" => owner.auth = AuthState::Blocked,
                "quota" => owner.quota = QuotaState::Exhausted { reset_at_ms: None },
                "rate" => owner.rate = RateState::Limited { not_before_ms: 200 },
                _ => unreachable!(),
            }
            let engine = engine_with([owner, quota_candidate("other", 3_500)]);
            let request = preferred_request(1, "owner");
            assert_eq!(engine.select(&request, 100).unwrap().candidate_id, "other");
            let diagnostics = engine
                .cache_affinity_diagnostics(&request, Some("owner"), "other", 100)
                .unwrap();
            assert_eq!(diagnostics.reason, CacheAffinityReason::OwnerUnavailable);
            assert!(diagnostics.owner_block_reason.is_some());
        }
    }
}
