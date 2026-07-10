use super::{
    error::{ErrorCode, LocalPoolError, Result},
    host::GatewayManager,
    models::{LocalPoolSnapshot, RuntimeTarget},
    store::{telemetry_db::TelemetryDb, LocalPoolStore},
};
use crate::platform;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
};
use zenith_relay_core::UsageCallback;

trait SecretLookup {
    fn load(&self, secret_ref: &str) -> Result<Option<String>>;
}

struct OsSecretLookup;

impl SecretLookup for OsSecretLookup {
    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        super::store::secret_store::load(secret_ref)
    }
}

pub struct DesktopState {
    pub(crate) root: PathBuf,
    pub(crate) gateway: GatewayManager,
    pub(crate) telemetry: Arc<TelemetryDb>,
    store: Arc<Mutex<LocalPoolStore>>,
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
            store: Arc::new(Mutex::new(store)),
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
        let store = self.store.clone();
        let failed = self.failed_usage_writes.clone();
        Arc::new(move |event| {
            // ponytail: synchronous local writes; add a bounded spool only if profiling shows contention.
            let recorded = telemetry.record(&event).is_ok();
            let touched = store
                .lock()
                .map_err(|_| ())
                .and_then(|mut store| {
                    store
                        .touch_usage(
                            &event.local_key_id,
                            &event.source_id,
                            chrono::Utc::now().to_rfc3339(),
                        )
                        .map_err(|_| ())
                })
                .is_ok();
            if !recorded || !touched {
                failed.fetch_add(1, Ordering::Relaxed);
            }
        })
    }

    pub async fn setup_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.setup_lock.lock().await
    }

    pub async fn snapshot(&self) -> Result<LocalPoolSnapshot> {
        self.snapshot_with(&OsSecretLookup).await
    }

    async fn snapshot_with(&self, secrets: &impl SecretLookup) -> Result<LocalPoolSnapshot> {
        let running = self.gateway.address().await.is_some();
        let (schema_version, gateway, sources, keys) = {
            let store = self.store()?;
            (
                store.metadata().schema_version,
                store.gateway().clone(),
                store.sources().to_vec(),
                store.keys().to_vec(),
            )
        };
        let mut warnings = Vec::new();
        if self.failed_usage_writes.load(Ordering::Relaxed) > 0 {
            warnings.push("usage_persistence_failed".to_string());
        }
        if gateway.enabled && !running {
            warnings.push("gateway_configured_but_not_running".to_string());
        }
        let mut sources_with_secrets = HashSet::new();
        for source in &sources {
            if secrets.load(&source.secret_ref)?.is_some() {
                sources_with_secrets.insert(source.id.as_str());
            } else {
                warnings.push(warning_code("source_secret_missing", &source.id));
            }
        }
        for key in &keys {
            let key_secret_available = secrets.load(&key.secret_ref)?.is_some();
            if !key_secret_available {
                warnings.push(warning_code("local_key_secret_missing", &key.id));
            }
            let usable = key.enabled
                && key_secret_available
                && sources.iter().any(|source| {
                    source.enabled
                        && !source.draining
                        && sources_with_secrets.contains(source.id.as_str())
                        && key
                            .source_ids
                            .as_ref()
                            .is_none_or(|ids| ids.iter().any(|id| id == &source.id))
                });
            if key.enabled && !usable {
                warnings.push(warning_code("local_key_unavailable", &key.id));
            }
        }
        Ok(LocalPoolSnapshot {
            schema_version,
            runtime_target: RuntimeTarget {
                kind: "local",
                connected: running,
            },
            gateway,
            platform: platform::platform_name(),
            capabilities: platform::capabilities(),
            sources,
            keys,
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

fn warning_code(code: &str, id: &str) -> String {
    let id = id.trim();
    let redacted = if id.chars().count() <= 12 {
        id.to_string()
    } else {
        format!("{}...", id.chars().take(8).collect::<String>())
    };
    format!("{code}:{redacted}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::models::{LocalGatewayKeyRecord, ProviderSourceRecord};
    use std::collections::HashMap;
    use zenith_relay_core::{UsageEvent, WireApi};

    #[test]
    fn usage_callback_persists_before_returning() {
        let root =
            std::env::temp_dir().join(format!("zenith-relay-state-{}", uuid::Uuid::new_v4()));
        let state = DesktopState::open(root.clone()).unwrap();
        state
            .store()
            .unwrap()
            .upsert_source(ProviderSourceRecord {
                id: "source_1".into(),
                name: "Synthetic".into(),
                enabled: true,
                draining: false,
                base_url: "https://example.test/v1".into(),
                secret_ref: "source:source_1".into(),
                wire_api: WireApi::Responses,
                models: vec!["gpt-test".into()],
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                priority: 0,
                weight: 1,
                last_used_at: None,
                last_test_at: None,
                last_test_status: None,
                last_error: None,
            })
            .unwrap();
        state
            .store()
            .unwrap()
            .upsert_key(LocalGatewayKeyRecord {
                id: "key_1".into(),
                label: "Default".into(),
                enabled: true,
                secret_ref: "key:key_1".into(),
                source_ids: None,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                model_prefix: None,
                created_at: "2026-07-10T00:00:00Z".into(),
                last_used_at: None,
            })
            .unwrap();

        (state.usage_callback())(UsageEvent {
            request_id: "req_callback".into(),
            attempt: 1,
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
        let store = state.store().unwrap();
        assert!(store.source("source_1").unwrap().last_used_at.is_some());
        assert!(store.key("key_1").unwrap().last_used_at.is_some());
        drop(store);
        drop(state);
        let reopened = LocalPoolStore::open(root.clone()).unwrap();
        assert!(reopened.source("source_1").unwrap().last_used_at.is_some());
        assert!(reopened.key("key_1").unwrap().last_used_at.is_some());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn setup_guard_serializes_mutations() {
        let root = std::env::temp_dir().join(format!("zenith-relay-lock-{}", uuid::Uuid::new_v4()));
        let state = Arc::new(DesktopState::open(root.clone()).unwrap());
        let first = state.setup_guard().await;
        let waiting_state = state.clone();
        let waiting = tokio::spawn(async move {
            let _guard = waiting_state.setup_guard().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        drop(first);
        waiting.await.unwrap();
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn snapshot_warns_and_filters_usability_when_secrets_are_missing() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-secret-state-{}",
            uuid::Uuid::new_v4()
        ));
        let state = DesktopState::open(root.clone()).unwrap();
        {
            let mut store = state.store().unwrap();
            store
                .replace_records(
                    vec![source_record("source_missing"), source_record("source_ok")],
                    vec![
                        key_record("key_missing", None),
                        key_record("key_scoped_missing", Some(vec!["source_missing".into()])),
                        key_record("key_ok", Some(vec!["source_ok".into()])),
                    ],
                )
                .unwrap();
        }
        let secrets = MemorySecrets(HashMap::from([
            ("source:source_ok".into(), "upstream".into()),
            ("key:key_scoped_missing".into(), "local".into()),
            ("key:key_ok".into(), "local".into()),
        ]));

        let snapshot = state.snapshot_with(&secrets).await.unwrap();
        assert!(snapshot
            .warnings
            .contains(&"source_secret_missing:source_m...".into()));
        assert!(snapshot
            .warnings
            .contains(&"local_key_secret_missing:key_missing".into()));
        assert!(snapshot
            .warnings
            .contains(&"local_key_unavailable:key_missing".into()));
        assert!(snapshot
            .warnings
            .contains(&"local_key_unavailable:key_scop...".into()));
        assert!(!snapshot
            .warnings
            .contains(&"local_key_unavailable:key_ok".into()));
        assert!(snapshot
            .warnings
            .iter()
            .all(|warning| !warning.contains("source_missing")
                && !warning.contains("key_scoped_missing")));
        assert!(snapshot
            .warnings
            .iter()
            .all(|warning| !warning.contains("source:") && !warning.contains("key:")));
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    struct MemorySecrets(HashMap<String, String>);

    impl SecretLookup for MemorySecrets {
        fn load(&self, secret_ref: &str) -> Result<Option<String>> {
            Ok(self.0.get(secret_ref).cloned())
        }
    }

    fn source_record(id: &str) -> ProviderSourceRecord {
        ProviderSourceRecord {
            id: id.into(),
            name: id.into(),
            enabled: true,
            draining: false,
            base_url: "https://example.test/v1".into(),
            secret_ref: format!("source:{id}"),
            wire_api: WireApi::Responses,
            models: vec!["gpt-test".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            last_used_at: None,
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        }
    }

    fn key_record(id: &str, source_ids: Option<Vec<String>>) -> LocalGatewayKeyRecord {
        LocalGatewayKeyRecord {
            id: id.into(),
            label: id.into(),
            enabled: true,
            secret_ref: format!("key:{id}"),
            source_ids,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
            created_at: "2026-07-10T00:00:00Z".into(),
            last_used_at: None,
        }
    }
}
