use super::*;

fn quota_candidate(id: &str, remaining: u64) -> RotationCandidate {
    let mut member = candidate(id, 0, 1);
    member.quota = QuotaState::Available;
    member.quota_remaining_basis_points = Some(remaining);
    member.quota_observed_at_ms = Some(100);
    member
}

#[test]
fn warm_quota_owner_survives_five_points_but_yields_at_ten() {
    for (remaining, expected) in [(3_500, "owner"), (3_999, "owner"), (4_000, "other")] {
        let engine = engine_with([
            quota_candidate("owner", 3_000),
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
    for gate in ["disabled", "draining", "auth", "quota", "rate"] {
        let mut owner = quota_candidate("owner", 3_000);
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
