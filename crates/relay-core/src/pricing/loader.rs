use super::{
    payload_hash, CatalogRefreshDeadline, CatalogRefreshKind, PricingCacheEnvelope, PricingCatalog,
    PricingCatalogHandle, PricingError, PRICING_REFRESH_INTERVAL_SECONDS,
};
use reqwest::{header, Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const DEFAULT_CATALOG_MAX_AGE_MS: u64 = PRICING_REFRESH_INTERVAL_SECONDS * 1_000;
pub const MAX_CATALOG_RESPONSE_BYTES: usize = super::MAX_CACHE_BYTES;
const REFRESH_TIMEOUT: Duration = Duration::from_secs(20);
const FIRST_RETRY_DELAY_MS: u64 = 5 * 60 * 1_000;
const SECOND_RETRY_DELAY_MS: u64 = 30 * 60 * 1_000;
const SUBSEQUENT_RETRY_DELAY_MS: u64 = 2 * 60 * 60 * 1_000;

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

/// Small synchronous persistence boundary shared by the desktop and server
/// loaders. It never parses untrusted data without validating the complete
/// envelope first and replaces the target only after the temporary file is
/// flushed successfully.
#[derive(Clone, Debug)]
pub struct PricingCacheStore {
    path: PathBuf,
}

impl PricingCacheStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> Result<Option<PricingCacheEnvelope>, PricingError> {
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(PricingError::Io),
        };
        if metadata.len() > u64::try_from(MAX_CATALOG_RESPONSE_BYTES).unwrap_or(u64::MAX) {
            return Err(PricingError::CacheTooLarge);
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        File::open(&self.path)
            .map_err(|_| PricingError::Io)?
            .read_to_end(&mut bytes)
            .map_err(|_| PricingError::Io)?;
        let envelope = serde_json::from_slice::<PricingCacheEnvelope>(&bytes)
            .map_err(|_| PricingError::InvalidCache)?;
        envelope.validate()?;
        Ok(Some(envelope))
    }

    pub fn read_catalog(
        &self,
    ) -> Result<Option<(PricingCacheEnvelope, PricingCatalog)>, PricingError> {
        self.read()?
            .map(|envelope| envelope.catalog().map(|catalog| (envelope, catalog)))
            .transpose()
    }

    pub fn write(&self, envelope: &PricingCacheEnvelope) -> Result<(), PricingError> {
        envelope.validate()?;
        let parent = self.path.parent().ok_or(PricingError::Io)?;
        fs::create_dir_all(parent).map_err(|_| PricingError::Io)?;
        let suffix = unique_suffix();
        let temp_path = self.path.with_extension(format!("tmp-{suffix}"));
        let bytes = serde_json::to_vec(envelope).map_err(|_| PricingError::InvalidCache)?;
        if bytes.len() > MAX_CATALOG_RESPONSE_BYTES {
            return Err(PricingError::CacheTooLarge);
        }
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp_path)
                .map_err(|_| PricingError::Io)?;
            file.write_all(&bytes).map_err(|_| PricingError::Io)?;
            file.sync_all().map_err(|_| PricingError::Io)?;
            replace_file(&temp_path, &self.path)
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        write_result
    }

    /// Persist an envelope only when its serialized value changed.  A refresh
    /// can legitimately keep the same payload while changing validators or
    /// freshness metadata, so equality is checked on the complete validated
    /// envelope rather than on the payload hash alone.
    pub fn write_if_changed(&self, envelope: &PricingCacheEnvelope) -> Result<bool, PricingError> {
        envelope.validate()?;
        if self.read().ok().flatten().as_ref() == Some(envelope) {
            return Ok(false);
        }
        self.write(envelope)?;
        Ok(true)
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

    pub async fn refresh(&self, force: bool) -> Result<CatalogRefreshOutcome, PricingError> {
        if !force && !self.refresh_due(now_ms()) {
            return Ok(CatalogRefreshOutcome::Skipped);
        }
        let _guard = self.refresh_lock.lock().await;
        if !force && !self.refresh_due(now_ms()) {
            return Ok(CatalogRefreshOutcome::Skipped);
        }
        self.set_status(CatalogStatus::Updating, None);
        let mut request = self.client.get(super::LITELLM_SOURCE_URL);
        let current_envelope = self
            .envelope
            .read()
            .expect("pricing envelope lock poisoned")
            .clone();
        if let Some(envelope) = current_envelope.as_ref() {
            if let Some(etag) = envelope.etag.as_deref() {
                request = request.header(header::IF_NONE_MATCH, etag);
            }
            if let Some(last_modified) = envelope.last_modified.as_deref() {
                request = request.header(header::IF_MODIFIED_SINCE, last_modified);
            }
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(_) => return self.refresh_failed(PricingError::Network),
        };
        if response.status() == StatusCode::NOT_MODIFIED {
            return self.accept_not_modified(response).await;
        }
        if response.status() != StatusCode::OK {
            return self.refresh_failed(PricingError::HttpStatus(response.status().as_u16()));
        }
        let response_headers = response.headers().clone();
        let payload = match collect_json(response).await {
            Ok(payload) => payload,
            Err(error) => return self.refresh_failed(error),
        };
        let payload_sha256 = match payload_hash(&payload) {
            Ok(hash) => hash,
            Err(error) => return self.refresh_failed(error),
        };
        let fetched_at_ms = now_ms();
        let mut envelope = match PricingCacheEnvelope::new(payload, payload_sha256, fetched_at_ms) {
            Ok(envelope) => envelope,
            Err(error) => return self.refresh_failed(error),
        };
        envelope.etag = header_string(&response_headers, header::ETAG);
        envelope.last_modified = header_string(&response_headers, header::LAST_MODIFIED);
        envelope.stale = false;
        let catalog = match PricingCatalog::from_litellm_payload(
            &envelope.payload,
            Some(envelope.revision.clone()),
            Some(envelope.fetched_at_ms),
            false,
        ) {
            Ok(catalog) => catalog,
            Err(error) => return self.refresh_failed(error),
        };
        // Keep validators and freshness metadata current even when the
        // payload itself is unchanged. The serialized envelope is still
        // replaced only after complete parsing and validation.
        if let Err(error) = self.store.write_if_changed(&envelope) {
            return self.refresh_failed(error);
        }
        self.handle.replace(catalog);
        *self
            .envelope
            .write()
            .expect("pricing envelope lock poisoned") = Some(envelope.clone());
        self.set_status(CatalogStatus::Current, None);
        self.record_refresh_success();
        Ok(CatalogRefreshOutcome::Updated {
            revision: envelope.revision,
        })
    }

    async fn accept_not_modified(
        &self,
        response: reqwest::Response,
    ) -> Result<CatalogRefreshOutcome, PricingError> {
        let current_envelope = self
            .envelope
            .read()
            .expect("pricing envelope lock poisoned")
            .clone();
        let Some(mut current) = current_envelope else {
            return self.refresh_failed(PricingError::InvalidCache);
        };
        if let Some(etag) = response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
        {
            current.etag = Some(etag.to_string());
        }
        if let Some(last_modified) = response
            .headers()
            .get(header::LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
        {
            current.last_modified = Some(last_modified.to_string());
        }
        current.fetched_at_ms = now_ms();
        current.stale = false;
        let catalog = match current.catalog() {
            Ok(catalog) => catalog,
            Err(error) => return self.refresh_failed(error),
        };
        if let Err(error) = self.store.write_if_changed(&current) {
            return self.refresh_failed(error);
        }
        self.handle.replace(catalog);
        *self
            .envelope
            .write()
            .expect("pricing envelope lock poisoned") = Some(current.clone());
        self.set_status(CatalogStatus::Current, None);
        self.record_refresh_success();
        Ok(CatalogRefreshOutcome::NotModified {
            revision: current.revision,
        })
    }

    fn refresh_failed<T>(&self, error: PricingError) -> Result<T, PricingError> {
        self.record_refresh_failure(now_ms());
        let current_envelope = self
            .envelope
            .read()
            .expect("pricing envelope lock poisoned")
            .clone();
        if let Some(envelope) = current_envelope {
            let mut stale = envelope;
            stale.stale = true;
            if let Ok(catalog) = stale.catalog() {
                self.handle.replace(catalog);
            }
            // Persist the marker so a restart does not mistake a catalog that
            // failed its last refresh for a current snapshot. The refresh
            // error remains the primary diagnostic even if this best-effort
            // write also fails.
            let _ = self.store.write_if_changed(&stale);
            *self
                .envelope
                .write()
                .expect("pricing envelope lock poisoned") = Some(stale);
            self.set_status(CatalogStatus::Stale, Some(error));
        } else {
            self.set_status(CatalogStatus::Error, Some(error));
        }
        // Wake the scheduler only after the retry deadline, stale marker, and
        // externally visible status have been updated as one logical result.
        // Otherwise it could observe the previous schedule and immediately
        // go back to sleep with stale state.
        self.schedule_changed.notify_one();
        Err(error)
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

async fn collect_json(mut response: reqwest::Response) -> Result<Value, PricingError> {
    if response.content_length().is_some_and(|length| {
        length > u64::try_from(MAX_CATALOG_RESPONSE_BYTES).unwrap_or(u64::MAX)
    }) {
        return Err(PricingError::CacheTooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| PricingError::Network)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_CATALOG_RESPONSE_BYTES {
            return Err(PricingError::CacheTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| PricingError::InvalidCatalog)
}

fn replace_file(temp: &Path, target: &Path) -> Result<(), PricingError> {
    if fs::rename(temp, target).is_ok() {
        return Ok(());
    }
    // Windows does not replace an existing file with rename. Keep a recoverable
    // backup while installing the fully synced temporary file.
    if target.exists() {
        let backup = target.with_extension(format!("bak-{}", unique_suffix()));
        fs::rename(target, &backup).map_err(|_| PricingError::Io)?;
        match fs::rename(temp, target) {
            Ok(()) => {
                let _ = fs::remove_file(backup);
                Ok(())
            }
            Err(_) => {
                let _ = fs::rename(&backup, target);
                Err(PricingError::Io)
            }
        }
    } else {
        Err(PricingError::Io)
    }
}

fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
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

// Kept private so the compiler catches accidental use of a removed response
// path; validators are read directly before body collection in `refresh`.
fn header_string(headers: &reqwest::header::HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn cache_store_round_trips_and_rejects_corrupt_data() {
        let directory = std::env::temp_dir().join(format!(
            "zenith-relay-pricing-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let store = PricingCacheStore::new(directory.join("litellm.json"));
        let envelope = PricingCacheEnvelope::new(
            json!({"gpt-test": {"input_cost_per_token": "0.000001", "output_cost_per_token": "0.000002"}}),
            "sha256:test".into(),
            1,
        )
        .unwrap();
        store.write(&envelope).unwrap();
        assert_eq!(store.read().unwrap(), Some(envelope));
        fs::write(store.path(), b"{}").unwrap();
        assert_eq!(store.read(), Err(PricingError::InvalidCache));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn stale_detection_is_monotonic() {
        assert!(!is_stale(100, 100, 10));
        assert!(is_stale(100, 110, 10));
        assert!(is_stale(0, 1, 10));
    }

    #[test]
    fn cache_store_skips_an_identical_envelope() {
        let directory = std::env::temp_dir().join(format!(
            "zenith-relay-pricing-unchanged-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let store = PricingCacheStore::new(directory.join("litellm.json"));
        let envelope = PricingCacheEnvelope::new(
            json!({
                "gpt-test": {
                    "input_cost_per_token": "0.000001",
                    "output_cost_per_token": "0.000002"
                }
            }),
            "sha256:test".into(),
            now_ms(),
        )
        .unwrap();
        assert!(store.write_if_changed(&envelope).unwrap());
        let modified = fs::metadata(store.path()).unwrap().modified().unwrap();
        assert!(!store.write_if_changed(&envelope).unwrap());
        assert_eq!(
            fs::metadata(store.path()).unwrap().modified().unwrap(),
            modified
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn explicit_stale_marker_forces_refresh_even_with_a_fresh_timestamp() {
        let payload = json!({
            "gpt-test": {
                "input_cost_per_token": "0.000001",
                "output_cost_per_token": "0.000002"
            }
        });
        let mut envelope =
            PricingCacheEnvelope::new(payload, "sha256:test".into(), now_ms()).unwrap();
        envelope.stale = true;
        assert!(envelope_is_stale(
            &envelope,
            now_ms(),
            DEFAULT_CATALOG_MAX_AGE_MS
        ));
    }

    #[test]
    fn refresh_failure_persists_stale_marker_for_the_next_startup() {
        let directory = std::env::temp_dir().join(format!(
            "zenith-relay-pricing-stale-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("litellm.json");
        let store = PricingCacheStore::new(&path);
        let envelope = PricingCacheEnvelope::new(
            json!({
                "gpt-test": {
                    "litellm_provider": "openai",
                    "input_cost_per_token": "0.000001",
                    "output_cost_per_token": "0.000002"
                }
            }),
            "sha256:test".into(),
            now_ms(),
        )
        .unwrap();
        store.write(&envelope).unwrap();

        let loader =
            PricingCatalogLoader::open_with_max_age(&path, DEFAULT_CATALOG_MAX_AGE_MS).unwrap();
        assert_eq!(loader.status(), CatalogStatus::Current);
        assert_eq!(
            loader.refresh_failed::<()>(PricingError::Network),
            Err(PricingError::Network)
        );
        assert_eq!(loader.status(), CatalogStatus::Stale);
        assert_eq!(loader.last_error(), Some(PricingError::Network));

        let persisted = store.read().unwrap().unwrap();
        assert!(persisted.stale);
        assert!(!loader.refresh_due(now_ms()));
        assert_eq!(
            loader
                .snapshot()
                .resolve_account("gpt-test", Some("openai"))
                .quote
                .map(|price| price.input),
            Some(1_000_000)
        );

        let reopened =
            PricingCatalogLoader::open_with_max_age(&path, DEFAULT_CATALOG_MAX_AGE_MS).unwrap();
        assert_eq!(reopened.status(), CatalogStatus::Stale);
        assert!(reopened.refresh_due(now_ms()));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn retry_backoff_progresses_and_reset_allows_an_immediate_attempt() {
        let mut retry = RefreshRetryState::default();
        let now = 10_000;

        assert!(retry.allows_attempt(now));
        retry.record_failure(now);
        assert_eq!(retry.next_retry_at_ms, Some(now + FIRST_RETRY_DELAY_MS));
        assert!(!retry.allows_attempt(now + FIRST_RETRY_DELAY_MS - 1));
        assert!(retry.allows_attempt(now + FIRST_RETRY_DELAY_MS));

        retry.record_failure(now + FIRST_RETRY_DELAY_MS);
        assert_eq!(
            retry.next_retry_at_ms,
            Some(now + FIRST_RETRY_DELAY_MS + SECOND_RETRY_DELAY_MS)
        );
        retry.record_failure(now + FIRST_RETRY_DELAY_MS + SECOND_RETRY_DELAY_MS);
        assert_eq!(
            retry.next_retry_at_ms,
            Some(now + FIRST_RETRY_DELAY_MS + SECOND_RETRY_DELAY_MS + SUBSEQUENT_RETRY_DELAY_MS)
        );

        retry.reset();
        assert_eq!(retry, RefreshRetryState::default());
        assert!(retry.allows_attempt(now));
    }

    #[test]
    fn retry_deadline_precedes_the_daily_schedule_and_is_not_jittered() {
        let directory = std::env::temp_dir().join(format!(
            "zenith-relay-pricing-retry-deadline-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let loader = PricingCatalogLoader::open(directory.join("litellm.json")).unwrap();
        let now = 10_000;

        loader.record_refresh_failure(now);
        assert_eq!(
            loader.next_refresh_deadline(now),
            CatalogRefreshDeadline {
                at_ms: now + FIRST_RETRY_DELAY_MS,
                kind: CatalogRefreshKind::Retry,
            }
        );
        assert!(!loader.refresh_due(now + FIRST_RETRY_DELAY_MS - 1));
        assert!(loader.refresh_due(now + FIRST_RETRY_DELAY_MS));
        let _ = fs::remove_dir_all(directory);
    }

    fn loader_with_fresh_test_cache(
        prefix: &str,
    ) -> (std::path::PathBuf, std::sync::Arc<PricingCatalogLoader>) {
        let directory = std::env::temp_dir().join(format!(
            "zenith-relay-pricing-{prefix}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("litellm.json");
        let envelope = PricingCacheEnvelope::new(
            json!({
                "gpt-test": {
                    "input_cost_per_token": "0.000001",
                    "output_cost_per_token": "0.000002"
                }
            }),
            "sha256:test".into(),
            now_ms(),
        )
        .unwrap();
        PricingCacheStore::new(&path).write(&envelope).unwrap();
        (
            directory,
            std::sync::Arc::new(PricingCatalogLoader::open(&path).unwrap()),
        )
    }

    #[tokio::test]
    async fn successful_manual_refresh_wakes_schedule_waiters() {
        let (directory, loader) = loader_with_fresh_test_cache("success-notify");
        let waiter_loader = std::sync::Arc::clone(&loader);
        let waiter = tokio::spawn(async move {
            waiter_loader.wait_for_schedule_change().await;
            (
                waiter_loader.status(),
                waiter_loader.next_refresh_deadline(now_ms()),
            )
        });
        tokio::task::yield_now().await;

        loader.record_refresh_success();

        let (status, deadline) = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("successful refresh should wake the scheduler")
            .expect("schedule waiter should not panic");
        assert_eq!(status, CatalogStatus::Current);
        assert_eq!(deadline.kind, CatalogRefreshKind::Scheduled);
        assert!(deadline.at_ms > now_ms());
        let _ = fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn failed_manual_refresh_wakes_waiters_after_retry_state_is_recorded() {
        let (directory, loader) = loader_with_fresh_test_cache("failure-notify");
        let waiter_loader = std::sync::Arc::clone(&loader);
        let waiter = tokio::spawn(async move {
            waiter_loader.wait_for_schedule_change().await;
            (
                waiter_loader.status(),
                waiter_loader.next_refresh_deadline(now_ms()),
            )
        });
        tokio::task::yield_now().await;

        assert_eq!(
            loader.refresh_failed::<()>(PricingError::Network),
            Err(PricingError::Network)
        );

        let (status, deadline) = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("failed refresh should wake the scheduler")
            .expect("schedule waiter should not panic");
        assert_eq!(deadline.kind, CatalogRefreshKind::Retry);
        assert!(deadline.at_ms > now_ms());
        assert_eq!(status, CatalogStatus::Stale);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn startup_check_precedes_ttl_and_then_daily_schedule_controls_refresh() {
        let directory = std::env::temp_dir().join(format!(
            "zenith-relay-pricing-due-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("litellm.json");
        let payload = json!({
            "gpt-test": {
                "input_cost_per_token": "0.000001",
                "output_cost_per_token": "0.000002"
            }
        });
        let now = now_ms();
        let fresh = PricingCacheEnvelope::new(payload.clone(), "sha256:fresh".into(), now).unwrap();
        let store = PricingCacheStore::new(&path);
        store.write(&fresh).unwrap();
        let loader = PricingCatalogLoader::open_with_max_age(&path, 60_000).unwrap();
        assert!(loader.refresh_due(now));
        assert_eq!(
            loader.next_refresh_deadline(now),
            CatalogRefreshDeadline {
                at_ms: now,
                kind: CatalogRefreshKind::Startup,
            }
        );
        loader.record_refresh_success();
        assert!(!loader.refresh_due(now));
        assert_eq!(
            loader.next_refresh_deadline(now),
            CatalogRefreshDeadline {
                at_ms: now + 60_000,
                kind: CatalogRefreshKind::Scheduled,
            }
        );

        let stale =
            PricingCacheEnvelope::new(payload, "sha256:stale".into(), now.saturating_sub(61_000))
                .unwrap();
        store.write(&stale).unwrap();
        let stale_loader = PricingCatalogLoader::open_with_max_age(&path, 60_000).unwrap();
        assert!(stale_loader.refresh_due(now));
        let _ = fs::remove_dir_all(directory);
    }
}
