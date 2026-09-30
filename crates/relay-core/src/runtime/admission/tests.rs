use super::*;
use futures_util::poll;
use tokio::time::{advance, Instant};

mod gateway;
mod load;

fn runtime() -> GatewayRuntime {
    let mut policy = crate::resolve_pool_routing(
        Some(&crate::PoolRoutingPolicy::default()),
        ["a", "b"]
            .map(|id| (crate::PoolMemberKind::Source, id.into(), 0, 1))
            .into(),
    );
    for member in &mut policy.members {
        member.max_concurrency = 1;
    }
    GatewayRuntime::from_pool(
        ["a", "b"]
            .map(|id| {
                RuntimeSource::unrestricted(ProviderSource {
                    id: id.into(),
                    name: id.into(),
                    base_url: "https://example.test/v1".into(),
                    api_key: "synthetic".into(),
                    wire_api: WireApi::Responses,
                    models: if id == "a" {
                        vec![format!("model-{id}"), "gpt-image-2".into()]
                    } else {
                        vec![format!("model-{id}")]
                    },
                })
            })
            .into(),
        ["one", "two", "three"]
            .map(|id| {
                RuntimeLocalKey::unrestricted(LocalGatewayKey {
                    id: id.into(),
                    secret: format!("synthetic-{id}"),
                })
            })
            .into(),
        GatewayRuntimeOptions {
            pool_routing: Some(policy),
            ..Default::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap()
}

fn request(runtime: &GatewayRuntime, principal: usize, model: &str) -> AdmissionRequest {
    AdmissionRequest {
        key: runtime.authenticated_key(&runtime.keys[principal]),
        model: model.into(),
        protocols: vec![WireApi::Responses],
        tried: HashSet::new(),
        response_affinity: None,
        prompt_affinity: None,
        operation: RotationOperation::Text,
        budget: SharedRequestBudget::for_incoming_request(3),
    }
}

fn queued(runtime: &GatewayRuntime) -> (usize, usize) {
    let queue = runtime.admission.lock().unwrap();
    (queue.waiters.len(), queue.retained_bytes)
}

#[tokio::test(start_paused = true)]
async fn retiring_a_runtime_wakes_queued_admissions_without_spending_dispatch() {
    let runtime = runtime();
    let now = crate::unix_time_ms();
    let (_, held) = runtime
        .admit(request(&runtime, 0, "model-a"), now)
        .await
        .unwrap();
    let pending = request(&runtime, 0, "model-a");
    let mut waiting = Box::pin(runtime.admit(pending.clone(), now));
    assert!(poll!(&mut waiting).is_pending());
    assert_eq!(queued(&runtime).0, 1);
    runtime.retire_for_replacement();
    assert!(waiting.await.is_none());
    assert_eq!(queued(&runtime), (0, 0));
    assert_eq!(pending.budget.dispatches(), 0);
    drop(held);
}

#[tokio::test(start_paused = true)]
async fn retiring_a_runtime_stops_persistent_recovery_waits() {
    let runtime = runtime();
    let now = crate::unix_time_ms();
    let (_, held) = runtime
        .admit(request(&runtime, 0, "model-a"), now)
        .await
        .unwrap();
    let pending = request(&runtime, 0, "model-a");
    let mut waiting = Box::pin(recovery(&runtime, &pending, true));
    assert!(poll!(&mut waiting).is_pending());
    assert_eq!(queued(&runtime).0, 1);
    runtime.retire_for_replacement();
    assert!(!waiting.await);
    assert_eq!(queued(&runtime), (0, 0));
    assert_eq!(pending.budget.dispatches(), 0);
    drop(held);
}

async fn recovery(runtime: &GatewayRuntime, request: &AdmissionRequest, persistent: bool) -> bool {
    runtime
        .wait_for_recovery_event(
            &request.key,
            &request.model,
            &request.protocols,
            &request.tried,
            None,
            request.operation,
            &request.budget,
            None,
            persistent,
        )
        .await
}

#[tokio::test(start_paused = true)]
async fn principal_round_robin_is_independent_of_future_poll_order() {
    let runtime = runtime();
    let now = crate::unix_time_ms();
    let (_, held) = runtime
        .admit(request(&runtime, 0, "model-a"), now)
        .await
        .unwrap();
    let a = request(&runtime, 0, "model-a");
    let a2 = request(&runtime, 0, "model-a");
    let b = request(&runtime, 1, "model-a");
    let c = request(&runtime, 2, "model-a");
    let mut fa = Box::pin(runtime.admit(a.clone(), now));
    let mut fa2 = Box::pin(runtime.admit(a2, now));
    let mut fb = Box::pin(runtime.admit(b, now));
    let mut fc = Box::pin(runtime.admit(c, now));
    for future in [&mut fa, &mut fa2, &mut fb, &mut fc] {
        assert!(poll!(future).is_pending());
    }
    assert_eq!(queued(&runtime).0, 4);
    assert_eq!(a.budget.dispatches(), 0);
    drop(held);
    assert!(poll!(&mut fc).is_pending());
    assert!(poll!(&mut fb).is_pending());
    assert!(poll!(&mut fa2).is_pending());
    let (_, first) = fa.await.unwrap();
    drop(first);
    assert!(poll!(&mut fa2).is_pending());
    assert!(poll!(&mut fc).is_pending());
    let (_, second) = fb.await.unwrap();
    drop(second);
    assert!(poll!(&mut fa2).is_pending());
    let (_, third) = fc.await.unwrap();
    drop(third);
    drop(fa2.await.unwrap());
    assert_eq!(queued(&runtime), (0, 0));
}

#[tokio::test(start_paused = true)]
async fn incompatible_head_and_full_queue_do_not_block_free_independent_capacity() {
    let runtime = runtime();
    runtime.admission.lock().unwrap().limits.requests = 1;
    let now = crate::unix_time_ms();
    let (_, held) = runtime
        .admit(request(&runtime, 0, "model-a"), now)
        .await
        .unwrap();
    let mut blocked = Box::pin(runtime.admit(request(&runtime, 0, "model-a"), now));
    assert!(poll!(&mut blocked).is_pending());
    assert_eq!(queued(&runtime).0, 1);
    // No queue count/byte debit at all when independent capacity is free.
    let huge = request(&runtime, 1, "model-b");
    huge.budget.retain_input_bytes(usize::MAX);
    let (_, free) = runtime.admit(huge, now).await.unwrap();
    assert_eq!(free.candidate_id(), "b");
    let overflow = request(&runtime, 1, "model-a");
    assert!(runtime.admit(overflow.clone(), now).await.is_none());
    assert_eq!(
        overflow.budget.admission_stop_reason(),
        Some(AdmissionStopReason::QueueFull)
    );
    assert_eq!(overflow.budget.dispatches(), 0);
    drop(blocked);
    assert_eq!(queued(&runtime), (0, 0));
    drop((held, free));
    assert!(runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| candidate.active_request_count == 0));
}

#[tokio::test(start_paused = true)]
async fn fully_reserved_pool_rejects_an_unsupported_route_without_queuing() {
    let runtime = runtime();
    let now = crate::unix_time_ms();
    let (_, a) = runtime
        .admit(request(&runtime, 0, "model-a"), now)
        .await
        .unwrap();
    let (_, b) = runtime
        .admit(request(&runtime, 1, "model-b"), now)
        .await
        .unwrap();
    assert!(runtime.lock_scheduler().all_capacity_reserved());
    let unsupported = request(&runtime, 2, "model-missing");
    assert!(runtime.admit(unsupported.clone(), now).await.is_none());
    assert_eq!(unsupported.budget.admission_stop_reason(), None);
    assert_eq!(queued(&runtime), (0, 0));
    drop((a, b));
}

#[tokio::test(start_paused = true)]
async fn principal_limits_and_retained_bytes_are_released_synchronously_on_cancel() {
    let runtime = runtime();
    runtime.admission.lock().unwrap().limits.principal_requests = 1;
    let now = crate::unix_time_ms();
    let (_, held) = runtime
        .admit(request(&runtime, 0, "model-a"), now)
        .await
        .unwrap();
    let one = request(&runtime, 0, "model-a");
    one.budget.retain_input_bytes(100);
    let charge = one.retained_bytes();
    let mut first = Box::pin(runtime.admit(one.clone(), now));
    assert!(poll!(&mut first).is_pending());
    assert_eq!(queued(&runtime), (1, charge));
    assert!(runtime
        .admit(request(&runtime, 0, "model-a"), now)
        .await
        .is_none());
    let mut other = Box::pin(runtime.admit(request(&runtime, 1, "model-a"), now));
    assert!(poll!(&mut other).is_pending());
    drop(first);
    assert_eq!(queued(&runtime).0, 1);
    let mut replacement = Box::pin(runtime.admit(request(&runtime, 0, "model-a"), now));
    assert!(poll!(&mut replacement).is_pending());
    drop((replacement, other, held));
    assert_eq!(queued(&runtime), (0, 0));
    assert_eq!(one.budget.dispatches(), 0);
}

#[tokio::test(start_paused = true)]
async fn runtime_and_principal_bytes_are_checked_without_overflow() {
    let runtime = runtime();
    let now = crate::unix_time_ms();
    let (_, held) = runtime
        .admit(request(&runtime, 0, "model-a"), now)
        .await
        .unwrap();
    let one = request(&runtime, 0, "model-a");
    one.budget.retain_input_bytes(200);
    let charge = one.retained_bytes();
    {
        let mut queue = runtime.admission.lock().unwrap();
        queue.limits.bytes = charge * 2;
        queue.limits.principal_bytes = charge;
    }
    let mut waiting = Box::pin(runtime.admit(one, now));
    assert!(poll!(&mut waiting).is_pending());
    for principal in [0, 1] {
        let large = request(&runtime, principal, "model-a");
        large.budget.retain_input_bytes(usize::MAX);
        assert!(runtime.admit(large.clone(), now).await.is_none());
        assert_eq!(
            large.budget.admission_stop_reason(),
            Some(AdmissionStopReason::QueueFull)
        );
    }
    drop((waiting, held));
    assert_eq!(queued(&runtime), (0, 0));
}

#[tokio::test(start_paused = true)]
async fn accumulated_wait_survives_passes_and_budget_clones_without_new_thirty_seconds() {
    let runtime = runtime();
    let now = crate::unix_time_ms();
    let (_, held) = runtime
        .admit(request(&runtime, 0, "model-a"), now)
        .await
        .unwrap();
    let request = request(&runtime, 0, "model-a");
    let mut first = Box::pin(runtime.admit(request.clone(), now));
    assert!(poll!(&mut first).is_pending());
    advance(Duration::from_secs(20)).await;
    drop(held);
    let (_, lease) = first.await.unwrap();
    // Keep that lease busy, and carry the logical request across a new pass.
    let mut second = Box::pin(runtime.admit(request.clone(), now + 20_000));
    assert!(poll!(&mut second).is_pending());
    advance(Duration::from_secs(10)).await;
    assert!(second.await.is_none());
    assert_eq!(
        request.budget.admission_stop_reason(),
        Some(AdmissionStopReason::WaitExpired)
    );
    assert!(!request.budget.can_dispatch());
    assert_eq!(request.budget.dispatches(), 0);
    assert_eq!(queued(&runtime), (0, 0));
    drop(lease);
}

#[tokio::test(start_paused = true)]
async fn recovery_wait_is_bounded_and_an_unsupported_head_needs_events_not_polling() {
    let runtime = runtime();
    runtime.set_route_recovery_enabled(true);
    let request = request(&runtime, 0, "model-missing");
    request.budget.configure_retry_window(30_000, true);
    let mut waiting = Box::pin(recovery(&runtime, &request, true));
    assert!(poll!(&mut waiting).is_pending());
    assert_eq!(queued(&runtime).0, 1);
    advance(Duration::from_secs(120)).await;
    assert!(poll!(&mut waiting).is_pending());
    // An unrelated queue cancellation cannot manufacture a recovery event.
    runtime.admission_changed.notify_waiters();
    assert!(poll!(&mut waiting).is_pending());
    runtime.set_route_recovery_enabled(false);
    assert!(!waiting.await);
    assert_eq!(
        request.budget.admission_stop_reason(),
        Some(AdmissionStopReason::WaitExpired)
    );
    assert_eq!(queued(&runtime), (0, 0));
}

#[tokio::test(start_paused = true)]
async fn recovery_wait_and_capacity_share_limits_and_elapsed_time() {
    let runtime = runtime();
    runtime.admission.lock().unwrap().limits.requests = 1;
    let request = request(&runtime, 0, "model-missing");
    let mut waiting = Box::pin(recovery(&runtime, &request, true));
    assert!(poll!(&mut waiting).is_pending());
    let other = super::tests::request(&runtime, 1, "model-missing");
    assert!(!recovery(&runtime, &other, true).await);
    assert_eq!(
        other.budget.admission_stop_reason(),
        Some(AdmissionStopReason::QueueFull)
    );
    advance(Duration::from_secs(30)).await;
    assert!(!waiting.await);
    assert_eq!(
        request.budget.admission_stop_reason(),
        Some(AdmissionStopReason::WaitExpired)
    );
    assert_eq!(queued(&runtime), (0, 0));
}

#[tokio::test(start_paused = true)]
async fn due_timer_and_release_before_await_are_not_lost() {
    let runtime = runtime();
    let request = request(&runtime, 0, "model-a");
    let now = crate::unix_time_ms();
    assert!(runtime.set_cooldown_with_reason_for_model_at(
        "a",
        CooldownRequest {
            scope: "model-a",
            retry_at_ms: now + 2_000,
            reason: crate::scheduler::CooldownReason::RateLimit,
        }
    ));
    let mut waiting = Box::pin(recovery(&runtime, &request, false));
    assert!(poll!(&mut waiting).is_pending());
    advance(Duration::from_secs(2)).await;
    assert!(waiting.await);
    assert_eq!(queued(&runtime), (0, 0));
    assert_eq!(request.budget.dispatches(), 0);
    let started = Instant::now();
    // A due time already elapsed is a ready retry, never a new timeout.
    assert!(
        runtime
            .wait_for_recovery_event(
                &request.key,
                "model-b",
                &request.protocols,
                &request.tried,
                None,
                request.operation,
                &request.budget,
                None,
                false
            )
            .await
    );
    assert_eq!(started, Instant::now());
}

#[tokio::test(start_paused = true)]
async fn auth_fence_release_and_capability_changes_wake_event_only_recovery() {
    let runtime = runtime();
    runtime.set_route_recovery_enabled(true);
    let request = request(&runtime, 0, "model-a");
    request.budget.configure_retry_window(30_000, true);
    let fence = runtime.fence_execution("a").unwrap();
    let mut waiting = Box::pin(recovery(&runtime, &request, true));
    assert!(poll!(&mut waiting).is_pending());
    drop(fence);
    assert!(waiting.await);
    assert!(runtime.block_candidate_capability("a", "model-a"));
    let mut waiting = Box::pin(recovery(&runtime, &request, true));
    assert!(poll!(&mut waiting).is_pending());
    assert!(runtime.clear_candidate_capability_blocks("a"));
    assert!(waiting.await);
    assert_eq!(queued(&runtime), (0, 0));
    assert_eq!(request.budget.dispatches(), 0);
}
