use super::{
    enrich_reasoning_metadata_with_models_dev_details, payload_hash, validate_payload,
    MetadataSourceStatus, ModelMetadataCatalog, ModelMetadataError, CACHE_FORMAT, RETRY_DELAYS_MS,
    SOURCES, URLS,
};
use crate::pricing::{CatalogRefreshDeadline, CatalogRefreshKind, CatalogStatus};
use reqwest::header;
use serde::{Deserialize, Serialize};
use serde_json::{value::RawValue, Value};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SourceEnvelope {
    pub(super) source_url: String,
    pub(super) revision: String,
    pub(super) etag: Option<String>,
    pub(super) last_modified: Option<String>,
    pub(super) fetched_at_ms: u64,
    pub(super) stale: bool,
    // Requests use the resolved catalog. Keep source data as compact JSON
    // between refreshes instead of retaining thousands of allocated objects.
    pub(super) payload: Arc<RawValue>,
}
impl SourceEnvelope {
    pub(super) fn new(index: usize, payload: Value, now: u64) -> Result<Self, ModelMetadataError> {
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
            payload: serde_json::value::to_raw_value(&payload)
                .map(Arc::from)
                .map_err(|_| ModelMetadataError::InvalidCatalog)?,
        })
    }
    pub(super) fn validate(&self, index: usize) -> Result<Value, ModelMetadataError> {
        if self.source_url != URLS[index] || self.fetched_at_ms == 0 {
            return Err(ModelMetadataError::InvalidCache);
        }
        let payload = self
            .parse_payload()
            .map_err(|_| ModelMetadataError::InvalidCache)?;
        if self.revision != payload_hash(&payload)? || !valid_payload(index, &payload) {
            return Err(ModelMetadataError::InvalidCache);
        }
        Ok(payload)
    }
    pub(super) fn parse_payload(&self) -> Result<Value, ModelMetadataError> {
        serde_json::from_str(self.payload.get()).map_err(|_| ModelMetadataError::InvalidCatalog)
    }
    pub(super) fn validators(&mut self, headers: &header::HeaderMap) {
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
pub(super) struct SourceState {
    pub(super) envelope: Option<SourceEnvelope>,
    pub(super) status: CatalogStatus,
    pub(super) error: Option<ModelMetadataError>,
    pub(super) failures: usize,
    pub(super) retry_at_ms: Option<u64>,
    pub(super) startup_pending: bool,
}
impl SourceState {
    pub(super) fn new(
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
    pub(super) fn deadline(&self, now: u64, max_age: u64) -> CatalogRefreshDeadline {
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
    pub(super) fn accept(&mut self, envelope: SourceEnvelope) {
        self.envelope = Some(envelope);
        self.status = CatalogStatus::Current;
        self.error = None;
        self.failures = 0;
        self.retry_at_ms = None;
        self.startup_pending = false;
    }
    pub(super) fn fail(&mut self, error: ModelMetadataError, now: u64) {
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
pub(super) struct CacheBundle {
    pub(super) format: String,
    pub(super) schema_version: u32,
    pub(super) sources: BTreeMap<String, SourceEnvelope>,
    pub(super) merged_revision: String,
    pub(super) merged_payload: Value,
}
impl CacheBundle {
    pub(super) fn new(states: &[SourceState; 4]) -> Result<Self, ModelMetadataError> {
        let merged_payload = merged_payload(states)?;
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
fn merged_payload(states: &[SourceState; 4]) -> Result<Value, ModelMetadataError> {
    let mut parsed: [Option<Value>; 4] = std::array::from_fn(|_| None);
    for (slot, state) in parsed.iter_mut().zip(states) {
        *slot = state
            .envelope
            .as_ref()
            .map(SourceEnvelope::parse_payload)
            .transpose()?;
    }
    Ok(merge_payloads(&parsed))
}

pub(super) fn merge_payloads(parsed: &[Option<Value>; 4]) -> Value {
    let empty = serde_json::json!({});
    let payload = |index: usize| parsed[index].as_ref();
    enrich_reasoning_metadata_with_models_dev_details(
        payload(0).unwrap_or(&empty),
        payload(1),
        payload(2),
        payload(3),
    )
}
pub(super) fn build_catalog(
    states: &[SourceState; 4],
    now: u64,
    max_age: u64,
) -> ModelMetadataCatalog {
    let Ok(payload) = merged_payload(states) else {
        return ModelMetadataCatalog::empty();
    };
    catalog_from_payload(states, &payload, payload_hash(&payload).ok(), now, max_age)
}

pub(super) fn catalog_from_payload(
    states: &[SourceState; 4],
    payload: &Value,
    revision: Option<String>,
    now: u64,
    max_age: u64,
) -> ModelMetadataCatalog {
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
        payload,
        revision,
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

pub(super) fn valid_payload(index: usize, payload: &Value) -> bool {
    let valid_id = |id: &str| !id.trim().is_empty() && id.len() <= super::super::MAX_STRING_LENGTH;
    match index {
        0 => validate_payload(payload).is_ok(),
        1 => payload.as_object().is_some_and(|providers| {
            !providers.is_empty()
                && providers.len() <= super::super::MAX_RECORDS
                && providers.values().all(|provider| {
                    provider
                        .get("models")
                        .and_then(Value::as_object)
                        .is_some_and(|models| {
                            !models.is_empty()
                                && models.len() <= super::super::MAX_RECORDS
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
                    && items.len() <= super::super::MAX_RECORDS
                    && items
                        .iter()
                        .all(|item| item.get("id").and_then(Value::as_str).is_some_and(valid_id))
            }),
        3 => payload.as_object().is_some_and(|items| {
            !items.is_empty()
                && items.len() <= super::super::MAX_RECORDS
                && items
                    .iter()
                    .all(|(id, value)| valid_id(id) && value.is_object())
                && items.keys().any(|id| id != "sample_spec")
        }),
        _ => false,
    }
}
