use super::*;
use crate::{NativeResponsesReplayState, RESPONSE_AFFINITY_TTL_MS};
use sha2::{Digest, Sha256};

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
        self.messages_bridge_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                local_key_id,
                &response.response_id,
                candidate_id,
                response.continuation.clone(),
                now_ms,
            );
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
    ) -> Option<String> {
        let prompt_cache_key = prompt_cache_key?.trim();
        if prompt_cache_key.is_empty() {
            return None;
        }
        Some(hex::encode(Sha256::digest(
            format!(
                "prompt\0{}\0{}\0{}",
                local_key_id,
                model.to_ascii_lowercase(),
                prompt_cache_key
            )
            .as_bytes(),
        )))
    }

    pub(crate) fn bind_prompt_affinity(&self, key: Option<&str>, candidate_id: &str, now_ms: u64) {
        if let Some(key) = key {
            self.lock_scheduler()
                .bind_prompt_affinity(key, candidate_id, now_ms);
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
}
