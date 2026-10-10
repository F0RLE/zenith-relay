use super::{
    enrich_reasoning_metadata_with_models_dev_details, metadata_payload_hash,
    validate_metadata_payload, MetadataSourceStatus, ModelMetadataCatalog,
    ModelMetadataCatalogHandle, LITELLM_MODELS_SOURCE_URL, MODELS_DEV_DETAILS_SOURCE_URL,
    MODELS_DEV_SOURCE_URL, OPENROUTER_MODELS_SOURCE_URL,
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
        let merged_catalog_payload = merge_payloads(&parsed);
        // Validation already parsed these sources. Reuse that work once, then
        // release the trees before constructing the long-lived catalog.
        drop(parsed);
        let catalog = catalog_from_payload(
            &states,
            &merged_catalog_payload,
            metadata_payload_hash(&merged_catalog_payload).ok(),
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
        let cached_states = self
            .state
            .read()
            .expect("metadata state lock poisoned")
            .clone();
        let due = cached_states
            .each_ref()
            .map(|s| force || s.deadline(now, self.max_age_ms).at_ms <= now);
        if !due.iter().any(|source_is_due| *source_is_due) {
            return Ok(CatalogRefreshOutcome::Skipped);
        }
        {
            let mut states = self.state.write().expect("metadata state lock poisoned");
            for (source_index, source_state) in states.iter_mut().enumerate() {
                if due[source_index] {
                    source_state.status = CatalogStatus::Updating;
                }
            }
        }
        let results = futures_util::future::join_all((0..4).map(|source_index| {
            let source_state = cached_states[source_index].clone();
            async move {
                if due[source_index] {
                    Some(self.fetch_source(source_index, source_state.envelope).await)
                } else {
                    None
                }
            }
        }))
        .await;
        let mut updated_states = cached_states.clone();
        let mut first_error = None;
        let mut successes = 0;
        for (source_index, source_load_result) in results.into_iter().enumerate() {
            match source_load_result {
                Some(Ok(envelope)) => {
                    updated_states[source_index].accept(envelope);
                    successes += 1;
                }
                Some(Err(error)) => {
                    first_error.get_or_insert(error);
                    updated_states[source_index].fail(error, crate::unix_time_ms());
                }
                None => {}
            }
        }
        let bundle = match CacheBundle::new(&updated_states) {
            Ok(bundle) => bundle,
            Err(error) => {
                // Do not leave sources stuck in Updating if building the
                // merged cache bundle fails before the write path is reached.
                // This is rare (the current JSON value is already validated),
                // but the state machine must recover from every fallible step.
                let mut failed = cached_states.clone();
                let failure_at = crate::unix_time_ms();
                for (source_index, source_state) in failed.iter_mut().enumerate() {
                    if due[source_index] {
                        source_state.fail(error, failure_at);
                    }
                }
                self.publish(failed, None);
                return Err(error);
            }
        };
        if let Err(error) = catalog_io::write_json_if_changed(&self.path, &bundle, MAX_CACHE_BYTES)
        {
            let error = map_io_error(error, true);
            updated_states = cached_states;
            for (source_index, source_state) in updated_states.iter_mut().enumerate() {
                if due[source_index] {
                    source_state.fail(error, now);
                }
            }
            self.publish(updated_states, None);
            return Err(error);
        }
        let previous_revision = self.snapshot().revision.clone();
        let revision = bundle.merged_revision.clone();
        self.publish(updated_states, Some(&bundle));
        if successes == 0 {
            if let Some(error) = first_error {
                return Err(error);
            }
        }
        Ok(if previous_revision.as_ref() == Some(&revision) {
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
        cached_envelope: Option<SourceEnvelope>,
    ) -> Result<SourceEnvelope, ModelMetadataError> {
        let mut metadata_request = self.client.get(&self.urls[index]);
        if let Some(current_envelope) = &cached_envelope {
            if let Some(etag) = &current_envelope.etag {
                metadata_request = metadata_request.header(header::IF_NONE_MATCH, etag);
            }
            if let Some(last_modified) = &current_envelope.last_modified {
                metadata_request =
                    metadata_request.header(header::IF_MODIFIED_SINCE, last_modified);
            }
        }
        let (response, permit) = crate::scheduler::refresh::http::management_http_gate()
            .send(
                &self.client,
                metadata_request,
                crate::scheduler::refresh::http::HttpClass::Ordinary,
            )
            .await
            .map_err(|_| ModelMetadataError::Network)?;
        let headers = response.headers().clone();
        let mut envelope = if response.status() == StatusCode::NOT_MODIFIED {
            let mut envelope = cached_envelope.ok_or(ModelMetadataError::InvalidCache)?;
            envelope.fetched_at_ms = crate::unix_time_ms();
            envelope.stale = false;
            envelope
        } else if response.status() == StatusCode::OK {
            let limit = if index == 0 {
                MAX_RESPONSE_BYTES
            } else {
                MAX_AUXILIARY_RESPONSE_BYTES
            };
            let source_payload = catalog_io::response_json(response, limit)
                .await
                .map_err(|io_error| map_io_error(io_error, false))?;
            SourceEnvelope::new(index, source_payload, crate::unix_time_ms())?
        } else {
            return Err(ModelMetadataError::HttpStatus(response.status().as_u16()));
        };
        drop(permit);
        envelope.validators(&headers);
        Ok(envelope)
    }
}
fn loaded_state(
    source_load_result: Result<SourceEnvelope, ModelMetadataError>,
    now: u64,
    age: u64,
) -> SourceState {
    match source_load_result {
        Ok(source_envelope) => SourceState::new(Some(source_envelope), None, now, age),
        Err(source_error) => SourceState::new(None, Some(source_error), now, age),
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
