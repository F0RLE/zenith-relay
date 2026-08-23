use super::{state::now_ms, store::telemetry_db::TelemetryDb};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use zenith_relay_core::{ResponseAffinityBinding, ResponseAffinityStore};

pub(crate) struct DesktopResponseAffinityStore {
    telemetry: Arc<TelemetryDb>,
    failed_writes: Arc<AtomicU64>,
}

impl DesktopResponseAffinityStore {
    pub(crate) fn new(telemetry: Arc<TelemetryDb>, failed_writes: Arc<AtomicU64>) -> Self {
        Self {
            telemetry,
            failed_writes,
        }
    }

    fn finish<T>(&self, result: super::error::Result<T>) -> Result<T, String> {
        result.map_err(|error| {
            self.failed_writes.fetch_add(1, Ordering::Relaxed);
            error.to_string()
        })
    }
}

impl ResponseAffinityStore for DesktopResponseAffinityStore {
    fn load(&self, now_ms: u64) -> Result<Vec<ResponseAffinityBinding>, String> {
        self.finish(self.telemetry.affinity_bindings(now_ms))
    }

    fn find(&self, key: &str, now_ms: u64) -> Result<Option<ResponseAffinityBinding>, String> {
        self.finish(self.telemetry.find_affinity(key, now_ms))
    }

    fn upsert(&self, binding: &ResponseAffinityBinding) -> Result<(), String> {
        self.finish(self.telemetry.upsert_affinity(binding, now_ms()))
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.finish(self.telemetry.delete_affinity(key))
    }

    fn delete_candidate(&self, candidate_id: &str) -> Result<(), String> {
        self.finish(self.telemetry.delete_candidate_affinities(candidate_id))
    }
}
