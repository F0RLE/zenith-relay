use crate::local_pool::{
    state::DesktopState,
    store::telemetry_db::{PerformanceSample, TelemetryDb},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Only fixed stage names and timings: no model, source, account or credential
/// data. Lock waits are subsets of stages, not additional projection work.
pub(super) struct SnapshotTiming {
    session: Option<crate::diagnostics::DebugSession>,
    started: Option<Instant>,
    last: Option<Instant>,
    stages: Vec<PerformanceSample<'static>>,
}

impl SnapshotTiming {
    pub(super) fn new(enabled: bool) -> Self {
        let started = enabled.then(Instant::now);
        Self {
            session: enabled
                .then(crate::diagnostics::DebugSession::capture)
                .flatten(),
            started,
            last: started,
            stages: if enabled {
                Vec::with_capacity(16)
            } else {
                Vec::new()
            },
        }
    }

    pub(super) fn mark(&mut self, stage: &'static str) {
        let Some(last) = self.last else { return };
        let now = Instant::now();
        self.stages.push(PerformanceSample {
            name: "snapshot_stage",
            duration_ms: now.duration_since(last).as_secs_f64() * 1_000.0,
            context: Some(stage),
        });
        self.last = Some(now);
    }

    pub(super) fn record_store_lock_wait(&mut self, duration: Duration) {
        if self.last.is_none() {
            return;
        }
        self.stages.push(PerformanceSample {
            name: "snapshot_lock_wait",
            duration_ms: duration.as_secs_f64() * 1_000.0,
            context: Some("store"),
        });
    }

    pub(super) fn finish(self, state: &DesktopState) {
        #[cfg(test)]
        super::performance_tests::print_stages(&self.stages);
        let Some(session) = self.session.filter(|session| session.is_current()) else {
            return;
        };
        let Some(started) = self.started else {
            return;
        };
        let elapsed = started.elapsed();
        let samples = self.into_samples(elapsed);
        // Total and stages share one worker and one durable commit, rather
        // than racing two writes for the same completed runtime snapshot.
        tauri::async_runtime::spawn_blocking(performance_batch_job(
            &state.telemetry,
            samples,
            move || session.is_current(),
        ));
    }

    fn into_samples(mut self, elapsed: Duration) -> Vec<PerformanceSample<'static>> {
        if elapsed < Duration::from_millis(250) {
            self.stages.retain(|sample| sample.name != "snapshot_stage");
        }
        self.stages.push(PerformanceSample {
            name: "full_snapshot_native",
            duration_ms: elapsed.as_secs_f64() * 1_000.0,
            context: Some("local"),
        });
        self.stages
    }
}

fn performance_batch_job(
    telemetry: &Arc<TelemetryDb>,
    samples: Vec<PerformanceSample<'static>>,
    collect: impl FnOnce() -> bool + Send + 'static,
) -> impl FnOnce() + Send + 'static {
    // Optional queued metrics must not extend the storage owner's lifetime.
    let telemetry = Arc::downgrade(telemetry);
    move || {
        if collect()
            && let Some(telemetry) = telemetry.upgrade()
        {
            let _ = telemetry.record_performance_batch(&samples);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_performance_batch_respects_collection_and_storage_lifetime() {
        let root = crate::files::test_temp_root("queued-performance-batch");
        let path = root.join("relay.sqlite");
        let database = Arc::new(TelemetryDb::open(&path).unwrap());
        let samples = vec![
            PerformanceSample {
                name: "full_snapshot_native",
                duration_ms: 300.0,
                context: Some("local"),
            },
            PerformanceSample {
                name: "snapshot_stage",
                duration_ms: 100.0,
                context: Some("inputs"),
            },
        ];
        performance_batch_job(&database, samples.clone(), || true)();
        performance_batch_job(&database, samples.clone(), || false)();
        let queued = performance_batch_job(&database, samples, || true);
        let lifetime = Arc::downgrade(&database);
        drop(database);
        let released = lifetime.upgrade().is_none();
        queued();

        let reader = rusqlite::Connection::open(&path).unwrap();
        let saved: (i64, i64) = reader
            .query_row(
                "SELECT SUM(name = 'full_snapshot_native'), SUM(name = 'snapshot_stage') \
                 FROM performance_samples",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        drop(reader);
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            released,
            "queued optional metrics must not retain the database"
        );
        assert_eq!(
            saved,
            (1, 1),
            "only the enabled batch on open storage is recorded"
        );
    }

    #[test]
    fn disabled_snapshot_timing_does_not_allocate_or_collect() {
        let mut timing = SnapshotTiming::new(false);
        timing.mark("inputs");
        timing.record_store_lock_wait(Duration::from_millis(10));
        assert!(timing.last.is_none());
        assert!(timing.stages.is_empty());
        assert_eq!(timing.stages.capacity(), 0);
    }

    #[test]
    fn fast_snapshot_keeps_exactly_one_total() {
        let mut timing = SnapshotTiming::new(true);
        timing.mark("inputs");
        let samples = timing.into_samples(Duration::from_micros(249_999));
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].name, "full_snapshot_native");
        assert_eq!(samples[0].context, Some("local"));
        assert!((samples[0].duration_ms - 249.999).abs() < 0.000_001);
    }

    #[test]
    fn store_lock_wait_is_retained_below_the_slow_snapshot_threshold() {
        for wait in [Duration::ZERO, Duration::from_micros(125)] {
            let mut timing = SnapshotTiming::new(true);
            timing.mark("inputs");
            timing.record_store_lock_wait(wait);
            let samples = timing.into_samples(Duration::from_millis(12));
            assert_eq!(samples.len(), 2);
            assert_eq!(samples[0].name, "snapshot_lock_wait");
            assert_eq!(samples[0].context, Some("store"));
            assert_eq!(samples[0].duration_ms, wait.as_secs_f64() * 1_000.0);
            assert_eq!(samples[1].name, "full_snapshot_native");
            assert_eq!(samples[1].duration_ms, 12.0);
        }
    }

    #[test]
    fn slow_snapshot_keeps_lock_wait_separate_from_inclusive_stages() {
        let mut timing = SnapshotTiming::new(true);
        timing.mark("inputs");
        timing.record_store_lock_wait(Duration::from_millis(100));
        let samples = timing.into_samples(Duration::from_millis(250));
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].name, "snapshot_stage");
        assert_eq!(samples[0].context, Some("inputs"));
        assert_eq!(samples[1].name, "snapshot_lock_wait");
        assert_eq!(samples[1].duration_ms, 100.0);
        assert_eq!(samples[2].name, "full_snapshot_native");
        assert_eq!(samples[2].duration_ms, 250.0);
    }

    #[test]
    fn slow_snapshot_keeps_all_stages_and_total_in_one_batch() {
        let mut timing = SnapshotTiming::new(true);
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
