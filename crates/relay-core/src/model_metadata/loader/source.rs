use super::{
    enrich_reasoning_metadata_with_models_dev_details, metadata_payload_hash,
    validate_metadata_payload, MetadataSourceStatus, ModelMetadataCatalog, ModelMetadataError,
    CACHE_FORMAT, RETRY_DELAYS_MS, SOURCES, URLS,
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
    pub(super) fn new(
        index: usize,
        source_payload: Value,
        now: u64,
    ) -> Result<Self, ModelMetadataError> {
        if !valid_source_payload(index, &source_payload) {
            return Err(ModelMetadataError::InvalidCatalog);
        }
        Ok(Self {
            source_url: URLS[index].into(),
            revision: metadata_payload_hash(&source_payload)?,
            etag: None,
            last_modified: None,
            fetched_at_ms: now,
            stale: false,
            payload: serde_json::value::to_raw_value(&source_payload)
                .map(Arc::from)
                .map_err(|_| ModelMetadataError::InvalidCatalog)?,
        })
    }
    pub(super) fn validate(&self, index: usize) -> Result<Value, ModelMetadataError> {
        if self.source_url != URLS[index] || self.fetched_at_ms == 0 {
            return Err(ModelMetadataError::InvalidCache);
        }
        let parsed_payload = self
            .parse_source_payload()
            .map_err(|_| ModelMetadataError::InvalidCache)?;
        if self.revision != metadata_payload_hash(&parsed_payload)?
            || !valid_source_payload(index, &parsed_payload)
        {
            return Err(ModelMetadataError::InvalidCache);
        }
        Ok(parsed_payload)
    }
    pub(super) fn parse_source_payload(&self) -> Result<Value, ModelMetadataError> {
        serde_json::from_str(self.payload.get()).map_err(|_| ModelMetadataError::InvalidCatalog)
    }
    pub(super) fn validators(&mut self, headers: &header::HeaderMap) {
        for (target, name) in [
            (&mut self.etag, header::ETAG),
            (&mut self.last_modified, header::LAST_MODIFIED),
        ] {
            if let Some(header_value) = headers
                .get(name)
                .and_then(|header_value| header_value.to_str().ok())
            {
                *target = Some(header_value.into());
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
            |source_envelope| {
                if source_envelope.stale
                    || now.saturating_sub(source_envelope.fetched_at_ms) >= max_age
                {
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
            at_ms: self.envelope.as_ref().map_or(now, |source_envelope| {
                if source_envelope.stale {
                    now
                } else {
                    source_envelope.fetched_at_ms.saturating_add(max_age)
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
        let merged_payload = build_merged_metadata_payload(states)?;
        Ok(Self {
            format: CACHE_FORMAT.into(),
            schema_version: 2,
            sources: states
                .iter()
                .enumerate()
                .filter_map(|(source_index, source_state)| {
                    source_state
                        .envelope
                        .clone()
                        .map(|source_envelope| (SOURCES[source_index].into(), source_envelope))
                })
                .collect(),
            merged_revision: metadata_payload_hash(&merged_payload)?,
            merged_payload,
        })
    }
}
fn build_merged_metadata_payload(states: &[SourceState; 4]) -> Result<Value, ModelMetadataError> {
    let mut parsed: [Option<Value>; 4] = std::array::from_fn(|_| None);
    for (slot, state) in parsed.iter_mut().zip(states) {
        *slot = state
            .envelope
            .as_ref()
            .map(SourceEnvelope::parse_source_payload)
            .transpose()?;
    }
    Ok(merge_payloads(&parsed))
}

pub(super) fn merge_payloads(parsed: &[Option<Value>; 4]) -> Value {
    let empty = serde_json::json!({});
    let source_payload = |source_index: usize| parsed[source_index].as_ref();
    enrich_reasoning_metadata_with_models_dev_details(
        source_payload(0).unwrap_or(&empty),
        source_payload(1),
        source_payload(2),
        source_payload(3),
    )
}
pub(super) fn build_catalog(
    states: &[SourceState; 4],
    now: u64,
    max_age: u64,
) -> ModelMetadataCatalog {
    let Ok(merged_catalog_payload) = build_merged_metadata_payload(states) else {
        return ModelMetadataCatalog::empty();
    };
    catalog_from_payload(
        states,
        &merged_catalog_payload,
        metadata_payload_hash(&merged_catalog_payload).ok(),
        now,
        max_age,
    )
}

pub(super) fn catalog_from_payload(
    states: &[SourceState; 4],
    merged_catalog_payload: &Value,
    revision: Option<String>,
    now: u64,
    max_age: u64,
) -> ModelMetadataCatalog {
    let statuses: BTreeMap<_, _> = states
        .iter()
        .enumerate()
        .map(|(source_index, source_state)| {
            (
                SOURCES[source_index].into(),
                MetadataSourceStatus {
                    revision: source_state
                        .envelope
                        .as_ref()
                        .map(|source_envelope| source_envelope.revision.clone()),
                    fetched_at_ms: source_state
                        .envelope
                        .as_ref()
                        .map(|source_envelope| source_envelope.fetched_at_ms),
                    stale: source_state
                        .envelope
                        .as_ref()
                        .is_none_or(|source_envelope| {
                            source_envelope.stale
                                || now.saturating_sub(source_envelope.fetched_at_ms) >= max_age
                        }),
                },
            )
        })
        .collect();
    let stale = statuses.values().any(|s| s.stale);
    let mut catalog = ModelMetadataCatalog::from_metadata_payload(
        merged_catalog_payload,
        revision,
        states
            .iter()
            .filter_map(|source_state| {
                source_state
                    .envelope
                    .as_ref()
                    .map(|source_envelope| source_envelope.fetched_at_ms)
            })
            .max(),
        stale,
    )
    .unwrap_or_else(|_| ModelMetadataCatalog::empty());
    catalog.sources = statuses;
    catalog.stale = stale;
    catalog
}

pub(super) fn valid_source_payload(index: usize, source_payload: &Value) -> bool {
    let valid_id = |id: &str| !id.trim().is_empty() && id.len() <= super::super::MAX_STRING_LENGTH;
    match index {
        0 => validate_metadata_payload(source_payload).is_ok(),
        1 => source_payload.as_object().is_some_and(|providers| {
            !providers.is_empty()
                && providers.len() <= super::super::MAX_RECORDS
                && providers.values().all(|provider| {
                    provider
                        .get("models")
                        .and_then(Value::as_object)
                        .is_some_and(|models_by_id| {
                            !models_by_id.is_empty()
                                && models_by_id.len() <= super::super::MAX_RECORDS
                                && models_by_id.iter().all(|(model_id, model_metadata)| {
                                    valid_id(model_id) && model_metadata.is_object()
                                })
                        })
                })
        }),
        2 => source_payload
            .get("data")
            .and_then(Value::as_array)
            .is_some_and(|model_records| {
                !model_records.is_empty()
                    && model_records.len() <= super::super::MAX_RECORDS
                    && model_records.iter().all(|model_record| {
                        model_record
                            .get("id")
                            .and_then(Value::as_str)
                            .is_some_and(valid_id)
                    })
            }),
        3 => source_payload.as_object().is_some_and(|model_entries| {
            !model_entries.is_empty()
                && model_entries.len() <= super::super::MAX_RECORDS
                && model_entries.iter().all(|(model_id, model_metadata)| {
                    valid_id(model_id) && model_metadata.is_object()
                })
                && model_entries
                    .keys()
                    .any(|model_id| model_id != "sample_spec")
        }),
        _ => false,
    }
}
