use super::*;

#[test]
fn rotation_view_keeps_inventory_and_deduplicates_source_bindings() {
    let source = source_summary("source", &["model"]);
    let mut account = account_summary(true, &["model"]);
    let mut hidden = account.clone();
    hidden.id = "inactive".into();
    hidden.route_inactive = true;
    account.operational_status = OperationalStatus::Unavailable;
    let accounts = [account, hidden];
    let sources = [source];
    let policy = pool_routing_summary(None, &sources, &accounts);
    let runtime = [
        runtime_candidate("source::responses", CandidateKind::ApiSource, true),
        runtime_candidate("source::messages", CandidateKind::ApiSource, false),
    ];
    let view = pool_rotation_view(&policy, &sources, &accounts, &runtime, 100);
    assert_eq!(view.member_order, ["source:source", "account:account"]);
    assert_eq!(view.preset, Some(crate::RotationPreset::Recommended));
    assert_eq!(
        pool_rotation_view(&policy, &sources, &accounts, &[], 100)
            .member_order
            .len(),
        2
    );
}

#[test]
fn rotation_view_groups_waits_and_preserves_live_work_for_every_mode() {
    let accounts = ["weekly", "temporary", "ready", "unavailable", "disabled"].map(|id| {
        let mut account = account_summary(true, &["model"]);
        account.id = id.into();
        account.operational_status = match id {
            "weekly" | "temporary" => OperationalStatus::QuotaWait,
            "unavailable" => OperationalStatus::Unavailable,
            "disabled" => OperationalStatus::Disabled,
            _ => OperationalStatus::Rotation,
        };
        account.quota.updated_at_ms = Some(100);
        account.refresh_state.quota = RefreshStatus::Fresh;
        if id == "weekly" {
            account.quota.secondary = Some(QuotaWindow {
                kind: QuotaWindowKind::Secondary,
                available_basis_points: Some(0),
                reset_at_ms: Some(200),
                window_minutes: Some(10_080),
                provider_cycle_id: None,
                window_start_ms: None,
                explicitly_full: None,
                observed_at_ms: 100,
                full_transition_fingerprint: None,
                exhaustion_transition_fingerprint: None,
            });
        }
        account
    });
    for mode in [
        crate::PoolRoutingMode::Automatic,
        crate::PoolRoutingMode::RoundRobin,
        crate::PoolRoutingMode::WeightedRoundRobin,
        crate::PoolRoutingMode::Failover,
    ] {
        let mut policy = pool_routing_summary(None, &[], &accounts);
        policy.mode = mode;
        let mut runtime = accounts
            .iter()
            .map(|account| {
                let mut candidate = runtime_candidate(
                    &account.id,
                    CandidateKind::OAuthAccount,
                    account.id == "ready",
                );
                if account.operational_status == OperationalStatus::QuotaWait {
                    candidate.quota_wait = true;
                    candidate.next_retry_at_ms =
                        Some(if account.id == "weekly" { 200 } else { 500 });
                }
                candidate
            })
            .collect::<Vec<_>>();
        assert_eq!(
            pool_rotation_view(&policy, &[], &accounts, &runtime, 100).member_order,
            [
                "account:ready",
                "account:temporary",
                "account:weekly",
                "account:unavailable",
                "account:disabled"
            ]
        );
        runtime[0].active_request_count = 1;
        assert_eq!(
            pool_rotation_view(&policy, &[], &accounts, &runtime, 100).member_order[0],
            "account:weekly"
        );
        runtime[0].active_request_count = 0;
        assert_eq!(
            pool_rotation_view(
                &policy,
                &[],
                &accounts,
                &runtime,
                100 + crate::QUOTA_STALE_AFTER_MS + 1
            )
            .member_order[1],
            "account:weekly"
        );
    }
}

#[test]
fn rotation_view_respects_backend_order_without_reinterpreting_counters() {
    let sources = [source_summary("a", &["one"]), source_summary("b", &["two"])];
    let policy = pool_routing_summary(None, &sources, &[]);
    let runtime = [
        runtime_candidate("b::responses", CandidateKind::ApiSource, true),
        runtime_candidate("a", CandidateKind::ApiSource, true),
    ];
    assert_eq!(
        pool_rotation_view(&policy, &sources, &[], &runtime, 100).member_order,
        ["source:b", "source:a"]
    );
    assert_eq!(runtime[0].selection_rank, None);
    assert!(!runtime[0].next_for_new_request);
}
