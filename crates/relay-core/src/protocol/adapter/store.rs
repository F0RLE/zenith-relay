use super::contracts::{
    AdapterError, AdapterResult, MessagesBridgeState, NativeResponsesReplayState,
};
use serde::Serialize;
use std::collections::BTreeMap;

const DEFAULT_MAX_ENTRIES: usize = 64;
const DEFAULT_MAX_ENTRY_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
struct StoredBridgeState {
    state: MessagesBridgeState,
    candidate_id: String,
    observed_at_ms: u64,
    size_bytes: usize,
}

/// Bounded, in-memory state for bridge continuations. Losing this state during
/// a restart is surfaced as a clear continuation error instead of silently
/// sending a context-free tool output upstream.
#[derive(Debug)]
pub struct MessagesBridgeStore {
    entries: BTreeMap<(String, String), StoredBridgeState>,
    max_entries: usize,
    ttl_ms: u64,
    max_entry_bytes: usize,
    max_total_bytes: usize,
    total_bytes: usize,
}

impl Default for MessagesBridgeStore {
    fn default() -> Self {
        Self::with_limits(
            DEFAULT_MAX_ENTRIES,
            60 * 60 * 1_000,
            DEFAULT_MAX_ENTRY_BYTES,
            DEFAULT_MAX_TOTAL_BYTES,
        )
    }
}

#[derive(Clone, Debug)]
struct StoredNativeResponsesReplay {
    state: NativeResponsesReplayState,
    candidate_id: String,
    observed_at_ms: u64,
    size_bytes: usize,
}

/// Bounded, in-memory state for native Responses HTTP replay. It is never
/// serialized into request logs, diagnostics, or the persisted local store.
#[derive(Debug)]
pub struct NativeResponsesReplayStore {
    entries: BTreeMap<(String, String), StoredNativeResponsesReplay>,
    max_entries: usize,
    ttl_ms: u64,
    max_entry_bytes: usize,
    max_total_bytes: usize,
    total_bytes: usize,
}

impl Default for NativeResponsesReplayStore {
    fn default() -> Self {
        Self::with_limits(
            DEFAULT_MAX_ENTRIES,
            60 * 60 * 1_000,
            DEFAULT_MAX_ENTRY_BYTES,
            DEFAULT_MAX_TOTAL_BYTES,
        )
    }
}

impl NativeResponsesReplayStore {
    pub const fn new(max_entries: usize, ttl_ms: u64) -> Self {
        Self::with_limits(
            max_entries,
            ttl_ms,
            DEFAULT_MAX_ENTRY_BYTES,
            max_entries.saturating_mul(DEFAULT_MAX_ENTRY_BYTES),
        )
    }

    pub const fn with_limits(
        max_entries: usize,
        ttl_ms: u64,
        max_entry_bytes: usize,
        max_total_bytes: usize,
    ) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            ttl_ms,
            max_entry_bytes,
            max_total_bytes,
            total_bytes: 0,
        }
    }

    pub fn get(
        &mut self,
        local_key_id: &str,
        response_id: &str,
        candidate_id: &str,
        now_ms: u64,
    ) -> Option<NativeResponsesReplayState> {
        self.prune(now_ms);
        self.entries
            .get(&(local_key_id.to_string(), response_id.to_string()))
            .filter(|entry| entry.candidate_id == candidate_id)
            .map(|entry| entry.state.clone())
    }

    pub fn insert(
        &mut self,
        local_key_id: &str,
        response_id: &str,
        candidate_id: &str,
        state: NativeResponsesReplayState,
        now_ms: u64,
    ) {
        self.prune(now_ms);
        let Some(size_bytes) = serialized_size_bytes(&state) else {
            return;
        };
        if self.max_entries == 0
            || self.max_entry_bytes == 0
            || self.max_total_bytes == 0
            || size_bytes > self.max_entry_bytes
            || size_bytes > self.max_total_bytes
        {
            return;
        }
        let key = (local_key_id.to_string(), response_id.to_string());
        if let Some(existing) = self.entries.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(existing.size_bytes);
        }
        while self.entries.len() >= self.max_entries
            || self.total_bytes.saturating_add(size_bytes) > self.max_total_bytes
        {
            if !self.evict_oldest() {
                return;
            }
        }
        self.entries.insert(
            key,
            StoredNativeResponsesReplay {
                state,
                candidate_id: candidate_id.to_string(),
                observed_at_ms: now_ms,
                size_bytes,
            },
        );
        self.total_bytes = self.total_bytes.saturating_add(size_bytes);
    }

    fn prune(&mut self, now_ms: u64) {
        self.entries
            .retain(|_, entry| now_ms.saturating_sub(entry.observed_at_ms) <= self.ttl_ms);
        self.recalculate_total_bytes();
    }

    fn evict_oldest(&mut self) -> bool {
        let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.observed_at_ms)
            .map(|(key, _)| key.clone())
        else {
            return false;
        };
        if let Some(entry) = self.entries.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.size_bytes);
        }
        true
    }

    fn recalculate_total_bytes(&mut self) {
        self.total_bytes = self.entries.values().fold(0_usize, |total, entry| {
            total.saturating_add(entry.size_bytes)
        });
    }
}

impl MessagesBridgeStore {
    pub const fn new(max_entries: usize, ttl_ms: u64) -> Self {
        Self::with_limits(
            max_entries,
            ttl_ms,
            DEFAULT_MAX_ENTRY_BYTES,
            max_entries.saturating_mul(DEFAULT_MAX_ENTRY_BYTES),
        )
    }

    pub const fn with_limits(
        max_entries: usize,
        ttl_ms: u64,
        max_entry_bytes: usize,
        max_total_bytes: usize,
    ) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            ttl_ms,
            max_entry_bytes,
            max_total_bytes,
            total_bytes: 0,
        }
    }

    pub fn get(
        &mut self,
        local_key_id: &str,
        response_id: &str,
        candidate_id: &str,
        now_ms: u64,
    ) -> AdapterResult<MessagesBridgeState> {
        self.prune(now_ms);
        let key = (local_key_id.to_string(), response_id.to_string());
        let Some(entry) = self.entries.get(&key) else {
            return Err(AdapterError::continuation_missing());
        };
        if entry.candidate_id != candidate_id {
            return Err(AdapterError::continuation_mismatch());
        }
        Ok(entry.state.clone())
    }

    pub fn insert(
        &mut self,
        local_key_id: &str,
        response_id: &str,
        candidate_id: &str,
        state: MessagesBridgeState,
        now_ms: u64,
    ) {
        self.prune(now_ms);
        let Some(size_bytes) = serialized_size_bytes(&state) else {
            return;
        };
        if self.max_entries == 0
            || self.max_entry_bytes == 0
            || self.max_total_bytes == 0
            || size_bytes > self.max_entry_bytes
            || size_bytes > self.max_total_bytes
        {
            return;
        }
        let key = (local_key_id.to_string(), response_id.to_string());
        if let Some(existing) = self.entries.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(existing.size_bytes);
        }
        while self.entries.len() >= self.max_entries
            || self.total_bytes.saturating_add(size_bytes) > self.max_total_bytes
        {
            if !self.evict_oldest() {
                return;
            }
        }
        self.entries.insert(
            key,
            StoredBridgeState {
                state,
                candidate_id: candidate_id.to_string(),
                observed_at_ms: now_ms,
                size_bytes,
            },
        );
        self.total_bytes = self.total_bytes.saturating_add(size_bytes);
    }

    fn prune(&mut self, now_ms: u64) {
        self.entries
            .retain(|_, entry| now_ms.saturating_sub(entry.observed_at_ms) <= self.ttl_ms);
        self.recalculate_total_bytes();
    }

    fn evict_oldest(&mut self) -> bool {
        let Some(key) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.observed_at_ms)
            .map(|(key, _)| key.clone())
        else {
            return false;
        };
        if let Some(entry) = self.entries.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.size_bytes);
        }
        true
    }

    fn recalculate_total_bytes(&mut self) {
        self.total_bytes = self.entries.values().fold(0_usize, |total, entry| {
            total.saturating_add(entry.size_bytes)
        });
    }
}

fn serialized_size_bytes(value: &impl Serialize) -> Option<usize> {
    serde_json::to_vec(value)
        .ok()
        .map(|serialized| serialized.len())
}
