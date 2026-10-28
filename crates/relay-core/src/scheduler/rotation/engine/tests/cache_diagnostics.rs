use super::*;

fn quota(engine: &mut RotationEngine, id: &str, remaining: u64, observed: u64) {
    assert!(engine.sync_candidate_state(
        id,
        true,
        false,
        AuthState::Ready,
        QuotaState::Available,
        RateState::Ready
    ));
    assert!(engine.set_quota_remaining(id, Some(remaining), Some(observed)));
}

fn inspect(engine: &RotationEngine, request: &RotationRequest) -> CacheAffinityDiagnostics {
    let before = engine.select(request, 100).unwrap();
    let diagnostics = engine
        .cache_affinity_diagnostics(
            request,
            request.preferred.as_deref(),
            &before.candidate_id,
            100,
        )
        .unwrap();
    assert_eq!(
        engine.select(request, 100),
        Some(before),
        "diagnostics must not advance rotation"
    );
    diagnostics
}

#[test]
fn cache_affinity_diagnostics_report_exact_quota_boundary_without_mutating_selection() {
    for (other, expected) in [
        (4_499, CacheAffinityReason::Retained),
        (4_500, CacheAffinityReason::Retained),
        (4_999, CacheAffinityReason::Retained),
        (5_000, CacheAffinityReason::QuotaHeadroom),
    ] {
        let mut engine = engine_with([
            candidate("private-owner", 0, 1),
            candidate("private-other", 0, 1),
        ]);
        quota(&mut engine, "private-owner", 4_000, 100);
        quota(&mut engine, "private-other", other, 100);
        let diagnostics = inspect(&engine, &preferred_request(1, "private-owner"));
        assert_eq!(diagnostics.reason, expected);
        assert_eq!(
            diagnostics
                .preferred
                .as_ref()
                .unwrap()
                .quota_remaining_basis_points,
            Some(4_000)
        );
        assert_eq!(diagnostics.quota_switch_margin_basis_points, 1_000);
        if expected == CacheAffinityReason::QuotaHeadroom {
            assert_eq!(diagnostics.best_quota_remaining_basis_points, Some(5_000));
            assert_eq!(
                diagnostics.selected.quota_remaining_basis_points,
                Some(5_000)
            );
        }
        let encoded = serde_json::to_string(&diagnostics).unwrap();
        assert!(!encoded.contains("private-owner"));
        assert!(!encoded.contains("private-other"));
        assert_eq!(
            serde_json::from_str::<CacheAffinityDiagnostics>(&encoded).unwrap(),
            diagnostics
        );
    }
}

#[test]
fn cache_affinity_diagnostics_do_not_explain_a_retained_owner_as_quota_rejection() {
    let mut engine = engine_with([candidate("owner", 0, 1), candidate("low", 0, 1)]);
    quota(&mut engine, "owner", 8_000, 100);
    quota(&mut engine, "low", 1_000, 100);
    let request = preferred_request(1, "owner");
    assert_eq!(
        engine.select(&request, 100).unwrap().reason,
        RotationSelectionReason::QuotaHeadroom
    );
    assert_eq!(
        inspect(&engine, &request).reason,
        CacheAffinityReason::Retained
    );
}

#[test]
fn cache_affinity_diagnostics_keep_unknown_or_future_quota_as_null() {
    let mut engine = engine_with([candidate("owner", 0, 1), candidate("other", 0, 1)]);
    quota(&mut engine, "owner", 9_000, 101);
    quota(&mut engine, "other", 4_000, 100);
    let diagnostics = inspect(&engine, &preferred_request(1, "owner"));
    assert_eq!(diagnostics.reason, CacheAffinityReason::QuotaHeadroom);
    assert_eq!(
        diagnostics.preferred.unwrap().quota_remaining_basis_points,
        None
    );
    assert_eq!(
        diagnostics.selected.quota_remaining_basis_points,
        Some(4_000)
    );
}

#[test]
fn cache_affinity_diagnostics_explain_normalized_load_without_ignoring_capacity() {
    let mut owner = candidate("owner", 0, 1);
    owner.capacity_limit = 4;
    let mut other = candidate("other", 0, 1);
    other.capacity_limit = 4;
    let mut engine = engine_with([owner, other]);
    quota(&mut engine, "owner", 3_000, 100);
    quota(&mut engine, "other", 3_500, 100);
    let first = engine
        .reserve(&request(1).with_owner("owner"), 100)
        .unwrap();
    let second = engine
        .reserve(&request(2).with_owner("owner"), 100)
        .unwrap();
    let diagnostics = inspect(&engine, &preferred_request(3, "owner"));
    assert_eq!(diagnostics.reason, CacheAffinityReason::ParallelLoad);
    assert_eq!(diagnostics.preferred.as_ref().unwrap().in_flight, 2);
    assert_eq!(diagnostics.preferred.unwrap().effective_capacity, 4);
    assert_eq!(diagnostics.selected.in_flight, 0);
    assert_eq!(diagnostics.load_allowance_requests, 1);
    assert!(engine.release_unstarted(first.lease_id));
    assert!(engine.release_unstarted(second.lease_id));
}

#[test]
fn cache_affinity_diagnostics_distinguish_busy_owner_and_warm_alternative() {
    let mut owner = candidate("owner", 0, 1);
    owner.capacity_limit = 1;
    let mut engine = engine_with([owner, candidate("a-cold", 0, 1), candidate("z-warm", 0, 1)]);
    quota(&mut engine, "owner", 3_000, 100);
    quota(&mut engine, "a-cold", 3_500, 100);
    quota(&mut engine, "z-warm", 3_500, 100);
    let lease = engine
        .reserve(&request(1).with_owner("owner"), 100)
        .unwrap();
    let mut request = preferred_request(2, "owner");
    request.warm_fallbacks = vec!["z-warm".into()];
    let diagnostics = inspect(&engine, &request);
    assert_eq!(
        diagnostics.selection,
        CacheAffinitySelection::WarmAlternative
    );
    assert_eq!(diagnostics.reason, CacheAffinityReason::OwnerUnavailable);
    assert_eq!(
        diagnostics.owner_block_reason,
        Some(CandidateBlockReason::CapacityBusy)
    );
    assert!(engine.release_unstarted(lease.lease_id));
}

#[test]
fn cache_affinity_diagnostics_preserve_scope_and_manual_policy() {
    let mut engine = engine_with([candidate("owner", 0, 1), candidate("other", 10, 1)]);
    let mut scoped = preferred_request(1, "owner");
    scoped.allowed_candidates = Some(BTreeSet::from(["other".into()]));
    let diagnostics = inspect(&engine, &scoped);
    assert_eq!(
        diagnostics.owner_block_reason,
        Some(CandidateBlockReason::NotAllowed)
    );
    engine.set_mode(RotationMode::Failover);
    assert_eq!(
        inspect(&engine, &preferred_request(2, "owner")).reason,
        CacheAffinityReason::RoutingPolicy
    );
    assert_eq!(
        inspect(&engine, &request(3)).reason,
        CacheAffinityReason::NoBinding
    );
}

#[test]
fn cache_affinity_diagnostics_report_fresh_credit_margin_and_unlimited_safely() {
    let mut engine = engine_with([candidate("owner", 0, 1), candidate("other", 0, 1)]);
    assert!(engine.set_provider_credits("owner", Some(20_000_000), false, Some(100)));
    assert!(engine.set_provider_credits("other", Some(35_000_000), false, Some(100)));
    let diagnostics = inspect(&engine, &preferred_request(1, "owner"));
    assert_eq!(diagnostics.reason, CacheAffinityReason::ProviderCredits);
    assert_eq!(
        diagnostics.provider_credit_switch_margin_micro_units,
        15_000_000
    );
    assert_eq!(
        diagnostics.best_provider_credits_micro_units,
        Some(35_000_000)
    );
    assert!(engine.set_provider_credits("other", None, true, Some(100)));
    let diagnostics = inspect(&engine, &preferred_request(2, "owner"));
    assert!(diagnostics.selected.provider_credits_unlimited);
    assert_eq!(diagnostics.selected.provider_credits_micro_units, None);
    assert_eq!(diagnostics.best_provider_credits_micro_units, None);
}

#[test]
fn cache_affinity_diagnostics_preserve_health_filtering() {
    let mut engine = engine_with([candidate("owner", 0, 1), candidate("other", 0, 1)]);
    quota(&mut engine, "owner", 3_000, 100);
    quota(&mut engine, "other", 3_500, 100);
    engine.health.insert(
        ("owner".into(), "responses:gpt".into()),
        HealthRuntime {
            failure_score_milli: HEALTH_FAILURE_SWITCH_MARGIN_MILLI,
            observed_at_ms: Some(100),
            ..HealthRuntime::default()
        },
    );
    assert_eq!(
        inspect(&engine, &preferred_request(1, "owner")).reason,
        CacheAffinityReason::Health
    );
}
