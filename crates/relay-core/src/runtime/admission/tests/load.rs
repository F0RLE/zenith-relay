//! Synthetic contention through the real reservation queue, without provider I/O.

use super::*;
use futures_util::{poll, stream::FuturesUnordered, StreamExt};

#[tokio::test(start_paused = true)]
async fn mixed_text_and_image_waiters_remain_bounded_and_principal_fair() {
    run_mixed_waiters(64).await;
}

/// An opt-in, reproducible high-contention check near the per-principal cap.
/// It is intentionally excluded from the ordinary suite's execution time.
#[tokio::test(start_paused = true)]
#[ignore = "run explicitly to measure high-contention admission"]
async fn large_mixed_queue_reaches_per_principal_limit_without_starvation() {
    run_mixed_waiters(256).await;
}

async fn run_mixed_waiters(per_principal: usize) {
    let runtime = runtime();
    let now = crate::unix_time_ms();
    let (_, held_a) = runtime
        .admit(request(&runtime, 0, "model-a"), now)
        .await
        .unwrap();
    let (_, held_b) = runtime
        .admit(request(&runtime, 1, "model-b"), now)
        .await
        .unwrap();

    let mut pending = FuturesUnordered::new();
    let mut expected_retained = 0usize;
    let started = std::time::Instant::now();
    for sequence in 0..per_principal {
        for principal in 0..3 {
            let (model, operation) = match sequence % 3 {
                0 => ("model-a", RotationOperation::Text),
                1 => ("model-b", RotationOperation::Text),
                _ => ("gpt-image-2", RotationOperation::Image),
            };
            let waiter = AdmissionRequest {
                operation,
                ..request(&runtime, principal, model)
            };
            waiter.budget.retain_input_bytes(8 * 1024);
            expected_retained += waiter.retained_bytes();
            let runtime = &runtime;
            pending.push(async move {
                let budget = waiter.budget.clone();
                let admitted = runtime.admit(waiter, now).await;
                (principal, budget, admitted)
            });
        }
    }
    assert!(poll!(pending.next()).is_pending());
    let enqueue_elapsed = started.elapsed();
    assert_eq!(queued(&runtime), (3 * per_principal, expected_retained));
    assert!(expected_retained < AdmissionLimits::default().bytes);
    drop((held_a, held_b));

    let started = std::time::Instant::now();
    let mut served = [0usize; 3];
    while let Some((principal, budget, admitted)) = pending.next().await {
        let (_, lease) = admitted.expect("a queued, compatible route remains available");
        served[principal] += 1;
        // Each principal had waiters from the start. The queue may select the
        // other free physical route, but cannot starve any one principal.
        if served.iter().sum::<usize>() >= 12 {
            let range = served.iter().max().unwrap() - served.iter().min().unwrap();
            assert!(
                range <= 3,
                "principal service skew exceeded three: {served:?}"
            );
        }
        lease.begin_rotation_http_dispatch().unwrap();
        lease.settle_rotation_success(now);
        assert_eq!(budget.dispatches(), 1);
        assert_eq!(budget.with_budget(|budget| budget.wire_attempts()), 1);
    }
    eprintln!(
        "served {} queued requests, peak retained {} bytes, enqueue {:?}, drain {:?}",
        3 * per_principal,
        expected_retained,
        enqueue_elapsed,
        started.elapsed()
    );
    assert_eq!(served, [per_principal; 3]);
    assert_eq!(queued(&runtime), (0, 0));
    assert!(runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| candidate.active_request_count == 0));
}
