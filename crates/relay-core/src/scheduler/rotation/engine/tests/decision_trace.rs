use super::*;

const NOW: u64 = 1_000;
const DAY: u64 = 86_400_000;

fn expiring(id: &str, days: u64, resets: u32) -> RotationCandidate {
    let mut value = candidate(id, 0, 1);
    value.quota = QuotaState::Available;
    value.quota_remaining_basis_points = Some(9_000);
    value.quota_observed_at_ms = Some(NOW);
    value.spend_deadline = Some(crate::quota::QuotaSpendDeadline {
        expires_at_ms: NOW + days * DAY,
        observed_at_ms: NOW,
        valid_until_ms: NOW + DAY,
        reset_credits: Some(crate::quota::QuotaSpendResets {
            count: resets,
            observed_at_ms: NOW,
            valid_until_ms: NOW + DAY,
        }),
    });
    value
}

#[test]
fn explains_expiry_weighting_and_every_fallback_without_mutating_selection() {
    let engine = engine_with([
        expiring("near-more-resets", 2, 2),
        expiring("near", 2, 0),
        expiring("later", 5, 1),
    ]);
    let request = request(1);
    let before = format!("{engine:?}");
    let trace = engine.decision_trace(&request, NOW);
    assert_eq!(
        trace.selected_candidate_id.as_deref(),
        Some("near-more-resets")
    );
    assert_eq!(
        trace.selected_candidate_id,
        engine.select(&request, NOW).map(|s| s.candidate_id)
    );
    assert_eq!(
        trace
            .candidates
            .iter()
            .map(|c| c.candidate_id.as_str())
            .collect::<Vec<_>>(),
        ["near-more-resets", "near", "later"]
    );
    assert_eq!(trace.candidates[0].effective_weight, Some(8));
    assert_eq!(trace.candidates[0].rotation_score, Some(8));
    assert_eq!(trace.candidates[1].effective_weight, Some(4));
    assert_eq!(
        trace.candidates[2].eliminated_at,
        Some(RotationDecisionStageKind::SubscriptionExpiry)
    );
    assert_eq!(trace.candidates[2].fallback_rank, Some(3));
    assert_eq!(trace.candidates[2].effective_weight, None);
    assert_eq!(format!("{engine:?}"), before);
    assert_eq!(engine.decision_trace(&request, NOW), trace);
    let encoded = serde_json::to_string(&trace).unwrap();
    assert_eq!(
        serde_json::from_str::<RotationDecisionTrace>(&encoded).unwrap(),
        trace
    );
}

#[test]
fn empty_selection_keeps_each_admission_failure_and_unknown_evidence() {
    let mut disabled = candidate("disabled", 0, 1);
    disabled.enabled = false;
    let mut waiting = candidate("waiting", 0, 1);
    waiting.quota = QuotaState::Exhausted {
        reset_at_ms: Some(NOW + DAY),
    };
    let engine = engine_with([disabled, waiting]);
    let trace = engine.decision_trace(&request(1), NOW);
    assert_eq!(trace.selected_candidate_id, None);
    assert_eq!(trace.candidates.len(), 2);
    let disabled = &trace.candidates[0];
    assert_eq!(disabled.block_reason, Some(CandidateBlockReason::Disabled));
    assert_eq!(disabled.quota_remaining_basis_points, None);
    assert_eq!(disabled.expiry, None);
    let waiting = &trace.candidates[1];
    assert_eq!(waiting.availability, RotationDecisionAvailability::Waiting);
    assert_eq!(waiting.retry_at_ms, Some(NOW + DAY));
    assert_eq!(
        waiting.block_reason,
        Some(CandidateBlockReason::QuotaExhausted)
    );
    assert!(trace.candidates.iter().all(|c| c.fallback_rank.is_none()));
}

#[test]
fn preserves_hard_ownership_and_soft_affinity_in_the_trace() {
    let engine = engine_with([expiring("near", 2, 2), expiring("warm", 12, 0)]);
    let owner = engine.decision_trace(&request(1).with_owner("warm"), NOW);
    assert_eq!(owner.selected_candidate_id.as_deref(), Some("warm"));
    assert_eq!(
        owner
            .candidates
            .iter()
            .filter(|c| c.fallback_rank.is_some())
            .count(),
        1
    );
    assert_eq!(
        owner.candidates[1].block_reason,
        Some(CandidateBlockReason::OwnerMismatch)
    );
    let warm = engine.decision_trace(&preferred_request(2, "warm"), NOW);
    assert_eq!(warm.selected_candidate_id.as_deref(), Some("warm"));
    assert!(
        !warm
            .stages
            .iter()
            .any(|s| s.kind == RotationDecisionStageKind::SubscriptionExpiry)
    );
}

#[test]
fn physical_aliases_never_get_an_extra_fallback_vote() {
    let mut alias = candidate("alias", 0, 1);
    alias.capacity_key = "same".into();
    let mut native = candidate("native", 0, 1);
    native.capacity_key = "same".into();
    let engine = engine_with([alias, native]);
    let trace = engine.decision_trace(&request(1), NOW);
    assert_eq!(
        trace
            .candidates
            .iter()
            .filter(|c| c.fallback_rank.is_some())
            .count(),
        1
    );
    assert_eq!(
        trace.candidates[1].eliminated_at,
        Some(RotationDecisionStageKind::PhysicalMember)
    );
}

#[test]
fn manual_modes_report_their_real_order_and_do_not_apply_expiry_weights() {
    for mode in [
        RotationMode::Failover,
        RotationMode::RoundRobin,
        RotationMode::WeightedRoundRobin,
    ] {
        let mut preferred = expiring("manual", 12, 0);
        preferred.priority = 10;
        preferred.weight = 5;
        let mut engine = engine_with([preferred, expiring("urgent", 2, 3)]);
        engine.mode = mode;
        let before = format!("{engine:?}");
        let trace = engine.decision_trace(&request(1), NOW);
        assert_eq!(trace.selected_candidate_id.as_deref(), Some("manual"));
        assert_eq!(trace.mode, mode);
        assert!(
            !trace
                .stages
                .iter()
                .any(|s| s.kind == RotationDecisionStageKind::SubscriptionExpiry)
        );
        assert_eq!(trace.candidates[0].configured_weight, 5);
        assert_eq!(
            trace.candidates[0].effective_weight,
            Some(if mode == RotationMode::RoundRobin {
                1
            } else {
                5
            })
        );
        assert_eq!(format!("{engine:?}"), before);
    }
}

#[test]
fn stale_reset_inventory_is_not_reported_as_a_zero_balance() {
    let mut value = expiring("near", 2, 2);
    value
        .spend_deadline
        .as_mut()
        .unwrap()
        .reset_credits
        .as_mut()
        .unwrap()
        .valid_until_ms = NOW - 1;
    let engine = engine_with([value]);
    let trace = engine.decision_trace(&request(1), NOW);
    assert_eq!(
        trace.candidates[0].expiry.as_ref().unwrap().reset_credits,
        None
    );
    assert_eq!(trace.candidates[0].effective_weight, Some(4));
}

#[test]
fn large_pools_keep_all_rows_and_report_a_bounded_fallback_walk() {
    let engine = engine_with((0..257).map(|index| candidate(&format!("member-{index:03}"), 0, 1)));
    let trace = engine.decision_trace(&request(1), NOW);
    assert_eq!(trace.candidates.len(), 257);
    assert_eq!(
        trace
            .candidates
            .iter()
            .filter(|candidate| candidate.fallback_rank.is_some())
            .count(),
        256
    );
    assert!(!trace.fallback_order_complete);
    assert_eq!(
        trace.candidates[0].candidate_id,
        trace.selected_candidate_id.unwrap()
    );
}
