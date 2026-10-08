mod adapters;
mod catalog_warning;
mod coordination;
mod paths;
mod snapshot;

pub(crate) use adapters::DesktopOAuthEvents;
use coordination::wake_coordinator;
pub(crate) use paths::migrate_storage_layout;
#[cfg(test)]
use snapshot::{account_secret_available, SecretLookup};
pub(crate) use snapshot::{AccountCredentialFacts, LocalRuntimeInputs, SnapshotInputs};

use super::{
    accounts::{
        import_session::ImportSessionStore, oauth_flow::OAuthFlowManager, NativeSecretBackend,
    },
    error::{ErrorCode, LocalPoolError, Result},
    host::GatewayManager,
    profiles::repair,
    store::{telemetry_db::TelemetryDb, LocalPoolStore},
};
use crate::storage_paths::StoragePaths;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::AtomicU64, Arc, Mutex, MutexGuard},
};
use tokio::sync::{watch, Mutex as AsyncMutex, Notify};
use url::Url;
use zenith_relay_core::{
    accounts::TokenAuthority,
    automations::WakeCoordinator,
    model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogLoader},
    pricing::{CatalogStatus, PricingCatalog, PricingCatalogLoader},
    scheduler::refresh::{service::RefreshService, RefreshLimits},
};

pub(super) use zenith_relay_core::unix_time_ms as now_ms;

#[cfg(test)]
use zenith_relay_core::DefaultServiceTier;

const DEFAULT_CODEX_ACCOUNT_CHECK_ENDPOINT: &str =
    "https://chatgpt.com/backend-api/wham/accounts/check";

#[derive(Clone)]
pub struct DesktopState {
    pub(super) owner: Arc<DesktopStateOwner>,
}

impl std::ops::Deref for DesktopState {
    type Target = DesktopStateOwner;
    fn deref(&self) -> &Self::Target {
        &self.owner
    }
}

// Shared ownership lets refresh jobs keep only a Weak reference while idle.
// The Tauri-managed state remains the sole long-lived lifetime owner.
pub struct DesktopStateOwner {
    pub(crate) root: PathBuf,
    pub(crate) gateway: GatewayManager,
    pub(crate) telemetry: Arc<TelemetryDb>,
    pricing: Arc<PricingCatalogLoader>,
    model_metadata: Arc<ModelMetadataCatalogLoader>,
    store: Arc<Mutex<LocalPoolStore>>,
    token_authority: Arc<TokenAuthority>,
    pub(super) refresh: Arc<RefreshService<super::refresh::RefreshReadResult>>,
    pub(super) refresh_started: std::sync::atomic::AtomicBool,
    wake: Arc<Mutex<WakeCoordinator>>,
    wake_notify: Arc<Notify>,
    oauth_flow: OAuthFlowManager<NativeSecretBackend, DesktopOAuthEvents>,
    pub(super) oauth_events: DesktopOAuthEvents,
    failed_usage_writes: Arc<AtomicU64>,
    failed_affinity_writes: Arc<AtomicU64>,
    catalog_refresh_error: Arc<Mutex<Option<String>>>,
    background_session_active: watch::Sender<bool>,
    quota_account_locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
    subscription_refresh_lock: AsyncMutex<()>,
    setup_lock: tokio::sync::Mutex<()>,
    account_check_url: Url,
    credential_cache: Mutex<snapshot::CredentialCache>,
}

impl DesktopState {
    pub fn open(root: PathBuf) -> Result<Self> {
        let paths = StoragePaths::from_root(&root);
        let transient_root = paths.cache_root();
        let history_repair_root = paths.history_repair_backup_root();
        let _ = std::thread::Builder::new()
            .name("transient-cleanup".to_string())
            .spawn(move || {
                if let Err(error) =
                    ImportSessionStore::new(transient_root.clone(), NativeSecretBackend)
                        .cleanup_expired()
                {
                    crate::diagnostics::record_error(
                        "startup-cleanup",
                        Some("import_sessions_cleanup_failed"),
                        &error.to_string(),
                        &[],
                    );
                }
                if let Err(error) = repair::cleanup_expired_previews(&transient_root) {
                    crate::diagnostics::record_error(
                        "startup-cleanup",
                        Some("repair_previews_cleanup_failed"),
                        &error,
                        &[],
                    );
                }
                if let Err(error) = repair::cleanup_history_repair_backups(&history_repair_root) {
                    crate::diagnostics::record_error(
                        "startup-cleanup",
                        Some("repair_backups_cleanup_failed"),
                        &error,
                        &[],
                    );
                }
            });
        let mut store = LocalPoolStore::open(root.clone())?;
        let telemetry = store.database();
        let pricing = Arc::new(
            PricingCatalogLoader::open(paths.pricing_catalog_file())
                .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?,
        );
        let model_metadata = Arc::new(
            ModelMetadataCatalogLoader::open(paths.model_metadata_catalog_file())
                .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?,
        );
        let wake = wake_coordinator(store.automations())?;
        if &store.automations().state != wake.state() {
            let mut automations = store.automations().clone();
            automations.state = wake.state().clone();
            store.replace_automations(automations)?;
        }
        let failed_usage_writes = Arc::new(AtomicU64::new(0));
        let failed_affinity_writes = Arc::new(AtomicU64::new(0));
        let catalog_refresh_error =
            Arc::new(Mutex::new(store.gateway().catalog_refresh_error.clone()));
        // The native process owns automatic account work. A tray-only startup
        // or a closed WebView must not pause quota, credit, or catalog refresh.
        let (background_session_active, _) = watch::channel(true);
        let token_authority = Arc::new(
            TokenAuthority::new(crate::local_pool::models::MAX_LOCAL_ACCOUNTS)
                .map_err(LocalPoolError::invalid_state)?,
        );
        let oauth_events = DesktopOAuthEvents::default();
        let oauth_flow = OAuthFlowManager::new(
            paths.cache_root(),
            NativeSecretBackend,
            oauth_events.clone(),
        );
        Ok(Self {
            owner: Arc::new(DesktopStateOwner {
                root,
                gateway: GatewayManager::default(),
                telemetry,
                pricing,
                model_metadata,
                store: Arc::new(Mutex::new(store)),
                token_authority,
                refresh: RefreshService::with_cache_policy(
                    RefreshLimits::default(),
                    super::refresh::cache_observation,
                )
                .map_err(LocalPoolError::invalid_state)?,
                refresh_started: std::sync::atomic::AtomicBool::new(false),
                wake: Arc::new(Mutex::new(wake)),
                wake_notify: Arc::new(Notify::new()),
                oauth_flow,
                oauth_events,
                failed_usage_writes,
                failed_affinity_writes,
                catalog_refresh_error,
                background_session_active,
                quota_account_locks: Arc::new(Mutex::new(HashMap::new())),
                subscription_refresh_lock: AsyncMutex::new(()),
                setup_lock: tokio::sync::Mutex::new(()),
                account_check_url: Url::parse(DEFAULT_CODEX_ACCOUNT_CHECK_ENDPOINT)
                    .expect("the built-in account-check endpoint must be valid"),
                credential_cache: Mutex::new(snapshot::CredentialCache::default()),
            }),
        })
    }

    pub fn store(&self) -> Result<MutexGuard<'_, LocalPoolStore>> {
        self.store
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "local pool store lock poisoned"))
    }

    pub(crate) fn token_authority(&self) -> Arc<TokenAuthority> {
        self.token_authority.clone()
    }

    pub(crate) fn pricing_loader(&self) -> Arc<PricingCatalogLoader> {
        self.pricing.clone()
    }

    pub(crate) fn pricing_catalog(&self) -> Arc<PricingCatalog> {
        self.pricing.snapshot()
    }

    pub(crate) fn pricing_status(&self) -> CatalogStatus {
        self.pricing.status()
    }

    pub(crate) fn model_metadata_loader(&self) -> Arc<ModelMetadataCatalogLoader> {
        self.model_metadata.clone()
    }

    pub(crate) fn model_metadata_catalog(&self) -> Arc<ModelMetadataCatalog> {
        self.model_metadata.snapshot()
    }

    pub(crate) fn record_performance(
        &self,
        name: &str,
        duration_ms: f64,
        context: Option<&str>,
    ) -> Result<()> {
        self.telemetry
            .record_performance(name, duration_ms, context)
    }

    pub(crate) fn record_performance_async(
        &self,
        name: &str,
        duration_ms: f64,
        context: Option<&str>,
    ) {
        let telemetry = self.telemetry.clone();
        let operation_name = name.to_string();
        let context = context.map(str::to_owned);
        tauri::async_runtime::spawn_blocking(move || {
            let _ = telemetry.record_performance(&operation_name, duration_ms, context.as_deref());
        });
    }

    pub async fn setup_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.setup_lock.lock().await
    }

    pub(crate) fn account_check_url(&self) -> &Url {
        &self.account_check_url
    }

    #[cfg(test)]
    pub(crate) fn set_account_check_url_for_test(&mut self, endpoint: Url) {
        Arc::get_mut(&mut self.owner)
            .expect("test endpoint must be set before sharing state")
            .account_check_url = endpoint;
    }

    pub(crate) fn quota_account_lock(&self, account_id: &str) -> Result<Arc<AsyncMutex<()>>> {
        let mut locks = self
            .quota_account_locks
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "quota account lock poisoned"))?;
        Ok(locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone())
    }

    pub(crate) fn weekly_reset_was_applied(
        &self,
        account_id: &str,
        fingerprint: &str,
    ) -> Result<bool> {
        Ok(self
            .store()?
            .automations()
            .weekly_reset_fingerprints
            .get(account_id)
            .is_some_and(|applied| applied == fingerprint))
    }

    pub(crate) fn mark_weekly_reset_applied(
        &self,
        account_id: &str,
        fingerprint: &str,
    ) -> Result<()> {
        let mut store = self.store()?;
        let mut automations = store.automations().clone();
        automations
            .weekly_reset_fingerprints
            .insert(account_id.to_string(), fingerprint.to_string());
        store.replace_automations(automations)
    }

    pub(crate) fn remove_quota_account_lock(&self, account_id: &str) -> Result<bool> {
        let removed = self
            .quota_account_locks
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "quota account lock poisoned"))?
            .remove(account_id)
            .is_some();
        Ok(removed)
    }

    pub(crate) async fn subscription_refresh_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.subscription_refresh_lock.lock().await
    }

    pub(crate) fn set_background_session_active(&self, active: bool) {
        if *self.background_session_active.borrow() == active {
            return;
        }
        self.background_session_active.send_replace(active);
        if let Ok(store) = self.store() {
            store.notify_refresh_changed();
        }
    }

    pub(crate) fn background_session_active(&self) -> bool {
        *self.background_session_active.borrow()
    }

    pub(crate) async fn wait_for_background_session_active(&self) {
        let mut receiver = self.background_session_active.subscribe();
        loop {
            if *receiver.borrow() {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }

    pub(crate) async fn wait_for_background_session_inactive(&self) {
        let mut receiver = self.background_session_active.subscribe();
        loop {
            if !*receiver.borrow() {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
