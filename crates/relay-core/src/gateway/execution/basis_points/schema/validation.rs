use super::super::{describe_cache, try_with_cache};
use crate::gateway::CacheMemoryDiagnostics;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_ENTRIES: usize = 512;
const MAX_SCHEMA_BYTES: usize = 128 * 1024;
const MAX_RETAINED_SCHEMA_BYTES: usize = 4 * 1024 * 1024;
const IDLE_TTL: Duration = Duration::from_secs(10 * 60);

struct Entry {
    validator: Arc<jsonschema::Validator>,
    schema_bytes: usize,
    last_used: Instant,
}

#[derive(Default)]
struct ValidatorCache {
    entries: HashMap<[u8; 32], Entry>,
}

static CACHE: OnceLock<Mutex<ValidatorCache>> = OnceLock::new();

pub(in crate::gateway::execution::basis_points) fn release_expired_cache_entries() {
    // Destruction of compiled validators must not hold the request-path lock.
    drop(try_with_cache(&CACHE, |cache| {
        cache.take_expired(Instant::now())
    }));
}

pub(in crate::gateway::execution::basis_points) fn cache_memory_diagnostics(
) -> CacheMemoryDiagnostics {
    describe_cache(
        &CACHE,
        "schema_validators",
        MAX_RETAINED_SCHEMA_BYTES,
        "schema_input",
        |cache| {
            (
                cache.entries.len(),
                cache.entries.values().map(|entry| entry.schema_bytes).sum(),
            )
        },
    )
}

impl ValidatorCache {
    fn take_expired(&mut self, now: Instant) -> Vec<Entry> {
        self.entries
            .extract_if(|_, entry| now.saturating_duration_since(entry.last_used) >= IDLE_TTL)
            .map(|(_, entry)| entry)
            .collect()
    }

    fn get(&mut self, key: &[u8; 32], now: Instant) -> Option<Arc<jsonschema::Validator>> {
        let entry = self.entries.get_mut(key)?;
        if now.duration_since(entry.last_used) >= IDLE_TTL {
            return None;
        }
        entry.last_used = now;
        Some(Arc::clone(&entry.validator))
    }

    fn insert(&mut self, key: [u8; 32], entry: Entry, now: Instant) {
        self.entries
            .retain(|_, entry| now.duration_since(entry.last_used) < IDLE_TTL);
        let mut retained_bytes: usize = self.entries.values().map(|entry| entry.schema_bytes).sum();
        while self.entries.len() >= MAX_ENTRIES
            || retained_bytes + entry.schema_bytes > MAX_RETAINED_SCHEMA_BYTES
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                return;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                retained_bytes -= removed.schema_bytes;
            }
        }
        self.entries.insert(key, entry);
    }
}

pub(in crate::gateway::execution::basis_points) fn validator(
    schema: &Value,
) -> Result<Arc<jsonschema::Validator>, jsonschema::ValidationError<'_>> {
    let cache = CACHE.get_or_init(|| Mutex::new(ValidatorCache::default()));
    let serialized = serde_json::to_vec(schema)
        .ok()
        .filter(|bytes| bytes.len() <= MAX_SCHEMA_BYTES);
    let key = serialized
        .as_ref()
        .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
    if let Some(key) = &key {
        if let Some(validator) = crate::poison::mutex(cache).get(key, Instant::now()) {
            return Ok(validator);
        }
    }
    // Compile outside the lock so a slow schema cannot stall unrelated cache hits.
    // Offline compilation preserves local $defs without enabling file or network references.
    let compiled = Arc::new(jsonschema::options().offline().build(schema)?);
    if let (Some(key), Some(serialized)) = (key, serialized) {
        let mut cache = crate::poison::mutex(cache);
        let now = Instant::now();
        if let Some(validator) = cache.get(&key, now) {
            return Ok(validator);
        }
        cache.insert(
            key,
            Entry {
                validator: Arc::clone(&compiled),
                schema_bytes: serialized.len(),
                last_used: now,
            },
            now,
        );
    }
    Ok(compiled)
}

#[cfg(test)]
mod tests;
