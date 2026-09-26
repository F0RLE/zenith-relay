use super::*;

#[tokio::test(start_paused = true)]
async fn retry_window_starts_at_rejection_and_survives_transport_handoff() {
    let budget = SharedRequestBudget::for_incoming_request(8);
    budget.configure_retry_window(30_000, false);
    // Silent work is not an expired retry window.
    tokio::time::advance(std::time::Duration::from_secs(20 * 60)).await;
    assert!(budget.start_dispatch().is_some());
    budget.observe_rejection();
    let deadline = budget.retry_wait_deadline(30_000);
    tokio::time::advance(std::time::Duration::from_secs(20)).await;
    let http_fallback = budget.clone();
    assert!(http_fallback.start_dispatch().is_some());
    http_fallback.observe_rejection();
    assert_eq!(http_fallback.retry_wait_deadline(30_000), deadline);
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    assert!(!budget.can_dispatch());
    assert!(http_fallback.start_wire_attempt().is_none());
}

#[tokio::test(start_paused = true)]
async fn persistent_wait_changes_neither_execution_budget_nor_original_window_start() {
    let budget = SharedRequestBudget::for_incoming_request(2);
    budget.configure_retry_window(1_000, false);
    assert!(budget.start_dispatch().is_some());
    budget.observe_rejection();
    budget.configure_retry_window(1_000, true);
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    assert!(budget.can_dispatch());
    budget.configure_retry_window(1_000, false);
    assert!(!budget.can_dispatch());
    budget.configure_retry_window(1_000, true);
    assert!(budget.start_dispatch().is_some());
    assert!(!budget.can_dispatch());
}

#[test]
fn shared_request_budget_cannot_restart_after_unknown_or_commit() {
    for observation in [
        ExecutionObservation::unknown(),
        ExecutionObservation::accepted(),
        ExecutionObservation::committed(),
    ] {
        let budget = SharedRequestBudget::for_incoming_request(8);
        let fallback = budget.clone();
        assert!(budget.start_dispatch().is_some());
        budget.with_budget(|budget| budget.observe_execution(observation));
        fallback.with_budget(|budget| budget.observe_execution(ExecutionObservation::not_sent()));
        assert!(!fallback.can_dispatch());
        assert_eq!(fallback.start_dispatch(), None);
        fallback.configure_retry_window(30_000, true);
        assert_eq!(fallback.start_dispatch(), None);
        assert_eq!(fallback.start_wire_attempt(), None);
        assert_eq!(fallback.dispatches(), 1);
    }
}
