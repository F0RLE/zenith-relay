use super::{
    CatalogRefreshDeadline, CatalogRefreshKind, PricingCacheEnvelope, PricingCatalog,
    PricingCatalogHandle, PricingError, PRICING_REFRESH_INTERVAL_SECONDS,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};

pub const DEFAULT_CATALOG_MAX_AGE_MS: u64 = PRICING_REFRESH_INTERVAL_SECONDS * 1_000;
pub const MAX_CATALOG_RESPONSE_BYTES: usize = super::MAX_CACHE_BYTES;
const REFRESH_TIMEOUT: Duration = Duration::from_secs(20);
const FIRST_RETRY_DELAY_MS: u64 = 5 * 60 * 1_000;
const SECOND_RETRY_DELAY_MS: u64 = 30 * 60 * 1_000;
const SUBSEQUENT_RETRY_DELAY_MS: u64 = 2 * 60 * 60 * 1_000;

mod refresh;
mod store;
pub use store::PricingCacheStore;

/// The externally visible state of the local catalog. The state is
/// deliberately independent from the current snapshot: a stale snapshot is
/// still useful while a refresh is unavailable.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CatalogStatus {
    Current,
    Stale,
    Updating,
    #[default]
    Unloaded,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum CatalogRefreshOutcome {
    Updated { revision: String },
    NotModified { revision: String },
    Skipped,
}

/// Volatile retry state intentionally does not survive a restart. A stale
/// cache is safe to use immediately, and a fresh process gets one asynchronous
/// conditional validation instead of carrying forward an arbitrary delay.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RefreshRetryState {
    consecutive_failures: u32,
    next_retry_at_ms: Option<u64>,
}

impl RefreshRetryState {
    fn allows_attempt(self, now_ms: u64) -> bool {
        self.next_retry_at_ms
            .is_none_or(|retry_at| now_ms >= retry_at)
    }

    fn record_failure(&mut self, now_ms: u64) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.next_retry_at_ms =
            Some(now_ms.saturating_add(retry_delay_ms(self.consecutive_failures)));
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// A refreshable immutable catalog. Construction only reads the local cache;
/// callers explicitly schedule `refresh` on a background task so startup and
/// request handling never wait on the network.
#[derive(Clone)]
pub struct PricingCatalogLoader {
    store: PricingCacheStore,
    client: Client,
    handle: PricingCatalogHandle,
    envelope: Arc<RwLock<Option<PricingCacheEnvelope>>>,
    status: Arc<RwLock<CatalogStatus>>,
    last_error: Arc<RwLock<Option<PricingError>>>,
    retry_state: Arc<RwLock<RefreshRetryState>>,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    startup_refresh_pending: Arc<AtomicBool>,
    schedule_changed: Arc<tokio::sync::Notify>,
    max_age_ms: u64,
}

impl PricingCatalogLoader {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, PricingError> {
        Self::open_with_max_age(path, DEFAULT_CATALOG_MAX_AGE_MS)
    }

    pub fn open_with_max_age(
        path: impl Into<PathBuf>,
        max_age_ms: u64,
    ) -> Result<Self, PricingError> {
        let store = PricingCacheStore::new(path);
        let (catalog, envelope, initial_error, status) = match store.read_catalog() {
            Ok(Some((envelope, catalog))) => {
                let status = if envelope_is_stale(&envelope, now_ms(), max_age_ms) {
                    CatalogStatus::Stale
                } else {
                    CatalogStatus::Current
                };
                (catalog, Some(envelope), None, status)
            }
            Ok(None) => (PricingCatalog::empty(), None, None, CatalogStatus::Unloaded),
            Err(error) => (
                PricingCatalog::empty(),
                None,
                Some(error),
                CatalogStatus::Error,
            ),
        };
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(REFRESH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| PricingError::Network)?;
        Ok(Self {
            store,
            client,
            handle: catalog.handle(),
            envelope: Arc::new(RwLock::new(envelope)),
            status: Arc::new(RwLock::new(status)),
            last_error: Arc::new(RwLock::new(initial_error)),
            retry_state: Arc::new(RwLock::new(RefreshRetryState::default())),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            startup_refresh_pending: Arc::new(AtomicBool::new(true)),
            schedule_changed: Arc::new(tokio::sync::Notify::new()),
            max_age_ms,
        })
    }

    pub fn snapshot(&self) -> Arc<PricingCatalog> {
        self.handle.snapshot()
    }

    pub fn status(&self) -> CatalogStatus {
        *self.status.read().expect("pricing status lock poisoned")
    }

    pub fn last_error(&self) -> Option<PricingError> {
        *self.last_error.read().expect("pricing error lock poisoned")
    }

    pub fn cache_path(&self) -> &Path {
        self.store.path()
    }

    pub fn refresh_due(&self, now_ms: u64) -> bool {
        let catalog_due = self
            .envelope
            .read()
            .expect("pricing envelope lock poisoned")
            .as_ref()
            .is_none_or(|envelope| envelope_is_stale(envelope, now_ms, self.max_age_ms));
        let startup_due = self.startup_refresh_pending.load(Ordering::Acquire);
        (startup_due || catalog_due) && self.retry_allows_attempt(now_ms)
    }

    /// Returns the next reason and wall-clock deadline for a background
    /// refresh. A startup validation is due once per loader instance even when
    /// the persisted cache is still fresh; after that, the cache TTL controls
    /// normal checks and the retry deadline takes precedence after failures.
    pub fn next_refresh_deadline(&self, now_ms: u64) -> CatalogRefreshDeadline {
        if let Some(next_retry_at_ms) = self
            .retry_state
            .read()
            .expect("pricing retry lock poisoned")
            .next_retry_at_ms
        {
            return CatalogRefreshDeadline {
                at_ms: next_retry_at_ms,
                kind: CatalogRefreshKind::Retry,
            };
        }
        if self.startup_refresh_pending.load(Ordering::Acquire) {
            return CatalogRefreshDeadline {
                at_ms: now_ms,
                kind: CatalogRefreshKind::Startup,
            };
        }

        let envelope = self
            .envelope
            .read()
            .expect("pricing envelope lock poisoned");
        let Some(envelope) = envelope.as_ref() else {
            return CatalogRefreshDeadline {
                at_ms: now_ms,
                kind: CatalogRefreshKind::Scheduled,
            };
        };
        if envelope_is_stale(envelope, now_ms, self.max_age_ms) {
            return CatalogRefreshDeadline {
                at_ms: now_ms,
                kind: CatalogRefreshKind::Scheduled,
            };
        }
        CatalogRefreshDeadline {
            at_ms: envelope.fetched_at_ms.saturating_add(self.max_age_ms),
            kind: CatalogRefreshKind::Scheduled,
        }
    }

    /// Wakes a background scheduler when a manual refresh or a completed
    /// attempt changes the next refresh deadline.
    pub async fn wait_for_schedule_change(&self) {
        self.schedule_changed.notified().await;
    }

    pub fn spawn_refresh_if_due(
        self: &Arc<Self>,
        now_ms: u64,
    ) -> Option<tokio::task::JoinHandle<()>> {
        if !self.refresh_due(now_ms) {
            return None;
        }
        let loader = Arc::clone(self);
        tokio::runtime::Handle::try_current().ok().map(|runtime| {
            runtime.spawn(async move {
                let _ = loader.refresh(false).await;
            })
        })
    }

    fn set_status(&self, status: CatalogStatus, error: Option<PricingError>) {
        *self.status.write().expect("pricing status lock poisoned") = status;
        *self
            .last_error
            .write()
            .expect("pricing error lock poisoned") = error;
    }

    fn retry_allows_attempt(&self, now_ms: u64) -> bool {
        self.retry_state
            .read()
            .expect("pricing retry lock poisoned")
            .allows_attempt(now_ms)
    }

    fn record_refresh_failure(&self, now_ms: u64) {
        self.startup_refresh_pending.store(false, Ordering::Release);
        self.retry_state
            .write()
            .expect("pricing retry lock poisoned")
            .record_failure(now_ms);
    }

    fn record_refresh_success(&self) {
        self.startup_refresh_pending.store(false, Ordering::Release);
        self.retry_state
            .write()
            .expect("pricing retry lock poisoned")
            .reset();
        self.schedule_changed.notify_one();
    }
}

fn now_ms() -> u64 {
    crate::unix_time_ms()
}

fn is_stale(fetched_at_ms: u64, now_ms: u64, max_age_ms: u64) -> bool {
    fetched_at_ms == 0 || now_ms.saturating_sub(fetched_at_ms) >= max_age_ms
}

fn envelope_is_stale(envelope: &PricingCacheEnvelope, now_ms: u64, max_age_ms: u64) -> bool {
    envelope.stale || is_stale(envelope.fetched_at_ms, now_ms, max_age_ms)
}

const fn retry_delay_ms(consecutive_failures: u32) -> u64 {
    match consecutive_failures {
        0 => 0,
        1 => FIRST_RETRY_DELAY_MS,
        2 => SECOND_RETRY_DELAY_MS,
        _ => SUBSEQUENT_RETRY_DELAY_MS,
    }
}

#[cfg(test)]
mod tests;
