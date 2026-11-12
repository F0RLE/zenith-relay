use super::*;

async fn rejected_owner(
    runtime: &GatewayRuntime,
    budget: &SharedRequestBudget,
    allow_fallback: bool,
) -> String {
    let lease = reserve(runtime, budget, WireApi::Responses).await;
    let owner = lease.candidate_id().to_owned();
    lease.begin_rotation_http_dispatch().unwrap();
    lease
        .settle_rotation(rejected(), crate::unix_time_ms())
        .unwrap();
    lease.prefer_transient_retry(allow_fallback, 30_000);
    owner
}

#[tokio::test(start_paused = true)]
async fn transient_retry_waits_for_its_owner_instead_of_advancing_rotation() {
    let runtime = runtime();
    runtime.set_route_recovery_enabled(true);
    let budget = SharedRequestBudget::for_incoming_request(4);
    let owner = rejected_owner(&runtime, &budget, true).await;
    let started = tokio::time::Instant::now();
    let lease = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(lease.candidate_id(), owner);
    assert!(started.elapsed() >= std::time::Duration::from_secs(1));
    assert_eq!(budget.dispatches(), 1, "waiting does not send a generation");
    assert!(budget.retry_candidate().is_none());
    let waiting = budget.wait_diagnostics().unwrap();
    assert!(waiting.recovery.unwrap().duration_ms >= 1_000);
    lease.begin_rotation_http_dispatch().unwrap();
    lease.settle_rotation_success(crate::unix_time_ms());
    assert_eq!(budget.dispatches(), 2);
}

#[tokio::test(start_paused = true)]
async fn disabling_wait_releases_a_soft_retry_but_not_a_nonportable_owner() {
    for allow_fallback in [true, false] {
        let runtime = runtime();
        runtime.set_route_recovery_enabled(true);
        let budget = SharedRequestBudget::for_incoming_request(4);
        let owner = rejected_owner(&runtime, &budget, allow_fallback).await;
        let pending_runtime = runtime.clone();
        let pending_budget = budget.clone();
        let pending = tokio::spawn(async move {
            pending_runtime
                .select_and_reserve_with_budget(
                    &key(&pending_runtime),
                    "model-a",
                    &[WireApi::Responses],
                    &HashSet::new(),
                    (None, None),
                    crate::unix_time_ms(),
                    &pending_budget,
                )
                .await
        });
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        runtime.set_route_recovery_enabled(false);
        let selected = pending.await.unwrap();
        if allow_fallback {
            assert_ne!(selected.unwrap().0.candidate_id, owner);
            assert!(budget.can_dispatch());
        } else {
            assert!(selected.is_none());
            assert!(!budget.can_dispatch());
        }
        assert_eq!(budget.dispatches(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn removed_retry_owner_does_not_hold_a_portable_request() {
    let runtime = runtime();
    runtime.set_route_recovery_enabled(true);
    let budget = SharedRequestBudget::for_incoming_request(4);
    let owner = rejected_owner(&runtime, &budget, true).await;
    runtime.lock_scheduler().remove(&owner).unwrap();
    let lease = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_ne!(lease.candidate_id(), owner);
    assert!(budget.retry_candidate().is_none());
    assert_eq!(budget.dispatches(), 1);
}

#[tokio::test(start_paused = true)]
async fn cancelling_a_retry_releases_its_waiter_without_another_dispatch() {
    let runtime = runtime();
    runtime.set_route_recovery_enabled(true);
    let budget = SharedRequestBudget::for_incoming_request(4);
    rejected_owner(&runtime, &budget, true).await;
    let pending_runtime = runtime.clone();
    let pending_budget = budget.clone();
    let pending = tokio::spawn(async move {
        reserve(&pending_runtime, &pending_budget, WireApi::Responses).await
    });
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());
    assert!(budget.wait_diagnostics().unwrap().recovery.is_some());
    pending.abort();
    assert!(pending.await.is_err_and(|error| error.is_cancelled()));
    assert_eq!(budget.dispatches(), 1);
    assert!(budget.begin_queue_wait(crate::scheduler::rotation::RequestWaitKind::Capacity));
    budget.finish_queue_wait();
}

#[tokio::test(start_paused = true)]
async fn a_rejected_handshake_keeps_its_owner_without_a_generation_debit() {
    let runtime = runtime();
    runtime.set_route_recovery_enabled(true);
    let budget = SharedRequestBudget::for_incoming_request(4);
    let lease = reserve(&runtime, &budget, WireApi::Responses).await;
    let owner = lease.candidate_id().to_owned();
    assert_eq!(budget.start_wire_attempt(), Some(1));
    lease
        .settle_rotation(rejected(), crate::unix_time_ms())
        .unwrap();
    lease.prefer_transient_retry(true, 30_000);
    drop(lease);
    let selected = reserve(&runtime, &budget, WireApi::Responses).await;
    assert_eq!(selected.candidate_id(), owner);
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|budget| budget.wire_attempts()), 1);
}

#[tokio::test(start_paused = true)]
async fn a_busy_retry_owner_waits_for_capacity_instead_of_using_the_backup() {
    let runtime = runtime();
    runtime.set_route_recovery_enabled(true);
    let budget = SharedRequestBudget::for_incoming_request(4);
    let owner = rejected_owner(&runtime, &budget, true).await;
    let mut occupying = Vec::new();
    for _ in 0..4 {
        occupying.push(
            reserve_from(
                &runtime,
                &SharedRequestBudget::for_incoming_request(1),
                WireApi::Responses,
                &owner,
            )
            .await,
        );
    }
    let pending_runtime = runtime.clone();
    let pending_budget = budget.clone();
    let pending = tokio::spawn(async move {
        reserve(&pending_runtime, &pending_budget, WireApi::Responses).await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());
    drop(occupying.pop());
    let selected = pending.await.unwrap();
    assert_eq!(selected.candidate_id(), owner);
    assert_eq!(budget.dispatches(), 1);
}
