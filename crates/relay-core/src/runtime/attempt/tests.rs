use super::*;
use crate::scheduler::rotation::{AttemptObservation, ExecutionObservation, HealthObservation};
use crate::scheduler::CooldownReason;
use crate::{
    CandidateHealth, CandidateQuota, PoolMemberKind, PoolRoutingMember, PoolRoutingMode,
    PoolRoutingPolicy,
};

fn runtime() -> Arc<GatewayRuntime> {
    Arc::new(
        GatewayRuntime::from_pool(
            ["source-a", "source-b"]
                .into_iter()
                .map(|id| {
                    RuntimeSource::unrestricted(ProviderSource {
                        id: id.into(),
                        name: id.into(),
                        base_url: "https://example.test/v1".into(),
                        api_key: "synthetic-source-key".into(),
                        wire_api: WireApi::Responses,
                        models: vec!["model-a".into(), "model-b".into()],
                    })
                })
                .collect(),
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "key".into(),
                secret: "synthetic-local-key".into(),
            })],
            GatewayRuntimeOptions {
                pool_routing: Some(PoolRoutingPolicy {
                    mode: PoolRoutingMode::InOrder,
                    members: ["source-a", "source-b"]
                        .into_iter()
                        .map(|id| PoolRoutingMember {
                            kind: PoolMemberKind::Source,
                            id: id.into(),
                            weight: 1,
                            max_concurrency: 4,
                        })
                        .collect(),
                    ..PoolRoutingPolicy::default()
                }),
                ..GatewayRuntimeOptions::default()
            },
            Arc::new(|_| {}),
        )
        .unwrap(),
    )
}

fn key(runtime: &GatewayRuntime) -> AuthenticatedKey {
    runtime
        .authenticate(Some(&HeaderValue::from_static(
            "Bearer synthetic-local-key",
        )))
        .unwrap()
}

async fn reserve(
    runtime: &GatewayRuntime,
    budget: &SharedRequestBudget,
    protocol: WireApi,
) -> CandidateLease {
    runtime
        .select_and_reserve_with_budget(
            &key(runtime),
            "model-a",
            &[protocol],
            &HashSet::new(),
            (None, None),
            crate::unix_time_ms(),
            budget,
        )
        .await
        .unwrap()
        .1
}

fn rejected() -> AttemptObservation {
    AttemptObservation {
        execution: ExecutionObservation::not_sent(),
        health: HealthObservation::ClientError,
    }
}

#[tokio::test]
async fn cooldown_is_visible_to_release_observers_and_revokes_pending_dispatch() {
    let runtime = runtime();
    let first_budget = SharedRequestBudget::for_incoming_request(3);
    let pending_budget = SharedRequestBudget::for_incoming_request(3);
    let first = reserve(&runtime, &first_budget, WireApi::Responses).await;
    let pending = reserve(&runtime, &pending_budget, WireApi::Responses).await;
    assert_eq!(first.candidate_id(), pending.candidate_id());
    first.begin_rotation_dispatch().unwrap();
    let deadline = crate::unix_time_ms() + 60_000;
    let seen = Arc::new(AtomicBool::new(false));
    let observed = seen.clone();
    let weak = Arc::downgrade(&runtime);
    runtime.set_activity_callback(move |_| {
        let runtime = weak.upgrade().unwrap();
        let scheduler = runtime.lock_scheduler();
        for candidate in scheduler
            .candidates()
            .filter(|candidate| candidate.source_id == "source-a")
        {
            assert_eq!(
                candidate.cooldowns.get("model-a"),
                Some(&deadline),
                "{}",
                candidate.id
            );
            assert!(!candidate.cooldowns.contains_key("model-b"));
        }
        observed.store(true, Ordering::Release);
    });
    runtime.settle_rotation_failure(
        &first,
        rejected(),
        Some(CooldownRequest {
            scope: "model-a",
            retry_at_ms: deadline,
            reason: CooldownReason::RateLimit,
        }),
        crate::unix_time_ms(),
    );
    assert!(
        seen.load(Ordering::Acquire),
        "release callback must see the installed block"
    );
    assert_eq!(
        pending.begin_rotation_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(
        pending_budget.dispatches(),
        0,
        "lost dispatch race is not a generation"
    );
    assert!(pending_budget.attempted_members().is_empty());
    // A duplicate outcome cannot extend the block or vote again.
    runtime.settle_rotation_failure(
        &first,
        rejected(),
        Some(CooldownRequest {
            scope: "model-a",
            retry_at_ms: deadline + 60_000,
            reason: CooldownReason::RateLimit,
        }),
        crate::unix_time_ms(),
    );
    assert_eq!(
        runtime
            .failure_state_for(first.candidate_id(), "model-a", crate::unix_time_ms())
            .1,
        Some(("model-a".into(), deadline))
    );
    drop(pending);
    let alternative = reserve(&runtime, &first_budget, WireApi::Responses).await;
    assert_eq!(alternative.member_key, "source:source-b");
}

#[tokio::test]
async fn a_new_driver_keeps_physical_attempt_history_but_explicit_repair_can_retry_owner() {
    let runtime = runtime();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let first = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(first.member_key, "source:source-a");
    first.begin_rotation_dispatch().unwrap();
    first
        .settle_rotation(rejected(), crate::unix_time_ms())
        .unwrap();
    let handoff = budget.clone();
    let next = reserve(&runtime, &handoff, WireApi::ChatCompletions).await;
    assert_eq!(
        next.member_key, "source:source-b",
        "an alias of A is not an untried source"
    );
    drop(next);
    first.allow_rotation_repair();
    assert_eq!(handoff.dispatches(), 1, "repair never refunds a dispatch");
    let repair = reserve(&runtime, &handoff, WireApi::Responses).await;
    assert_eq!(repair.member_key, "source:source-a");
    repair.begin_rotation_dispatch().unwrap();
    repair.settle_rotation_unknown(crate::unix_time_ms());
    repair.allow_rotation_repair();
    handoff.begin_recovery_pass();
    assert!(
        !handoff.can_dispatch(),
        "neither repair nor recovery resets unknown execution"
    );
    assert_eq!(handoff.dispatches(), 2);
}

#[tokio::test]
async fn local_route_incompatibility_does_not_visit_or_exclude_its_member() {
    let runtime = runtime();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let first = reserve(&runtime, &budget, WireApi::Responses).await;
    let incompatible = HashSet::from([first.candidate_id().to_owned()]);
    drop(first);
    let (selection, alias) = runtime
        .select_and_reserve_with_budget(
            &key(&runtime),
            "model-a",
            &[WireApi::Responses, WireApi::ChatCompletions],
            &incompatible,
            (None, None),
            crate::unix_time_ms(),
            &budget,
        )
        .await
        .unwrap();
    assert!(!incompatible.contains(&selection.candidate_id));
    assert_eq!(alias.member_key, "source:source-a");
    assert_eq!(budget.dispatches(), 0);
    assert!(budget.attempted_members().is_empty());
}

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

#[tokio::test]
async fn changed_response_owner_revokes_pending_dispatch_without_spending_a_generation() {
    let runtime = runtime();
    let now = crate::unix_time_ms();
    let owner_key = "synthetic-response-owner";
    assert!(runtime
        .lock_scheduler()
        .bind_response_affinity(owner_key, "source-a", now));
    let key = key(&runtime);
    let reserve_owner = |budget: SharedRequestBudget| {
        let runtime = runtime.clone();
        let key = key.clone();
        async move {
            runtime
                .select_and_reserve_with_budget(
                    &key,
                    "model-a",
                    &[WireApi::Responses],
                    &HashSet::new(),
                    (Some(owner_key), None),
                    crate::unix_time_ms(),
                    &budget,
                )
                .await
                .unwrap()
                .1
        }
    };

    let invalidated_budget = SharedRequestBudget::for_incoming_request(3);
    let invalidated = reserve_owner(invalidated_budget.clone()).await;
    assert_eq!(invalidated.candidate_id(), "source-a");
    assert!(runtime
        .lock_scheduler()
        .invalidate_response_affinity(owner_key));
    assert_eq!(
        invalidated.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(invalidated_budget.dispatches(), 0);
    assert_eq!(invalidated_budget.with_budget(|b| b.wire_attempts()), 0);
    drop(invalidated);

    assert!(runtime.lock_scheduler().bind_response_affinity(
        owner_key,
        "source-a",
        crate::unix_time_ms()
    ));
    let replaced_budget = SharedRequestBudget::for_incoming_request(3);
    let replaced = reserve_owner(replaced_budget.clone()).await;
    assert!(runtime
        .lock_scheduler()
        .invalidate_response_affinity(owner_key));
    assert!(runtime.lock_scheduler().bind_response_affinity(
        owner_key,
        "source-a",
        crate::unix_time_ms()
    ));
    assert_eq!(
        replaced.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged),
        "the same candidate id is not the old owner revision"
    );
    assert_eq!(replaced_budget.dispatches(), 0);
    drop(replaced);

    let rebound_budget = SharedRequestBudget::for_incoming_request(3);
    let rebound = reserve_owner(rebound_budget.clone()).await;
    assert_eq!(rebound.candidate_id(), "source-a");
    assert!(runtime.lock_scheduler().bind_response_affinity(
        owner_key,
        "source-b",
        crate::unix_time_ms()
    ));
    assert_eq!(
        rebound.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(rebound_budget.dispatches(), 0);
    assert_eq!(rebound_budget.with_budget(|b| b.wire_attempts()), 0);
    assert!(rebound_budget.attempted_members().is_empty());
    drop(rebound);

    let current_budget = SharedRequestBudget::for_incoming_request(3);
    let current = reserve_owner(current_budget.clone()).await;
    assert_eq!(current.candidate_id(), "source-b");
    current.begin_rotation_http_dispatch().unwrap();
    current.settle_rotation_success(crate::unix_time_ms());
    assert_eq!(current_budget.dispatches(), 1);
}

#[tokio::test]
async fn new_execution_and_capability_fences_revoke_pending_leases_without_dispatch() {
    let runtime = runtime();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let pending = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(pending.candidate_id(), "source-a");
    let auth_fence = runtime.fence_execution("source-a").unwrap();
    assert_eq!(
        pending.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|b| b.wire_attempts()), 0);
    drop(pending);
    drop(auth_fence);

    let blocked = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(blocked.candidate_id(), "source-a");
    assert!(runtime.block_candidate_capability("source-a", "model-a"));
    assert_eq!(
        blocked.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|b| b.wire_attempts()), 0);
    drop(blocked);

    let alternative = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(alternative.candidate_id(), "source-b");
    alternative.begin_rotation_http_dispatch().unwrap();
    alternative.settle_rotation_success(crate::unix_time_ms());
    assert_eq!(budget.dispatches(), 1);
}

#[tokio::test]
async fn host_policy_fence_covers_the_commit_gap_and_rejects_an_old_lease_after_release() {
    let runtime = runtime();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let pending = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(pending.candidate_id(), "source-a");
    let fence = runtime.fence_candidate_dispatch("source-a").unwrap();
    assert_eq!(
        pending.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert!(runtime.update_source_policy(
        "source-a",
        RuntimeCandidatePolicy {
            enabled: false,
            draining: false,
            priority: 0,
            weight: 1,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
        },
        0,
    ));
    drop(fence);
    assert_eq!(
        pending.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|budget| budget.wire_attempts()), 0);
}

#[test]
fn source_host_fence_covers_every_protocol_candidate() {
    let runtime = runtime();
    let before = runtime
        .candidate_runtime_order()
        .into_iter()
        .filter(|candidate| candidate.candidate_id.starts_with("source-a"))
        .collect::<Vec<_>>();
    assert!(before.len() > 1);
    assert!(before.iter().all(|candidate| candidate.available));

    let fences = runtime.fence_source_dispatch("source-a");
    assert_eq!(fences.len(), before.len());
    let during = runtime.candidate_runtime_order();
    assert!(during
        .iter()
        .filter(|candidate| candidate.candidate_id.starts_with("source-a"))
        .all(|candidate| !candidate.available));
    assert!(during
        .iter()
        .any(|candidate| candidate.candidate_id.starts_with("source-b") && candidate.available));
    drop(fences);
    assert!(runtime
        .candidate_runtime_order()
        .iter()
        .filter(|candidate| candidate.candidate_id.starts_with("source-a"))
        .all(|candidate| candidate.available));
}

#[tokio::test]
async fn protected_quota_reserve_is_rechecked_at_final_dispatch() {
    let runtime = runtime();
    let now = crate::unix_time_ms();
    assert!(runtime.update_candidate_availability_at(
        "source-a",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Available(100),
        Some(now),
    ));
    assert!(runtime.set_protected_candidate(Some("source-a"), 50));
    let budget = SharedRequestBudget::for_incoming_request(3);
    let pending = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(pending.candidate_id(), "source-a");
    // Both observations still project as Available; only the protected
    // numeric reserve changed its permission to dispatch.
    assert!(runtime.update_candidate_availability_at(
        "source-a",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Available(40),
        Some(now + 1),
    ));
    assert_eq!(
        pending.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|b| b.wire_attempts()), 0);
    drop(pending);

    let alternative = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(alternative.candidate_id(), "source-b");
    alternative.begin_rotation_http_dispatch().unwrap();
    alternative.settle_rotation_success(crate::unix_time_ms());
}

#[tokio::test]
async fn replaced_runtime_rejects_pending_and_new_dispatch_without_losing_started_settlement() {
    let runtime = runtime();
    let pending_budget = SharedRequestBudget::for_incoming_request(3);
    let pending = reserve(&runtime, &pending_budget, WireApi::Responses).await;
    let started_budget = SharedRequestBudget::for_incoming_request(3);
    let started = reserve(&runtime, &started_budget, WireApi::Responses).await;
    started.begin_rotation_http_dispatch().unwrap();

    runtime.retire_for_replacement();
    assert_eq!(
        pending.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(pending_budget.dispatches(), 0);
    assert_eq!(
        pending_budget.with_budget(|budget| budget.wire_attempts()),
        0
    );
    assert!(pending_budget.attempted_members().is_empty());
    drop(pending);
    assert!(runtime
        .select_and_reserve_with_budget(
            &key(&runtime),
            "model-a",
            &[WireApi::Responses],
            &HashSet::new(),
            (None, None),
            crate::unix_time_ms(),
            &pending_budget,
        )
        .await
        .is_none());
    started.settle_rotation_success(crate::unix_time_ms());
    assert_eq!(started_budget.dispatches(), 1);
    assert!(runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| candidate.active_request_count == 0 && !candidate.available));
}

#[tokio::test]
async fn late_rejection_cannot_install_a_block_after_remove_and_same_id_readd() {
    let runtime = runtime();
    let old_budget = SharedRequestBudget::for_incoming_request(3);
    let old = reserve(&runtime, &old_budget, WireApi::Responses).await;
    old.begin_rotation_dispatch().unwrap();
    {
        let mut scheduler = runtime.lock_scheduler();
        let original = scheduler.remove(old.candidate_id()).unwrap();
        scheduler.upsert(original);
    }
    let new_budget = SharedRequestBudget::for_incoming_request(3);
    let current = reserve(&runtime, &new_budget, WireApi::Responses).await;
    assert_eq!(old.candidate_id(), current.candidate_id());
    runtime.settle_rotation_failure(
        &old,
        rejected(),
        Some(CooldownRequest {
            scope: "*",
            retry_at_ms: crate::unix_time_ms() + 60_000,
            reason: CooldownReason::Mandatory,
        }),
        crate::unix_time_ms(),
    );
    assert_eq!(
        runtime.failure_state_for(current.candidate_id(), "model-a", crate::unix_time_ms()),
        (0, None)
    );
    assert!(current.begin_rotation_dispatch().is_ok());
    current.settle_rotation_success(crate::unix_time_ms());
}
