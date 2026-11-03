use super::*;

#[test]
fn quantiles_require_measured_samples_and_keep_missing_ttft_separate() {
    let mut window = LatencyWindow::default();
    assert_eq!(window.snapshot(100).state, RouteLatencyState::Unknown);
    for value in 1..20 {
        window.observe(Some(value), 100);
    }
    window.observe(None, 100);
    let cold = window.snapshot(100);
    assert_eq!((cold.sample_count, cold.unknown_count), (19, 1));
    assert_eq!(cold.state, RouteLatencyState::Insufficient);
    assert_eq!((cold.p50_ms, cold.p90_ms, cold.p95_ms), (None, None, None));
    window.observe(Some(20), 100);
    let ready = window.snapshot(100);
    assert_eq!(ready.state, RouteLatencyState::Ready);
    assert_eq!(
        (ready.p50_ms, ready.p90_ms, ready.p95_ms),
        (Some(10), Some(18), Some(19))
    );
    assert_eq!((ready.sample_count, ready.unknown_count), (20, 1));
    assert_eq!(ready.last_observed_at_ms, Some(100));
}

#[test]
fn latency_window_bounds_retention_and_expires_without_refreshing_on_reads() {
    let mut window = LatencyWindow::default();
    for value in 1..=100 {
        window.observe(Some(value), 100);
    }
    let snapshot = window.snapshot(100 + WINDOW_MS);
    assert_eq!(snapshot.sample_count, MAX_SAMPLES as u32);
    assert_eq!(
        (snapshot.p50_ms, snapshot.p90_ms, snapshot.p95_ms),
        (Some(68), Some(94), Some(97))
    );
    assert_eq!(snapshot.last_observed_at_ms, Some(100));
    let expired = window.snapshot(101 + WINDOW_MS);
    assert_eq!(expired.state, RouteLatencyState::Stale);
    assert_eq!((expired.sample_count, expired.p95_ms), (0, None));
    assert_eq!(expired.last_observed_at_ms, Some(100));
    window.observe(None, 101 + WINDOW_MS);
    let unknown = window.snapshot(101 + WINDOW_MS);
    assert_eq!(unknown.state, RouteLatencyState::Unknown);
    assert_eq!((unknown.sample_count, unknown.unknown_count), (0, 1));
}

#[test]
fn clock_rollback_and_measured_zero_remain_distinct() {
    let mut window = LatencyWindow::default();
    for _ in 0..MIN_MEASURED_SAMPLES {
        window.observe(Some(0), 100);
    }
    let future = window.snapshot(99);
    assert_eq!(future.state, RouteLatencyState::Stale);
    assert_eq!(future.last_observed_at_ms, None);
    let current = window.snapshot(100);
    assert_eq!(current.p95_ms, Some(0));
    window.observe(Some(5), 99);
    let reset = window.snapshot(99);
    assert_eq!(reset.sample_count, 1);
    assert_eq!(reset.state, RouteLatencyState::Insufficient);
}
