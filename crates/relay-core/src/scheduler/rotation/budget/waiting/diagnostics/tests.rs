use super::*;
use tokio::time::advance;

#[tokio::test(start_paused = true)]
async fn waits_are_cumulative_across_clones_and_snapshots_do_not_double_count() {
    let budget = SharedRequestBudget::for_incoming_request(3);
    assert!(budget.wait_diagnostics().is_none());
    assert!(budget.begin_queue_wait(RequestWaitKind::Capacity));
    advance(Duration::from_millis(120)).await;
    let clone = budget.clone();
    let first = clone.wait_diagnostics().unwrap();
    assert_eq!(first.total_ms, 120);
    assert_eq!(
        first.capacity.unwrap(),
        WaitSummary {
            count: 1,
            duration_ms: 120
        }
    );
    assert!(!clone.begin_queue_wait(RequestWaitKind::PoolEvent));
    clone.finish_queue_wait();
    clone.finish_queue_wait();
    advance(Duration::from_secs(9)).await;
    assert_eq!(budget.wait_diagnostics().unwrap().total_ms, 120);
    assert!(clone.begin_queue_wait(RequestWaitKind::Recovery { delay_ms: 1_000 }));
    advance(Duration::from_millis(1_000)).await;
    let running = budget.wait_diagnostics().unwrap();
    assert_eq!(running.total_ms, 1_120);
    assert_eq!(running.recovery.as_ref().unwrap().duration_ms, 1_000);
    budget.finish_queue_wait();
    assert_eq!(clone.wait_diagnostics(), Some(running));
    assert!(budget.begin_queue_wait(RequestWaitKind::PoolEvent));
    advance(Duration::from_millis(50)).await;
    budget.finish_queue_wait();
    let final_snapshot = budget.wait_diagnostics().unwrap();
    assert_eq!(final_snapshot.total_ms, 1_170);
    assert_eq!(
        final_snapshot.pool_event.unwrap(),
        WaitSummary {
            count: 1,
            duration_ms: 50
        }
    );
    assert_eq!(final_snapshot.last_recovery_delay_ms, Some(1_000));
    assert_eq!(budget.dispatches(), 0);
    assert!(budget.can_dispatch());
}

#[tokio::test(start_paused = true)]
async fn zero_wait_and_queue_rejection_are_not_missing_measurements() {
    let budget = SharedRequestBudget::for_incoming_request(3);
    assert!(budget.begin_queue_wait(RequestWaitKind::Capacity));
    budget.finish_queue_wait();
    let snapshot = budget.wait_diagnostics().unwrap();
    assert_eq!(snapshot.total_ms, 0);
    assert_eq!(
        snapshot.capacity.unwrap(),
        WaitSummary {
            count: 1,
            duration_ms: 0
        }
    );

    let rejected = SharedRequestBudget::for_incoming_request(3);
    rejected.stop_admission(AdmissionStopReason::QueueFull);
    assert_eq!(
        serde_json::to_value(rejected.wait_diagnostics()).unwrap(),
        serde_json::json!({"totalMs": 0, "stopReason": "queue_full"})
    );
}

#[tokio::test(start_paused = true)]
async fn counters_stay_bounded_and_serialization_contains_only_timing_and_enums() {
    let budget = SharedRequestBudget::for_incoming_request(3);
    for _ in 0..1_000 {
        assert!(budget.begin_queue_wait(RequestWaitKind::PoolEvent));
        budget.finish_queue_wait();
    }
    budget.stop_admission(AdmissionStopReason::WaitExpired);
    let encoded = serde_json::to_value(budget.wait_diagnostics()).unwrap();
    assert_eq!(
        encoded,
        serde_json::json!({
            "totalMs": 0, "poolEvent": {"count": 1_000, "durationMs": 0},
            "stopReason": "wait_expired"
        })
    );
    let decoded: RequestWaitDiagnostics = serde_json::from_value(encoded).unwrap();
    assert_eq!(budget.wait_diagnostics(), Some(decoded));
}

#[test]
fn legacy_request_timing_round_trips_without_fabricated_waits() {
    let legacy = serde_json::json!({"durationMs": 10, "ttftMs": null, "projectionOnly": false});
    let decoded: crate::scheduler::RequestTiming = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
}
