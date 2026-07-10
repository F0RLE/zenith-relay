use super::{
    error::{ErrorCode, LocalPoolError, Result},
    host::GatewayManager,
    models::{LocalPoolSnapshot, RuntimeTarget},
    store::{telemetry_db::TelemetryDb, LocalPoolStore},
};
use crate::platform;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
};
use zenith_relay_core::UsageCallback;

pub struct DesktopState {
    pub(crate) root: PathBuf,
    pub(crate) gateway: GatewayManager,
    pub(crate) telemetry: Arc<TelemetryDb>,
    store: Mutex<LocalPoolStore>,
    failed_usage_writes: Arc<AtomicU64>,
    setup_lock: tokio::sync::Mutex<()>,
}

impl DesktopState {
    pub fn open(root: PathBuf) -> Result<Self> {
        let store = LocalPoolStore::open(root.clone())?;
        let telemetry = Arc::new(TelemetryDb::open(
            &root.join("telemetry").join("usage.sqlite"),
        )?);
        let failed_usage_writes = Arc::new(AtomicU64::new(0));
        Ok(Self {
            root,
            gateway: GatewayManager::default(),
            telemetry,
            store: Mutex::new(store),
            failed_usage_writes,
            setup_lock: tokio::sync::Mutex::new(()),
        })
    }

    pub fn store(&self) -> Result<MutexGuard<'_, LocalPoolStore>> {
        self.store
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "local pool store lock poisoned"))
    }

    pub fn usage_callback(&self) -> UsageCallback {
        let telemetry = self.telemetry.clone();
        let failed = self.failed_usage_writes.clone();
        Arc::new(move |event| {
            // ponytail: synchronous local SQLite write; add a bounded spool only if profiling shows contention.
            if telemetry.record(&event).is_err() {
                failed.fetch_add(1, Ordering::Relaxed);
            }
        })
    }

    pub async fn setup_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.setup_lock.lock().await
    }

    pub async fn snapshot(&self) -> Result<LocalPoolSnapshot> {
        let running = self.gateway.address().await.is_some();
        let store = self.store()?;
        let mut warnings = Vec::new();
        if self.failed_usage_writes.load(Ordering::Relaxed) > 0 {
            warnings.push("usage_persistence_failed".to_string());
        }
        if store.gateway().enabled && !running {
            warnings.push("gateway_configured_but_not_running".to_string());
        }
        Ok(LocalPoolSnapshot {
            schema_version: store.metadata().schema_version,
            runtime_target: RuntimeTarget {
                kind: "local",
                connected: running,
            },
            gateway: store.gateway().clone(),
            platform: platform::platform_name(),
            capabilities: platform::capabilities(),
            sources: store.sources().to_vec(),
            keys: store.keys().to_vec(),
            warnings,
        })
    }

    pub fn profile_backup_root(&self) -> PathBuf {
        self.root.join("backups").join("profiles")
    }

    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::{UsageEvent, WireApi};

    #[test]
    fn usage_callback_persists_before_returning() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-state-{}", uuid::Uuid::new_v4()));
        let state = DesktopState::open(root.clone()).unwrap();

        (state.usage_callback())(UsageEvent {
            request_id: "req_callback".into(),
            local_key_id: "key_1".into(),
            source_id: "source_1".into(),
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            wire_api: WireApi::Responses,
            success: true,
            http_status: 200,
            error_category: None,
            latency_ms: 7,
            ttft_ms: None,
            input_tokens: Some(2),
            output_tokens: Some(3),
            total_tokens: Some(5),
        });

        let logs = state.telemetry.list(10).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].request_id, "req_callback");
        assert_eq!(logs[0].total_tokens, Some(5));
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
