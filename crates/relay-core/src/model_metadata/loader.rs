use super::{
    enrich_reasoning_metadata_with_models_dev_details, payload_hash, validate_payload,
    MetadataSourceStatus, ModelMetadataCatalog, ModelMetadataCatalogHandle,
    LITELLM_MODELS_SOURCE_URL, MODELS_DEV_DETAILS_SOURCE_URL, MODELS_DEV_SOURCE_URL,
    OPENROUTER_MODELS_SOURCE_URL,
};
use crate::{
    catalog_io::{self, CatalogIoError},
    pricing::{CatalogRefreshDeadline, CatalogRefreshOutcome, CatalogStatus},
};
use reqwest::{header, Client, StatusCode};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

mod cache;
mod source;

use source::{
    build_catalog, catalog_from_payload, merge_payloads, CacheBundle, SourceEnvelope, SourceState,
};

const REFRESH_INTERVAL_MS: u64 = 60 * 60 * 1_000;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_AUXILIARY_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 64 * 1024 * 1024;
const RETRY_DELAYS_MS: [u64; 3] = [300_000, 1_800_000, 7_200_000];
const SOURCES: [&str; 4] = ["models_dev", "models_dev_details", "openrouter", "litellm"];
const URLS: [&str; 4] = [
    MODELS_DEV_SOURCE_URL,
    MODELS_DEV_DETAILS_SOURCE_URL,
    OPENROUTER_MODELS_SOURCE_URL,
    LITELLM_MODELS_SOURCE_URL,
];
const CACHE_FORMAT: &str = "zenith-relay-merged-model-metadata-cache";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelMetadataError {
    InvalidCatalog,
    InvalidCache,
    CacheTooLarge,
    Io,
    Network,
    HttpStatus(u16),
}
impl std::fmt::Display for ModelMetadataError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCatalog => formatter.write_str("model metadata catalog is invalid"),
            Self::InvalidCache => formatter.write_str("model metadata cache is invalid"),
            Self::CacheTooLarge => formatter.write_str("model metadata catalog is too large"),
            Self::Io => formatter.write_str("model metadata cache I/O failed"),
            Self::Network => formatter.write_str("model metadata refresh failed"),
            Self::HttpStatus(status) => write!(formatter, "model metadata returned HTTP {status}"),
        }
    }
}
impl std::error::Error for ModelMetadataError {}

#[derive(Clone)]
pub struct ModelMetadataCatalogLoader {
    path: PathBuf,
    client: Client,
    handle: ModelMetadataCatalogHandle,
    state: Arc<RwLock<[SourceState; 4]>>,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    schedule_changed: Arc<tokio::sync::Notify>,
    max_age_ms: u64,
    // Private injection point for deterministic HTTP tests; production URLs
    // are fixed, never obtained from a provider response or cache payload.
    urls: [String; 4],
}
impl ModelMetadataCatalogLoader {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ModelMetadataError> {
        Self::open_with_max_age(path, REFRESH_INTERVAL_MS)
    }
    pub fn open_with_max_age(
        path: impl Into<PathBuf>,
        max_age_ms: u64,
    ) -> Result<Self, ModelMetadataError> {
        let path = path.into();
        let max_age_ms = max_age_ms.max(1);
        let now = crate::unix_time_ms();
        let (states, parsed) = cache::read_states(&path, now, max_age_ms);
        let payload = merge_payloads(&parsed);
        // Validation already parsed these sources. Reuse that work once, then
        // release the trees before constructing the long-lived catalog.
        drop(parsed);
        let catalog = catalog_from_payload(
            &states,
            &payload,
            payload_hash(&payload).ok(),
            now,
            max_age_ms,
        );
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ModelMetadataError::Network)?;
        Ok(Self {
            path,
            client,
            handle: ModelMetadataCatalogHandle::new(catalog),
            state: Arc::new(RwLock::new(states)),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            schedule_changed: Arc::new(tokio::sync::Notify::new()),
            max_age_ms,
            urls: URLS.map(str::to_string),
        })
    }
    pub fn snapshot(&self) -> Arc<ModelMetadataCatalog> {
        self.handle.snapshot()
    }
    pub fn catalog_handle(&self) -> ModelMetadataCatalogHandle {
        self.handle.clone()
    }
    pub fn cache_path(&self) -> &Path {
        &self.path
    }
    pub fn status(&self) -> CatalogStatus {
        let states = self.state.read().expect("metadata state lock poisoned");
        if states.iter().any(|s| s.status == CatalogStatus::Updating) {
            CatalogStatus::Updating
        } else if states.iter().all(|s| s.status == CatalogStatus::Current) {
            CatalogStatus::Current
        } else if states.iter().any(|s| s.envelope.is_some()) {
            CatalogStatus::Stale
        } else if states.iter().any(|s| s.error.is_some()) {
            CatalogStatus::Error
        } else {
            CatalogStatus::Unloaded
        }
    }
    pub fn auxiliary_status(&self, source: &str) -> CatalogStatus {
        SOURCES
            .iter()
            .position(|s| *s == source)
            .map_or(CatalogStatus::Unloaded, |i| {
                self.state.read().expect("metadata state lock poisoned")[i].status
            })
    }
    pub fn refresh_due(&self, now_ms: u64) -> bool {
        self.next_refresh_deadline(now_ms).at_ms <= now_ms
    }
    pub fn next_refresh_deadline(&self, now_ms: u64) -> CatalogRefreshDeadline {
        self.state
            .read()
            .expect("metadata state lock poisoned")
            .iter()
            .map(|s| s.deadline(now_ms, self.max_age_ms))
            .min_by_key(|d| d.at_ms)
            .expect("four catalog sources")
    }
    pub async fn wait_for_schedule_change(&self) {
        self.schedule_changed.notified().await;
    }
    pub async fn refresh(&self, force: bool) -> Result<CatalogRefreshOutcome, ModelMetadataError> {
        let _guard = self.refresh_lock.lock().await;
        let now = crate::unix_time_ms();
        let previous = self
            .state
            .read()
            .expect("metadata state lock poisoned")
            .clone();
        let due = previous
            .each_ref()
            .map(|s| force || s.deadline(now, self.max_age_ms).at_ms <= now);
        if !due.iter().any(|v| *v) {
            return Ok(CatalogRefreshOutcome::Skipped);
        }
        {
            let mut states = self.state.write().expect("metadata state lock poisoned");
            for (i, state) in states.iter_mut().enumerate().filter(|(i, _)| due[*i]) {
                let _ = i;
                state.status = CatalogStatus::Updating;
            }
        }
        let results = futures_util::future::join_all((0..4).map(|i| {
            let source_state = previous[i].clone();
            async move {
                if due[i] {
                    Some(self.fetch_source(i, source_state.envelope).await)
                } else {
                    None
                }
            }
        }))
        .await;
        let mut next = previous.clone();
        let mut first_error = None;
        let mut successes = 0;
        for (i, result) in results.into_iter().enumerate() {
            match result {
                Some(Ok(envelope)) => {
                    next[i].accept(envelope);
                    successes += 1;
                }
                Some(Err(error)) => {
                    first_error.get_or_insert(error);
                    next[i].fail(error, crate::unix_time_ms());
                }
                None => {}
            }
        }
        let bundle = match CacheBundle::new(&next) {
            Ok(bundle) => bundle,
            Err(error) => {
                // Do not leave sources stuck in Updating if building the
                // merged cache bundle fails before the write path is reached.
                // This is rare (the current JSON value is already validated),
                // but the state machine must recover from every fallible step.
                let mut failed = previous;
                let failure_at = crate::unix_time_ms();
                for (i, state) in failed.iter_mut().enumerate() {
                    if due[i] {
                        state.fail(error, failure_at);
                    }
                }
                self.publish(failed, None);
                return Err(error);
            }
        };
        if let Err(error) = catalog_io::write_json_if_changed(&self.path, &bundle, MAX_CACHE_BYTES)
        {
            let error = map_io_error(error, true);
            next = previous;
            for (i, state) in next.iter_mut().enumerate() {
                if due[i] {
                    state.fail(error, now);
                }
            }
            self.publish(next, None);
            return Err(error);
        }
        let old_revision = self.snapshot().revision.clone();
        let revision = bundle.merged_revision.clone();
        self.publish(next, Some(&bundle));
        if successes == 0 {
            if let Some(error) = first_error {
                return Err(error);
            }
        }
        Ok(if old_revision.as_ref() == Some(&revision) {
            CatalogRefreshOutcome::NotModified { revision }
        } else {
            CatalogRefreshOutcome::Updated { revision }
        })
    }
    fn publish(&self, states: [SourceState; 4], prepared: Option<&CacheBundle>) {
        let now = crate::unix_time_ms();
        let catalog = match prepared {
            Some(bundle) => catalog_from_payload(
                &states,
                &bundle.merged_payload,
                Some(bundle.merged_revision.clone()),
                now,
                self.max_age_ms,
            ),
            None => build_catalog(&states, now, self.max_age_ms),
        };
        self.handle.replace(catalog);
        *self.state.write().expect("metadata state lock poisoned") = states;
        self.schedule_changed.notify_one();
    }
    async fn fetch_source(
        &self,
        index: usize,
        current: Option<SourceEnvelope>,
    ) -> Result<SourceEnvelope, ModelMetadataError> {
        let mut request = self.client.get(&self.urls[index]);
        if let Some(e) = &current {
            if let Some(v) = &e.etag {
                request = request.header(header::IF_NONE_MATCH, v);
            }
            if let Some(v) = &e.last_modified {
                request = request.header(header::IF_MODIFIED_SINCE, v);
            }
        }
        let (response, permit) = crate::scheduler::refresh::http::management_http_gate()
            .send(
                &self.client,
                request,
                crate::scheduler::refresh::http::HttpClass::Ordinary,
            )
            .await
            .map_err(|_| ModelMetadataError::Network)?;
        let headers = response.headers().clone();
        let mut envelope = if response.status() == StatusCode::NOT_MODIFIED {
            let mut envelope = current.ok_or(ModelMetadataError::InvalidCache)?;
            envelope.fetched_at_ms = crate::unix_time_ms();
            envelope.stale = false;
            envelope
        } else if response.status() == StatusCode::OK {
            let limit = if index == 0 {
                MAX_RESPONSE_BYTES
            } else {
                MAX_AUXILIARY_RESPONSE_BYTES
            };
            let payload = catalog_io::response_json(response, limit)
                .await
                .map_err(|e| map_io_error(e, false))?;
            SourceEnvelope::new(index, payload, crate::unix_time_ms())?
        } else {
            return Err(ModelMetadataError::HttpStatus(response.status().as_u16()));
        };
        drop(permit);
        envelope.validators(&headers);
        Ok(envelope)
    }
}
fn loaded_state(
    result: Result<SourceEnvelope, ModelMetadataError>,
    now: u64,
    age: u64,
) -> SourceState {
    match result {
        Ok(e) => SourceState::new(Some(e), None, now, age),
        Err(e) => SourceState::new(None, Some(e), now, age),
    }
}
fn map_io_error(error: CatalogIoError, cache: bool) -> ModelMetadataError {
    match error {
        CatalogIoError::TooLarge => ModelMetadataError::CacheTooLarge,
        CatalogIoError::Io => ModelMetadataError::Io,
        CatalogIoError::Network => ModelMetadataError::Network,
        CatalogIoError::InvalidJson if cache => ModelMetadataError::InvalidCache,
        CatalogIoError::InvalidJson => ModelMetadataError::InvalidCatalog,
    }
}

#[cfg(test)]
mod tests;
