//! Content-addressed presentation only. Current request policy remains authoritative.
use super::super::schema::{parameter_schema, prompt_schema};
use super::super::{describe_cache, try_with_cache};
use super::ClientTool;
use crate::gateway::CacheMemoryDiagnostics;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_ENTRIES: usize = 1024;
const MAX_ENTRY_BYTES: usize = 512 * 1024;
const MAX_RETAINED_BYTES: usize = 16 * 1024 * 1024;
const IDLE_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug)]
pub(super) struct Presentation {
    compact: String,
    full: String,
    schema_bytes: u64,
    compact_schema_bytes: u64,
}

impl ClientTool {
    fn presentation(&self) -> &Presentation {
        self.presentation.get_or_init(|| {
            let mut line = format!("- {} ({})", self.key(), self.kind);
            if let Some(description) = self.spec.get("description").and_then(Value::as_str)
                && !description.is_empty()
            {
                line.push_str(": ");
                line.push_str(description);
            }
            if self.kind == "custom" {
                line.push_str(". It receives raw text in input.");
                if let Some(format) = self.spec.get("format").filter(|value| value.is_object()) {
                    line.push_str(" Input format: ");
                    line.push_str(&format.to_string());
                }
            } else if let Some(schema) = parameter_schema(self) {
                let full = schema.to_string();
                let compact = prompt_schema(schema);
                line.push_str(". Parameters: ");
                return Presentation {
                    compact: format!("{line}{compact}"),
                    full: format!("{line}{full}"),
                    schema_bytes: full.len() as u64,
                    compact_schema_bytes: compact.len() as u64,
                };
            }
            Presentation {
                compact: line.clone(),
                full: line,
                schema_bytes: parameter_schema(self)
                    .map(crate::tool_policy::serialized_bytes)
                    .unwrap_or_default(),
                compact_schema_bytes: parameter_schema(self)
                    .map(|schema| prompt_schema(schema).len() as u64)
                    .unwrap_or_default(),
            }
        })
    }

    pub(in crate::gateway::execution::basis_points) fn description(&self, full: bool) -> &str {
        let presentation = self.presentation();
        if full {
            &presentation.full
        } else {
            &presentation.compact
        }
    }

    pub(in crate::gateway::execution::basis_points) fn schema_bytes(&self, full: bool) -> u64 {
        if full {
            *self.schema_size.get_or_init(|| {
                self.presentation
                    .get()
                    .map(|value| value.schema_bytes)
                    .unwrap_or_else(|| {
                        parameter_schema(self)
                            .map(crate::tool_policy::serialized_bytes)
                            .unwrap_or_default()
                    })
            })
        } else {
            self.presentation().compact_schema_bytes
        }
    }
}

struct Entry {
    tool: ClientTool,
    charged_bytes: usize,
    last_used: Instant,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<[u8; 32], Entry>,
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

pub(in crate::gateway::execution::basis_points) fn release_expired_cache_entries() {
    drop(try_with_cache(&CACHE, |cache| {
        cache.take_expired(Instant::now())
    }));
}

pub(in crate::gateway::execution::basis_points) fn cache_memory_diagnostics()
-> CacheMemoryDiagnostics {
    describe_cache(
        &CACHE,
        "tool_presentations",
        MAX_RETAINED_BYTES,
        "allocation_estimate",
        |cache| {
            (
                cache.entries.len(),
                cache
                    .entries
                    .values()
                    .map(|entry| entry.charged_bytes)
                    .sum(),
            )
        },
    )
}

impl Cache {
    fn take_expired(&mut self, now: Instant) -> Vec<Entry> {
        self.entries
            .extract_if(|_, entry| now.saturating_duration_since(entry.last_used) >= IDLE_TTL)
            .map(|(_, entry)| entry)
            .collect()
    }

    fn get(&mut self, key: &[u8; 32], now: Instant) -> Option<ClientTool> {
        let entry = self.entries.get_mut(key)?;
        if now.saturating_duration_since(entry.last_used) >= IDLE_TTL {
            return None;
        }
        entry.last_used = now;
        Some(entry.tool.clone())
    }

    fn insert(&mut self, key: [u8; 32], entry: Entry, now: Instant) {
        if entry.charged_bytes > MAX_ENTRY_BYTES {
            return;
        }
        self.entries
            .retain(|_, entry| now.saturating_duration_since(entry.last_used) < IDLE_TTL);
        let mut retained: usize = self.entries.values().map(|entry| entry.charged_bytes).sum();
        while self.entries.len() >= MAX_ENTRIES
            || retained + entry.charged_bytes > MAX_RETAINED_BYTES
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                retained -= removed.charged_bytes;
            }
        }
        self.entries.insert(key, entry);
    }
}

pub(super) fn tool(
    name: &str,
    namespace: Option<&str>,
    kind: &str,
    spec: &Map<String, Value>,
) -> ClientTool {
    // Only declarations enter the key/cache, never history, arguments, credentials or policy.
    let bytes = serde_json::to_vec(spec).expect("JSON declaration serialization");
    let mut digest = Sha256::new();
    digest.update(b"bps-tool-presentation-v1");
    for part in [
        namespace.unwrap_or_default().as_bytes(),
        name.as_bytes(),
        kind.as_bytes(),
        bytes.as_slice(),
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    let key: [u8; 32] = digest.finalize().into();
    let cache = CACHE.get_or_init(|| Mutex::new(Cache::default()));
    if let Some(tool) = crate::poison::mutex(cache).get(&key, Instant::now()) {
        return tool;
    }
    let tool = ClientTool {
        name: name.to_owned(),
        namespace: namespace.map(str::to_owned),
        kind: kind.to_owned(),
        spec: Arc::new(spec.clone()),
        serialized_spec: Arc::from(std::str::from_utf8(&bytes).expect("serialized JSON is UTF-8")),
        schema_size: Arc::new(OnceLock::new()),
        schema_validated: Arc::new(OnceLock::new()),
        presentation: Arc::new(OnceLock::new()),
    };
    // Reserve for the parsed object plus both lazy presentations and string capacity.
    // This is conservative retained allocation accounting, not process RSS.
    let charged_bytes = crate::gateway::request_body::retained_object_bytes(&tool.spec)
        .saturating_add(bytes.len().saturating_mul(8))
        .saturating_add(4 * (name.len() + namespace.map_or(0, str::len) + kind.len()) + 1024);
    if charged_bytes <= MAX_ENTRY_BYTES {
        let now = Instant::now();
        let mut cache = crate::poison::mutex(cache);
        if let Some(existing) = cache.get(&key, now) {
            return existing;
        }
        cache.insert(
            key,
            Entry {
                tool: tool.clone(),
                charged_bytes,
                last_used: now,
            },
            now,
        );
    }
    tool
}

#[cfg(test)]
mod tests;
