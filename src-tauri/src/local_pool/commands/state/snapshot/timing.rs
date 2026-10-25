use crate::local_pool::{state::DesktopState, store::telemetry_db::PerformanceSample};
use std::time::{Duration, Instant};

/// Only fixed stage names and timings: no model, source, account or credential
/// data. Fast snapshots retain the existing single total sample.
pub(super) struct SnapshotTiming {
    last: Instant,
    stages: Vec<PerformanceSample<'static>>,
}

impl SnapshotTiming {
    pub(super) fn new() -> Self {
        Self {
            last: Instant::now(),
            stages: Vec::with_capacity(16),
        }
    }

    pub(super) fn mark(&mut self, stage: &'static str) {
        let now = Instant::now();
        self.stages.push(PerformanceSample {
            name: "snapshot_stage",
            duration_ms: now.duration_since(self.last).as_secs_f64() * 1_000.0,
            context: Some(stage),
        });
        self.last = now;
    }

    pub(super) fn finish(self, state: &DesktopState, elapsed: Duration) {
        let samples = self.into_samples(elapsed);
        if let [sample] = samples.as_slice() {
            state.record_performance_async(sample.name, sample.duration_ms, sample.context);
            return;
        }
        let telemetry = state.telemetry.clone();
        // Total and stages share one worker and one durable commit, rather
        // than racing two writes for the same completed runtime snapshot.
        tauri::async_runtime::spawn_blocking(move || {
            let _ = telemetry.record_performance_batch(&samples);
        });
    }

    fn into_samples(mut self, elapsed: Duration) -> Vec<PerformanceSample<'static>> {
        if elapsed < Duration::from_millis(250) {
            self.stages.clear();
        }
        self.stages.push(PerformanceSample {
            name: "full_snapshot_native",
            duration_ms: elapsed.as_secs_f64() * 1_000.0,
            context: Some("local"),
        });
        self.stages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_snapshot_keeps_exactly_one_total() {
        let mut timing = SnapshotTiming::new();
        timing.mark("inputs");
        let samples = timing.into_samples(Duration::from_micros(249_999));
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].name, "full_snapshot_native");
        assert_eq!(samples[0].context, Some("local"));
        assert!((samples[0].duration_ms - 249.999).abs() < 0.000_001);
    }

    #[test]
    fn slow_snapshot_keeps_all_stages_and_total_in_one_batch() {
        let mut timing = SnapshotTiming::new();
        timing.mark("inputs");
        timing.mark("catalog_projection");
        let stage_times: Vec<_> = timing
            .stages
            .iter()
            .map(|sample| sample.duration_ms)
            .collect();
        let samples = timing.into_samples(Duration::from_millis(250));
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].context, Some("inputs"));
        assert_eq!(samples[1].context, Some("catalog_projection"));
        for (sample, duration) in samples[..2].iter().zip(stage_times) {
            assert_eq!(sample.name, "snapshot_stage");
            assert_eq!(sample.duration_ms, duration);
        }
        assert_eq!(samples[2].name, "full_snapshot_native");
        assert_eq!(samples[2].context, Some("local"));
        assert_eq!(samples[2].duration_ms, 250.0);
    }
}
