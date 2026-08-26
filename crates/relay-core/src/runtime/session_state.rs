use super::*;
use crate::{NativeResponsesReplayState, PROMPT_AFFINITY_TTL_MS, RESPONSE_AFFINITY_TTL_MS};
use sha2::{Digest, Sha256};

const CODEX_TURN_STATE_TTL_MS: u64 = 60 * 60 * 1_000;

#[derive(Default)]
pub(super) struct CodexTurnStateStore {
    origins: Mutex<BTreeMap<String, CodexTurnStateOrigin>>,
    writes: AtomicU64,
}

#[derive(Clone)]
struct CodexTurnStateOrigin {
    account_id: String,
    expires_at_ms: u64,
}

impl CodexTurnStateStore {
    fn key(local_key_id: &str, session_id: &str) -> Option<String> {
        let local_key_id = local_key_id.trim();
        let session_id = session_id.trim();
        if local_key_id.is_empty() || session_id.is_empty() {
            return None;
        }
        Some(hex::encode(Sha256::digest(
            format!("codex-turn-state\0{local_key_id}\0{session_id}").as_bytes(),
        )))
    }

    fn note(&self, local_key_id: &str, session_id: &str, account_id: &str, now_ms: u64) {
        let Some(key) = Self::key(local_key_id, session_id) else {
            return;
        };
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return;
        }
        self.origins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                key,
                CodexTurnStateOrigin {
                    account_id: account_id.to_string(),
                    expires_at_ms: now_ms.saturating_add(CODEX_TURN_STATE_TTL_MS),
                },
            );
        if self.writes.fetch_add(1, Ordering::Relaxed) % 256 == 255 {
            self.sweep(now_ms);
        }
    }

    fn belongs_to_account(
        &self,
        local_key_id: &str,
        session_id: &str,
        account_id: &str,
        now_ms: u64,
    ) -> bool {
        let Some(key) = Self::key(local_key_id, session_id) else {
            return false;
        };
        let mut origins = self
            .origins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(origin) = origins.get(&key) else {
            return false;
        };
        if origin.expires_at_ms <= now_ms {
            origins.remove(&key);
            return false;
        }
        origin.account_id == account_id.trim()
    }

    fn sweep(&self, now_ms: u64) {
        self.origins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, origin| origin.expires_at_ms > now_ms);
    }
}

impl GatewayRuntime {
    pub(crate) fn note_codex_turn_state(
        &self,
        local_key_id: &str,
        session_id: &str,
        account_id: &str,
        now_ms: u64,
    ) {
        self.codex_turn_state_store
            .note(local_key_id, session_id, account_id, now_ms);
    }

    pub(crate) fn codex_turn_state_owned_by_account(
        &self,
        local_key_id: &str,
        session_id: &str,
        account_id: &str,
        now_ms: u64,
    ) -> bool {
        self.codex_turn_state_store
            .belongs_to_account(local_key_id, session_id, account_id, now_ms)
    }
}

impl GatewayRuntime {
    pub(crate) fn load_messages_bridge_state(
        &self,
        local_key_id: &str,
        response_id: &str,
        candidate_id: &str,
        now_ms: u64,
    ) -> crate::AdapterResult<crate::MessagesBridgeState> {
        self.messages_bridge_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(local_key_id, response_id, candidate_id, now_ms)
    }

    pub(crate) fn save_messages_bridge_response(
        &self,
        local_key_id: &str,
        candidate_id: &str,
        response: &crate::MessagesBridgeResponse,
        now_ms: u64,
    ) {
        let stored = self
            .messages_bridge_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert_if_stored(
                local_key_id,
                &response.response_id,
                candidate_id,
                response.continuation.clone(),
                now_ms,
            );
        if stored {
            self.bind_response_affinity(Some(&response.response_id), candidate_id, now_ms);
        }
    }

    pub(crate) fn load_native_responses_replay(
        &self,
        local_key_id: &str,
        response_id: &str,
        candidate_id: &str,
        now_ms: u64,
    ) -> Option<NativeResponsesReplayState> {
        self.native_responses_replay_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(local_key_id, response_id, candidate_id, now_ms)
    }

    pub(crate) fn save_native_responses_replay(
        &self,
        local_key_id: &str,
        candidate_id: &str,
        response_id: &str,
        state: NativeResponsesReplayState,
        now_ms: u64,
    ) {
        self.native_responses_replay_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(local_key_id, response_id, candidate_id, state, now_ms);
    }

    pub(crate) fn response_affinity_key(&self, response_id: Option<&str>) -> Option<String> {
        let response_id = response_id?.trim();
        if response_id.is_empty() {
            return None;
        }
        Some(hex::encode(Sha256::digest(
            format!("response\0{response_id}").as_bytes(),
        )))
    }

    pub(crate) fn prompt_affinity_key(
        &self,
        local_key_id: &str,
        model: &str,
        prompt_cache_key: Option<&str>,
        client_context_id: Option<&str>,
    ) -> Option<String> {
        // Codex normally supplies prompt_cache_key. Older clients do not, but
        // still send a privacy-safe session/thread fingerprint. Keep that
        // session on the successful candidate so provider-side prefix caches
        // survive normal key rotation; explicit cache keys remain stronger.
        let (material_kind, material) = prompt_cache_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| ("cache", value))
            .or_else(|| {
                client_context_id
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| ("session", value))
            })?;
        let digest = hex::encode(Sha256::digest(
            format!(
                "prompt\0{}\0{}\0{}\0{}",
                local_key_id,
                model.to_ascii_lowercase(),
                material_kind,
                material,
            )
            .as_bytes(),
        ));
        // Keep the material class in the opaque scheduler key so selection
        // can give explicit provider cache keys a stronger owner preference
        // without changing normal session-based rotation.
        Some(format!(
            "{}{}",
            if material_kind == "cache" {
                "cache:"
            } else {
                "session:"
            },
            digest
        ))
    }

    pub(crate) fn bind_prompt_affinity(&self, key: Option<&str>, candidate_id: &str, now_ms: u64) {
        if let Some(key) = key {
            if self
                .lock_scheduler()
                .bind_prompt_affinity_sticky(key, candidate_id, now_ms)
            {
                self.persist_prompt_affinity(key, candidate_id, now_ms);
            }
        }
    }

    pub(crate) fn bind_response_affinity(
        &self,
        response_id: Option<&str>,
        candidate_id: &str,
        now_ms: u64,
    ) {
        if let Some(key) = self.response_affinity_key(response_id) {
            if self
                .lock_scheduler()
                .bind_response_affinity(key.clone(), candidate_id, now_ms)
            {
                self.persist_response_affinity(&key, candidate_id, now_ms);
            }
        }
    }

    pub(crate) fn invalidate_response_affinity(&self, key: Option<&str>) -> bool {
        key.is_some_and(|key| {
            let invalidated = self.lock_scheduler().invalidate_response_affinity(key);
            if invalidated {
                if let Some(store) = self.response_affinity_store.as_ref() {
                    let _ = store.delete(key);
                }
            }
            invalidated
        })
    }

    pub(crate) fn persist_response_affinity(&self, key: &str, candidate_id: &str, now_ms: u64) {
        if let Some(store) = self.response_affinity_store.as_ref() {
            let _ = store.upsert(&ResponseAffinityBinding {
                key: key.to_string(),
                candidate_id: candidate_id.to_string(),
                expires_at_ms: now_ms.saturating_add(RESPONSE_AFFINITY_TTL_MS),
            });
        }
    }

    pub(crate) fn persist_prompt_affinity(&self, key: &str, candidate_id: &str, now_ms: u64) {
        if let Some(store) = self.response_affinity_store.as_ref() {
            let _ = store.upsert(&ResponseAffinityBinding {
                key: key.to_string(),
                candidate_id: candidate_id.to_string(),
                expires_at_ms: now_ms.saturating_add(PROMPT_AFFINITY_TTL_MS),
            });
        }
    }
}

#[cfg(test)]
mod turn_state_tests {
    use super::*;

    #[test]
    fn turn_state_origin_blocks_cross_account_echo_until_expiry() {
        let store = CodexTurnStateStore::default();
        store.note("key", "thread", "account-a", 10);
        assert!(store.belongs_to_account("key", "thread", "account-a", 11));
        assert!(!store.belongs_to_account("key", "thread", "account-b", 11));
        assert!(!store.belongs_to_account(
            "key",
            "thread",
            "account-b",
            10 + CODEX_TURN_STATE_TTL_MS,
        ));
        assert!(!store.belongs_to_account("key", "unknown", "account-a", 11));
    }
}
