use super::*;

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
    let next_owner_lease = reserve_owner(current_budget.clone()).await;
    assert_eq!(next_owner_lease.candidate_id(), "source-b");
    next_owner_lease.begin_rotation_http_dispatch().unwrap();
    next_owner_lease.settle_rotation_success(crate::unix_time_ms());
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
    assert_eq!(blocked.candidate_id(), "source-b");
    let blocked_id = blocked.candidate_id().to_owned();
    assert!(runtime.block_candidate_capability(&blocked_id, "model-a"));
    assert_eq!(
        blocked.begin_rotation_http_dispatch(),
        Err(RotationDispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|b| b.wire_attempts()), 0);
    drop(blocked);

    let alternative = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(alternative.candidate_id(), "source-a");
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
    let candidates_before_fence = runtime
        .candidate_runtime_order()
        .into_iter()
        .filter(|candidate| candidate.candidate_id.starts_with("source-a"))
        .collect::<Vec<_>>();
    assert!(candidates_before_fence.len() > 1);
    assert!(candidates_before_fence
        .iter()
        .all(|candidate| candidate.available));

    let fences = runtime.fence_source_dispatch("source-a");
    assert_eq!(fences.len(), candidates_before_fence.len());
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
    let initial_budget = SharedRequestBudget::for_incoming_request(3);
    let initial_lease = reserve(&runtime, &initial_budget, WireApi::Responses).await;
    let initial_candidate_id = initial_lease.candidate_id().to_owned();
    initial_lease.begin_rotation_dispatch().unwrap();
    {
        let mut scheduler = runtime.lock_scheduler();
        let original = scheduler.remove(initial_lease.candidate_id()).unwrap();
        scheduler.upsert(original);
    }
    let new_budget = SharedRequestBudget::for_incoming_request(3);
    let readded_candidate_lease = reserve_from(
        &runtime,
        &new_budget,
        WireApi::Responses,
        &initial_candidate_id,
    )
    .await;
    assert_eq!(
        initial_lease.candidate_id(),
        readded_candidate_lease.candidate_id()
    );
    runtime.settle_rotation_failure(
        &initial_lease,
        rejected(),
        Some(CooldownRequest {
            scope: "*",
            retry_at_ms: crate::unix_time_ms() + 60_000,
            reason: CooldownReason::Mandatory,
        }),
        crate::unix_time_ms(),
    );
    assert_eq!(
        runtime.failure_state_for(
            readded_candidate_lease.candidate_id(),
            "model-a",
            crate::unix_time_ms()
        ),
        (0, None)
    );
    assert!(readded_candidate_lease.begin_rotation_dispatch().is_ok());
    readded_candidate_lease.settle_rotation_success(crate::unix_time_ms());
}
