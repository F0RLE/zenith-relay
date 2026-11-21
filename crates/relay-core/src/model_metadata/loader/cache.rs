use super::{
    CACHE_FORMAT, MAX_CACHE_BYTES, ModelMetadataError, SOURCES, SourceEnvelope, SourceState,
    loaded_state, map_io_error,
};
use crate::catalog_io;
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};
use std::{collections::BTreeMap, path::Path};

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

/// Same v2 source envelopes, without the untrusted, rebuildable merged tree.
pub(super) fn write_states(
    path: &Path,
    states: &[SourceState; 4],
    merged_revision: &str,
) -> Result<(), ModelMetadataError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct SourceCache<'a> {
        format: &'static str,
        schema_version: u32,
        sources: BTreeMap<&'static str, &'a SourceEnvelope>,
        merged_revision: &'a str,
    }
    let cache = SourceCache {
        format: CACHE_FORMAT,
        schema_version: 2,
        sources: states
            .iter()
            .enumerate()
            .filter_map(|(i, state)| {
                state
                    .envelope
                    .as_ref()
                    .map(|envelope| (SOURCES[i], envelope))
            })
            .collect(),
        merged_revision,
    };
    catalog_io::write_json_if_changed(path, &cache, MAX_CACHE_BYTES)
        .map(|_| ())
        .map_err(|error| map_io_error(error, true))
}

pub(super) fn read_states(
    path: &Path,
    now: u64,
    age: u64,
) -> ([SourceState; 4], [Option<Value>; 4]) {
    let mut states = std::array::from_fn(|_| SourceState::new(None, None, now, age));
    let mut parsed = std::array::from_fn(|_| None);
    let cache_bytes = match catalog_io::read_bytes(path, MAX_CACHE_BYTES) {
        Ok(Some(cache_bytes)) => cache_bytes,
        Ok(None) => return (states, parsed),
        Err(error) => {
            states[0] = loaded_state(Err(map_io_error(error, true)), now, age);
            return (states, parsed);
        }
    };
    let header = match serde_json::from_slice::<CacheHeader<'_>>(&cache_bytes) {
        Ok(header) => header,
        Err(_) => {
            states[0] = loaded_state(Err(ModelMetadataError::InvalidCache), now, age);
            return (states, parsed);
        }
    };
    if header.format.as_deref() != Some(CACHE_FORMAT) || header.schema_version.as_u64() != Some(2) {
        states[0] = loaded_state(Err(ModelMetadataError::InvalidCache), now, age);
        return (states, parsed);
    }
    // Borrow each source envelope from the bounded input. Do not allocate the
    // stored merged tree: it is only a derivative and never trusted on reload.
    let sources = header.sources.and_then(|source_map_raw| {
        serde_json::from_str::<BTreeMap<String, &RawValue>>(source_map_raw.get()).ok()
    });
    if let Some(sources) = sources {
        for (i, state) in states.iter_mut().enumerate() {
            if let Some(source_envelope_raw) = sources.get(SOURCES[i]) {
                let envelope = serde_json::from_str::<SourceEnvelope>(source_envelope_raw.get())
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
