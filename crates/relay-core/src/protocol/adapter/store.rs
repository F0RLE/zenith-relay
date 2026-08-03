use super::contracts::{
    AdapterError, AdapterResult, MessagesBridgeState, NativeResponsesReplayState,
};
use std::collections::BTreeMap;
#[derive(Clone, Debug)]
struct StoredBridgeState {
    state: MessagesBridgeState,
    candidate_id: String,
    observed_at_ms: u64,
}

/// Bounded, in-memory state for bridge continuations. Losing this state during
/// a restart is surfaced as a clear continuation error instead of silently
/// sending a context-free tool output upstream.
#[derive(Debug)]
pub struct MessagesBridgeStore {
    entries: BTreeMap<(String, String), StoredBridgeState>,
    max_entries: usize,
    ttl_ms: u64,
}

impl Default for MessagesBridgeStore {
    fn default() -> Self {
        Self::new(256, 60 * 60 * 1_000)
    }
}

#[derive(Clone, Debug)]
struct StoredNativeResponsesReplay {
    state: NativeResponsesReplayState,
    candidate_id: String,
    observed_at_ms: u64,
}

/// Bounded, in-memory state for native Responses HTTP replay. It is never
/// serialized into request logs, diagnostics, or the persisted local store.
#[derive(Debug)]
pub struct NativeResponsesReplayStore {
    entries: BTreeMap<(String, String), StoredNativeResponsesReplay>,
    max_entries: usize,
    ttl_ms: u64,
}

impl Default for NativeResponsesReplayStore {
    fn default() -> Self {
        Self::new(256, 60 * 60 * 1_000)
    }
}

impl NativeResponsesReplayStore {
    pub const fn new(max_entries: usize, ttl_ms: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            ttl_ms,
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
        if self.max_entries == 0 {
            return;
        }
        while self.entries.len() >= self.max_entries {
            let Some(key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.observed_at_ms)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.entries.remove(&key);
        }
        self.entries.insert(
            (local_key_id.to_string(), response_id.to_string()),
            StoredNativeResponsesReplay {
                state,
                candidate_id: candidate_id.to_string(),
                observed_at_ms: now_ms,
            },
        );
    }

    fn prune(&mut self, now_ms: u64) {
        self.entries
            .retain(|_, entry| now_ms.saturating_sub(entry.observed_at_ms) <= self.ttl_ms);
    }
}

impl MessagesBridgeStore {
    pub const fn new(max_entries: usize, ttl_ms: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            ttl_ms,
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
        if self.max_entries == 0 {
            return;
        }
        while self.entries.len() >= self.max_entries {
            let Some(key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.observed_at_ms)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.entries.remove(&key);
        }
        self.entries.insert(
            (local_key_id.to_string(), response_id.to_string()),
            StoredBridgeState {
                state,
                candidate_id: candidate_id.to_string(),
                observed_at_ms: now_ms,
            },
        );
    }

    fn prune(&mut self, now_ms: u64) {
        self.entries
            .retain(|_, entry| now_ms.saturating_sub(entry.observed_at_ms) <= self.ttl_ms);
    }
}
