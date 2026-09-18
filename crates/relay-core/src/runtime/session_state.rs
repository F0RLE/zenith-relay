use super::*;
use crate::{NativeResponsesReplayState, PROMPT_AFFINITY_TTL_MS, RESPONSE_AFFINITY_TTL_MS};
use serde_json::Value;
use sha2::{Digest, Sha256};

const CODEX_TURN_STATE_TTL_MS: u64 = 60 * 60 * 1_000;
const VOLATILE_RESPONSE_PREFIX: &str = "volatile-response:";

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

    /// Captures a completed native Responses turn as a bounded, materialized
    /// conversation. When the completed turn continues a previous response,
    /// fold the predecessor's replay state into it first. That keeps the
    /// next recovery independent of an opaque upstream response id, rather
    /// than retaining only the most recent user message.
    pub(crate) fn capture_native_responses_replay(
        &self,
        local_key_id: &str,
        candidate_id: &str,
        request: &Value,
        model: &str,
        upstream: &Value,
        now_ms: u64,
    ) {
        let materialized_request = request
            .get("previous_response_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|response_id| !response_id.is_empty())
            .and_then(|response_id| {
                self.load_native_responses_replay(local_key_id, response_id, candidate_id, now_ms)
            })
            .and_then(|previous| {
                previous
                    .replay_request(
                        request,
                        model,
                        request
                            .get("stream")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    )
                    .ok()
            })
            .unwrap_or_else(|| request.clone());
        let Some((response_id, state)) =
            NativeResponsesReplayState::from_response(&materialized_request, model, upstream)
        else {
            return;
        };
        self.native_responses_replay_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(local_key_id, &response_id, candidate_id, state, now_ms);
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

    pub(crate) fn tool_call_affinity_key(
        &self,
        local_key_id: &str,
        call_id: &str,
    ) -> Option<String> {
        let local_key_id = local_key_id.trim();
        let call_id = call_id.trim();
        if local_key_id.is_empty() || call_id.is_empty() || call_id.len() > 256 {
            return None;
        }
        Some(format!(
            "tool:{}",
            hex::encode(Sha256::digest(
                format!("tool\0{local_key_id}\0{call_id}").as_bytes(),
            ))
        ))
    }

    pub(crate) fn has_response_affinity_binding(&self, key: &str, now_ms: u64) -> bool {
        if self.lock_scheduler().has_response_affinity(key, now_ms) {
            return true;
        }
        self.response_affinity_store
            .as_ref()
            .and_then(|store| store.find(key, now_ms).ok().flatten())
            .is_some()
    }

    /// Returns the in-memory owner of a response continuation. This is used
    /// only to read the owner-scoped native replay state when that owner has
    /// left the active pool or was removed before the next turn arrives.
    pub(crate) fn response_affinity_candidate(&self, key: &str, now_ms: u64) -> Option<String> {
        self.lock_scheduler()
            .response_affinity_candidate(key, now_ms)
    }

    pub(crate) fn response_affinity_owner_supports_route(
        &self,
        key: &AuthenticatedKey,
        affinity_key: &str,
        model: &str,
        allowed_protocols: &[crate::WireApi],
        now_ms: u64,
    ) -> Option<bool> {
        let scope = key.scope_snapshot();
        self.lock_scheduler()
            .response_affinity_owner_supports_route(
                affinity_key,
                model,
                allowed_protocols,
                &scope,
                now_ms,
            )
    }

    pub(crate) fn response_affinity_owner_is_eligible(
        &self,
        key: &AuthenticatedKey,
        affinity_key: &str,
        model: &str,
        allowed_protocols: &[crate::WireApi],
        now_ms: u64,
    ) -> Option<bool> {
        let scope = key.scope_snapshot();
        self.lock_scheduler().response_affinity_owner_is_eligible(
            affinity_key,
            model,
            allowed_protocols,
            &scope,
            now_ms,
        )
    }

    pub(crate) fn response_affinity_owner_supports_model(
        &self,
        affinity_key: &str,
        model: &str,
        allowed_protocols: &[crate::WireApi],
        now_ms: u64,
    ) -> Option<bool> {
        self.lock_scheduler()
            .response_affinity_owner_supports_model(affinity_key, model, allowed_protocols, now_ms)
    }

    /// Release an optional tool binding after its owner is no longer eligible
    /// or leaves the request's configured routes. Callers must first establish
    /// that the input contains the full tool history and does not depend on an
    /// opaque response id.
    pub(crate) fn release_unroutable_response_affinity(
        &self,
        key: &AuthenticatedKey,
        affinity_key: &mut Option<String>,
        model: &str,
        allowed_protocols: &[crate::WireApi],
        now_ms: u64,
    ) -> bool {
        let owner_is_eligible = affinity_key.as_deref().and_then(|affinity_key| {
            self.response_affinity_owner_is_eligible(
                key,
                affinity_key,
                model,
                allowed_protocols,
                now_ms,
            )
        });
        let supports_route = affinity_key.as_deref().and_then(|affinity_key| {
            self.response_affinity_owner_supports_route(
                key,
                affinity_key,
                model,
                allowed_protocols,
                now_ms,
            )
        });
        if owner_is_eligible != Some(false) && supports_route != Some(false) {
            return false;
        }
        // Other branches may still need this owner or its cached replay.
        // Only this self-contained request releases the routing constraint.
        *affinity_key = None;
        true
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
            self.bind_affinity_key(&key, candidate_id, now_ms);
        }
    }

    /// Keeps an incomplete Responses turn on its current live WebSocket
    /// without writing an ownership record to durable storage. A completed
    /// response uses `bind_response_affinity`; an incomplete one may only be
    /// continued while that same client connection remains alive.
    pub(crate) fn bind_volatile_response_affinity(
        &self,
        response_id: Option<&str>,
        candidate_id: &str,
        request_id: &str,
        now_ms: u64,
    ) -> Option<String> {
        let response_key = self.response_affinity_key(response_id)?;
        let key = format!("{VOLATILE_RESPONSE_PREFIX}{request_id}:{response_key}");
        self.lock_scheduler()
            .bind_response_affinity(key.clone(), candidate_id, now_ms)
            .then_some(key)
    }

    pub(crate) fn bind_tool_call_affinity(
        &self,
        local_key_id: &str,
        call_id: &str,
        candidate_id: &str,
        now_ms: u64,
    ) {
        if let Some(key) = self.tool_call_affinity_key(local_key_id, call_id) {
            self.bind_affinity_key(&key, candidate_id, now_ms);
        }
    }

    fn bind_affinity_key(&self, key: &str, candidate_id: &str, now_ms: u64) {
        if self
            .lock_scheduler()
            .bind_response_affinity(key.to_string(), candidate_id, now_ms)
        {
            self.persist_response_affinity(key, candidate_id, now_ms);
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

    pub(crate) fn invalidate_prompt_affinity(&self, key: Option<&str>) -> bool {
        key.is_some_and(|key| {
            let invalidated = self.lock_scheduler().invalidate_prompt_affinity(key);
            if invalidated {
                if let Some(store) = self.response_affinity_store.as_ref() {
                    let _ = store.delete(key);
                }
            }
            invalidated
        })
    }

    pub(crate) fn persist_response_affinity(&self, key: &str, candidate_id: &str, now_ms: u64) {
        if key.starts_with(VOLATILE_RESPONSE_PREFIX) {
            return;
        }
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
