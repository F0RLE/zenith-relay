use super::{
    loaded_state, map_io_error, ModelMetadataError, SourceEnvelope, SourceState, CACHE_FORMAT,
    MAX_AUXILIARY_RESPONSE_BYTES, MAX_CACHE_BYTES, SOURCES,
};
use crate::{catalog_io, model_metadata::MetadataCacheEnvelope};
use serde::Deserialize;
use serde_json::{value::RawValue, Value};
use std::{collections::BTreeMap, path::Path, sync::Arc};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheHeader<'a> {
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    schema_version: Value,
    #[serde(default, borrow)]
    sources: Option<&'a RawValue>,
}

pub(super) fn read_states(
    path: &Path,
    now: u64,
    age: u64,
) -> ([SourceState; 4], [Option<Value>; 4]) {
    let mut states = std::array::from_fn(|_| SourceState::new(None, None, now, age));
    let mut parsed = std::array::from_fn(|_| None);
    let raw = match catalog_io::read_bytes(path, MAX_CACHE_BYTES) {
        Ok(Some(raw)) => raw,
        Ok(None) => return (states, parsed),
        Err(error) => {
            states[0] = loaded_state(Err(map_io_error(error, true)), now, age);
            return (states, parsed);
        }
    };
    let header = match serde_json::from_slice::<CacheHeader<'_>>(&raw) {
        Ok(header) => header,
        Err(error) if error.is_data() => return read_legacy_states(path, &raw, now, age),
        Err(_) => {
            states[0] = loaded_state(Err(ModelMetadataError::InvalidCache), now, age);
            return (states, parsed);
        }
    };
    if header.format.as_deref() != Some(CACHE_FORMAT) {
        return read_legacy_states(path, &raw, now, age);
    }
    if header.schema_version.as_u64() != Some(2) {
        states[0] = loaded_state(Err(ModelMetadataError::InvalidCache), now, age);
        return (states, parsed);
    }
    // Borrow each source envelope from the bounded input. Do not allocate the
    // stored merged tree: it is only a derivative and never trusted on reload.
    let sources = header
        .sources
        .and_then(|raw| serde_json::from_str::<BTreeMap<String, &RawValue>>(raw.get()).ok());
    if let Some(sources) = sources {
        for (i, state) in states.iter_mut().enumerate() {
            if let Some(raw) = sources.get(SOURCES[i]) {
                let envelope = serde_json::from_str::<SourceEnvelope>(raw.get())
                    .map_err(|_| ModelMetadataError::InvalidCache)
                    .and_then(|envelope| {
                        parsed[i] = Some(envelope.validate(i)?);
                        Ok(envelope)
                    });
                // A malformed source cannot discard the remaining sources.
                *state = loaded_state(envelope, now, age);
            }
        }
    }
    (states, parsed)
}

fn read_legacy_states(
    path: &Path,
    raw: &[u8],
    now: u64,
    age: u64,
) -> ([SourceState; 4], [Option<Value>; 4]) {
    let mut states = std::array::from_fn(|_| SourceState::new(None, None, now, age));
    let mut parsed = std::array::from_fn(|_| None);
    let envelope = serde_json::from_slice::<MetadataCacheEnvelope>(raw)
        .map_err(|_| ModelMetadataError::InvalidCache)
        .and_then(|envelope| {
            envelope.validate()?;
            let payload = serde_json::value::to_raw_value(&envelope.payload)
                .map(Arc::from)
                .map_err(|_| ModelMetadataError::InvalidCache)?;
            parsed[0] = Some(envelope.payload);
            Ok(SourceEnvelope {
                source_url: envelope.source_url,
                revision: envelope.revision,
                etag: envelope.etag,
                last_modified: envelope.last_modified,
                fetched_at_ms: envelope.fetched_at_ms,
                stale: envelope.stale,
                payload,
            })
        });
    states[0] = loaded_state(envelope, now, age);
    for (i, file) in [(2, "openrouter-models.json"), (3, "litellm-models.json")] {
        if let Ok(Some(envelope)) = catalog_io::read_json::<SourceEnvelope>(
            &path.with_file_name(file),
            MAX_AUXILIARY_RESPONSE_BYTES,
        ) {
            let validated = envelope.validate(i).map(|payload| {
                parsed[i] = Some(payload);
                envelope
            });
            states[i] = loaded_state(validated, now, age);
        }
    }
    (states, parsed)
}
