use super::*;

#[tokio::test]
async fn principal_scope_revocation_between_reserve_and_dispatch_spends_no_generation() {
    let runtime = runtime();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let lease = reserve(&runtime, &budget, WireApi::Responses).await;
    runtime.update_key_scope(
        "key",
        CandidateScope {
            source_ids: Some(["source-b".into()].into()),
            ..CandidateScope::default()
        },
    );
    assert_eq!(
        lease.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|budget| budget.wire_attempts()), 0);
    assert!(budget.attempted_members().is_empty());
    drop(lease);
    let permitted = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(permitted.member_key, "source:source-b");
    permitted.begin_rotation_http_dispatch().unwrap();
    assert_eq!(budget.with_budget(|budget| budget.wire_attempts()), 1);
    permitted.settle_rotation_success(crate::unix_time_ms());
}

#[tokio::test]
async fn principal_scope_revision_revokes_an_aba_edit_but_not_an_unchanged_save() {
    let runtime = runtime();
    let key = key(&runtime);
    let original = key.scope_snapshot();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let reserve = |budget: SharedRequestBudget| {
        let runtime = runtime.clone();
        let key = key.clone();
        async move {
            runtime
                .select_and_reserve_with_budget(
                    &key,
                    "model-a",
                    &[WireApi::Responses],
                    &HashSet::new(),
                    (None, None),
                    crate::unix_time_ms(),
                    &budget,
                )
                .await
                .unwrap()
                .1
        }
    };
    let pending = reserve(budget.clone()).await;
    assert_eq!(pending.candidate_id(), "source-a");
    assert!(runtime.update_key_scope("key", original.clone()));
    assert!(runtime.update_key_scope(
        "key",
        CandidateScope {
            source_ids: Some(["source-b".into()].into()),
            ..CandidateScope::default()
        },
    ));
    assert!(runtime.update_key_scope("key", original));
    assert_eq!(
        pending.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|b| b.wire_attempts()), 0);
    drop(pending);

    let current = reserve(budget.clone()).await;
    assert_eq!(current.candidate_id(), "source-a");
    assert!(runtime.update_key_scope("key", key.scope_snapshot()));
    current.begin_rotation_http_dispatch().unwrap();
    current.settle_rotation_success(crate::unix_time_ms());
    assert_eq!(budget.dispatches(), 1);
}

#[tokio::test]
async fn host_membership_and_key_scope_update_revoke_pending_dispatch_together() {
    let runtime = runtime();
    let key = key(&runtime);
    let old_scope = key.scope_snapshot();
    let mut policy = PoolRoutingPolicy {
        mode: PoolRoutingMode::InOrder,
        members: vec![PoolRoutingMember {
            kind: PoolMemberKind::Source,
            id: "source-b".into(),
            weight: 1,
            max_concurrency: 4,
        }],
        ..PoolRoutingPolicy::default()
    };
    let next_scope = CandidateScope {
        source_ids: Some(["source-b".into()].into()),
        ..CandidateScope::default()
    };
    // A missing internal key cannot leave the new policy partially applied.
    assert!(!runtime
        .set_pool_routing_policy_with_key_scopes(
            policy.clone(),
            2,
            &[("missing".into(), next_scope.clone())],
        )
        .unwrap());
    assert_eq!(runtime.request_dispatch_budget(), 3);
    assert_eq!(key.scope_snapshot(), old_scope);

    let budget = SharedRequestBudget::for_incoming_request(3);
    let pending = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(pending.candidate_id(), "source-a");
    assert!(runtime
        .set_pool_routing_policy_with_key_scopes(
            policy.clone(),
            2,
            &[("key".into(), next_scope.clone())],
        )
        .unwrap());
    assert_eq!(key.scope_snapshot(), next_scope);
    assert_eq!(
        pending.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|b| b.wire_attempts()), 0);
    drop(pending);

    let current = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(current.candidate_id(), "source-b");
    policy.members[0].weight = 2;
    assert!(runtime
        .set_pool_routing_policy_with_key_scopes(policy, 2, &[("key".into(), next_scope)])
        .unwrap());
    current.begin_rotation_http_dispatch().unwrap();
    current.settle_rotation_success(crate::unix_time_ms());
    assert_eq!(budget.dispatches(), 1);
}

#[tokio::test]
async fn candidate_permission_revision_revokes_an_aba_policy_edit_but_not_weight_only() {
    let runtime = runtime();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let pending = reserve(&runtime, &budget, WireApi::Responses).await;
    let policy = |enabled, weight| RuntimeCandidatePolicy {
        enabled,
        draining: false,
        priority: 0,
        weight,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
    };
    assert!(runtime.update_source_policy("source-a", policy(false, 1), 0));
    assert!(runtime.update_source_policy("source-a", policy(true, 1), 0));
    assert_eq!(
        pending.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|b| b.wire_attempts()), 0);
    drop(pending);

    let current = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(current.candidate_id(), "source-a");
    assert!(runtime.update_source_policy("source-a", policy(true, 2), 0));
    current.begin_rotation_http_dispatch().unwrap();
    current.settle_rotation_success(crate::unix_time_ms());
    assert_eq!(budget.dispatches(), 1);
}
