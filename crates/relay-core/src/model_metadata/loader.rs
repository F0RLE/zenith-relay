use super::{
    enrich_reasoning_metadata_with_models_dev_details, payload_hash, validate_payload,
    MetadataCacheEnvelope, MetadataSourceStatus, ModelMetadataCatalog, ModelMetadataCatalogHandle,
    LITELLM_MODELS_SOURCE_URL, MODELS_DEV_DETAILS_SOURCE_URL, MODELS_DEV_SOURCE_URL,
    OPENROUTER_MODELS_SOURCE_URL,
};
use crate::{
    catalog_io::{self, CatalogIoError},
    pricing::{CatalogRefreshDeadline, CatalogRefreshKind, CatalogRefreshOutcome, CatalogStatus},
};
use reqwest::{header, Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceEnvelope {
    source_url: String,
    revision: String,
    etag: Option<String>,
    last_modified: Option<String>,
    fetched_at_ms: u64,
    stale: bool,
    payload: Value,
}
impl SourceEnvelope {
    fn new(index: usize, payload: Value, now: u64) -> Result<Self, ModelMetadataError> {
        if !valid_payload(index, &payload) {
            return Err(ModelMetadataError::InvalidCatalog);
        }
        Ok(Self {
            source_url: URLS[index].into(),
            revision: payload_hash(&payload)?,
            etag: None,
            last_modified: None,
            fetched_at_ms: now,
            stale: false,
            payload,
        })
    }
    fn validate(&self, index: usize) -> Result<(), ModelMetadataError> {
        if self.source_url != URLS[index]
            || self.fetched_at_ms == 0
            || self.revision != payload_hash(&self.payload)?
            || !valid_payload(index, &self.payload)
        {
            return Err(ModelMetadataError::InvalidCache);
        }
        Ok(())
    }
    fn validators(&mut self, headers: &header::HeaderMap) {
        for (target, name) in [
            (&mut self.etag, header::ETAG),
            (&mut self.last_modified, header::LAST_MODIFIED),
        ] {
            if let Some(value) = headers.get(name).and_then(|v| v.to_str().ok()) {
                *target = Some(value.into());
            }
        }
    }
}

#[derive(Clone, Debug)]
struct SourceState {
    envelope: Option<SourceEnvelope>,
    status: CatalogStatus,
    error: Option<ModelMetadataError>,
    failures: usize,
    retry_at_ms: Option<u64>,
    startup_pending: bool,
}
impl SourceState {
    fn new(
        envelope: Option<SourceEnvelope>,
        error: Option<ModelMetadataError>,
        now: u64,
        max_age: u64,
    ) -> Self {
        let status = envelope.as_ref().map_or_else(
            || {
                if error.is_some() {
                    CatalogStatus::Error
                } else {
                    CatalogStatus::Unloaded
                }
            },
            |e| {
                if e.stale || now.saturating_sub(e.fetched_at_ms) >= max_age {
                    CatalogStatus::Stale
                } else {
                    CatalogStatus::Current
                }
            },
        );
        Self {
            envelope,
            status,
            error,
            failures: 0,
            retry_at_ms: None,
            startup_pending: true,
        }
    }
    fn deadline(&self, now: u64, max_age: u64) -> CatalogRefreshDeadline {
        if let Some(at_ms) = self.retry_at_ms {
            return CatalogRefreshDeadline {
                at_ms,
                kind: CatalogRefreshKind::Retry,
            };
        }
        if self.startup_pending {
            return CatalogRefreshDeadline {
                at_ms: now,
                kind: CatalogRefreshKind::Startup,
            };
        }
        CatalogRefreshDeadline {
            at_ms: self.envelope.as_ref().map_or(now, |e| {
                if e.stale {
                    now
                } else {
                    e.fetched_at_ms.saturating_add(max_age)
                }
            }),
            kind: CatalogRefreshKind::Scheduled,
        }
    }
    fn accept(&mut self, envelope: SourceEnvelope) {
        self.envelope = Some(envelope);
        self.status = CatalogStatus::Current;
        self.error = None;
        self.failures = 0;
        self.retry_at_ms = None;
        self.startup_pending = false;
    }
    fn fail(&mut self, error: ModelMetadataError, now: u64) {
        self.failures = self.failures.saturating_add(1);
        self.retry_at_ms =
            Some(now.saturating_add(RETRY_DELAYS_MS[self.failures.saturating_sub(1).min(2)]));
        self.error = Some(error);
        self.startup_pending = false;
        self.status = if let Some(envelope) = self.envelope.as_mut() {
            envelope.stale = true;
            CatalogStatus::Stale
        } else {
            CatalogStatus::Error
        };
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CacheBundle {
    format: String,
    schema_version: u32,
    sources: BTreeMap<String, SourceEnvelope>,
    merged_revision: String,
    merged_payload: Value,
}
impl CacheBundle {
    fn new(states: &[SourceState; 4]) -> Result<Self, ModelMetadataError> {
        let merged_payload = merged_payload(states);
        Ok(Self {
            format: CACHE_FORMAT.into(),
            schema_version: 2,
            sources: states
                .iter()
                .enumerate()
                .filter_map(|(i, s)| s.envelope.clone().map(|e| (SOURCES[i].into(), e)))
                .collect(),
            merged_revision: payload_hash(&merged_payload)?,
            merged_payload,
        })
    }
}
fn merged_payload(states: &[SourceState; 4]) -> Value {
    let empty = serde_json::json!({});
    let payload = |index: usize| states[index].envelope.as_ref().map(|e| &e.payload);
    enrich_reasoning_metadata_with_models_dev_details(
        payload(0).unwrap_or(&empty),
        payload(1),
        payload(2),
        payload(3),
    )
}
fn build_catalog(states: &[SourceState; 4], now: u64, max_age: u64) -> ModelMetadataCatalog {
    let payload = merged_payload(states);
    let statuses: BTreeMap<_, _> = states
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (
                SOURCES[i].into(),
                MetadataSourceStatus {
                    revision: s.envelope.as_ref().map(|e| e.revision.clone()),
                    fetched_at_ms: s.envelope.as_ref().map(|e| e.fetched_at_ms),
                    stale: s
                        .envelope
                        .as_ref()
                        .is_none_or(|e| e.stale || now.saturating_sub(e.fetched_at_ms) >= max_age),
                },
            )
        })
        .collect();
    let stale = statuses.values().any(|s| s.stale);
    let mut catalog = ModelMetadataCatalog::from_payload(
        &payload,
        payload_hash(&payload).ok(),
        states
            .iter()
            .filter_map(|s| s.envelope.as_ref().map(|e| e.fetched_at_ms))
            .max(),
        stale,
    )
    .unwrap_or_else(|_| ModelMetadataCatalog::empty());
    catalog.sources = statuses;
    catalog.stale = stale;
    catalog
}

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
        let now = catalog_io::unix_time_ms();
        let mut states = std::array::from_fn(|_| SourceState::new(None, None, now, max_age_ms));
        match catalog_io::read_json::<Value>(&path, MAX_CACHE_BYTES) {
            Ok(Some(raw)) if raw.get("format").and_then(Value::as_str) == Some(CACHE_FORMAT) => {
                // Validate sources independently: one damaged source cannot
                // erase the other two. Never trust the stored merged payload.
                if raw.get("schemaVersion").and_then(Value::as_u64) == Some(2) {
                    for (i, state) in states.iter_mut().enumerate() {
                        if let Some(value) = raw.get("sources").and_then(|v| v.get(SOURCES[i])) {
                            let envelope = serde_json::from_value::<SourceEnvelope>(value.clone())
                                .map_err(|_| ModelMetadataError::InvalidCache)
                                .and_then(|e| {
                                    e.validate(i)?;
                                    Ok(e)
                                });
                            *state = loaded_state(envelope, now, max_age_ms);
                        }
                    }
                } else {
                    states[0] =
                        loaded_state(Err(ModelMetadataError::InvalidCache), now, max_age_ms);
                }
            }
            Ok(Some(raw)) => {
                let envelope = serde_json::from_value::<MetadataCacheEnvelope>(raw)
                    .map_err(|_| ModelMetadataError::InvalidCache)
                    .and_then(|e| {
                        e.validate()?;
                        Ok(SourceEnvelope {
                            source_url: e.source_url,
                            revision: e.revision,
                            etag: e.etag,
                            last_modified: e.last_modified,
                            fetched_at_ms: e.fetched_at_ms,
                            stale: e.stale,
                            payload: e.payload,
                        })
                    });
                states[0] = loaded_state(envelope, now, max_age_ms);
                // Migrate validated auxiliary caches from the earlier format.
                for (i, file) in [(2, "openrouter-models.json"), (3, "litellm-models.json")] {
                    if let Ok(Some(e)) = catalog_io::read_json::<SourceEnvelope>(
                        &path.with_file_name(file),
                        MAX_AUXILIARY_RESPONSE_BYTES,
                    ) {
                        states[i] = loaded_state(e.validate(i).map(|()| e), now, max_age_ms);
                    }
                }
            }
            Ok(None) => {}
            Err(error) => states[0] = loaded_state(Err(map_io_error(error, true)), now, max_age_ms),
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ModelMetadataError::Network)?;
        Ok(Self {
            path,
            client,
            handle: ModelMetadataCatalogHandle::new(build_catalog(&states, now, max_age_ms)),
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
        let now = catalog_io::unix_time_ms();
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
                    next[i].fail(error, catalog_io::unix_time_ms());
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
                let failure_at = catalog_io::unix_time_ms();
                for (i, state) in failed.iter_mut().enumerate() {
                    if due[i] {
                        state.fail(error, failure_at);
                    }
                }
                self.publish(failed);
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
            self.publish(next);
            return Err(error);
        }
        let old_revision = self.snapshot().revision.clone();
        let revision = bundle.merged_revision;
        self.publish(next);
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
    fn publish(&self, states: [SourceState; 4]) {
        self.handle.replace(build_catalog(
            &states,
            catalog_io::unix_time_ms(),
            self.max_age_ms,
        ));
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
        let response = request
            .send()
            .await
            .map_err(|_| ModelMetadataError::Network)?;
        let headers = response.headers().clone();
        let mut envelope = if response.status() == StatusCode::NOT_MODIFIED {
            let mut envelope = current.ok_or(ModelMetadataError::InvalidCache)?;
            envelope.fetched_at_ms = catalog_io::unix_time_ms();
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
            SourceEnvelope::new(index, payload, catalog_io::unix_time_ms())?
        } else {
            return Err(ModelMetadataError::HttpStatus(response.status().as_u16()));
        };
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
fn valid_payload(index: usize, payload: &Value) -> bool {
    let valid_id = |id: &str| !id.trim().is_empty() && id.len() <= super::MAX_STRING_LENGTH;
    match index {
        0 => validate_payload(payload).is_ok(),
        1 => payload.as_object().is_some_and(|providers| {
            !providers.is_empty()
                && providers.len() <= super::MAX_RECORDS
                && providers.values().all(|provider| {
                    provider
                        .get("models")
                        .and_then(Value::as_object)
                        .is_some_and(|models| {
                            !models.is_empty()
                                && models.len() <= super::MAX_RECORDS
                                && models
                                    .iter()
                                    .all(|(id, value)| valid_id(id) && value.is_object())
                        })
                })
        }),
        2 => payload
            .get("data")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                !items.is_empty()
                    && items.len() <= super::MAX_RECORDS
                    && items
                        .iter()
                        .all(|item| item.get("id").and_then(Value::as_str).is_some_and(valid_id))
            }),
        3 => payload.as_object().is_some_and(|items| {
            !items.is_empty()
                && items.len() <= super::MAX_RECORDS
                && items
                    .iter()
                    .all(|(id, value)| valid_id(id) && value.is_object())
                && items.keys().any(|id| id != "sample_spec")
        }),
        _ => false,
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
