use super::*;
use sha2::{Digest, Sha256};

/// Host-owned volatile conversation state. Clone the handles when replacing a
/// routing graph, not the conversations. A new host/process starts with empty
/// conversations; durable hosts may restore their protected agent-carrier key.
#[derive(Clone)]
pub struct RuntimeContinuationStores {
    pub(in crate::runtime) agent_messages: Arc<crate::protocol::agent_messages::AgentMessageCipher>,
    pub(in crate::runtime) adapters: Arc<Mutex<crate::AdapterContinuationStore>>,
    pub(in crate::runtime) native_responses: Arc<Mutex<crate::NativeResponsesReplayStore>>,
    pub(in crate::runtime) routes: Arc<Mutex<crate::scheduler::AffinityCache>>,
}

impl Default for RuntimeContinuationStores {
    fn default() -> Self {
        Self {
            agent_messages: Arc::default(),
            adapters: Arc::new(Mutex::new(crate::AdapterContinuationStore::default())),
            native_responses: Arc::new(Mutex::new(crate::NativeResponsesReplayStore::default())),
            routes: Arc::new(Mutex::new(crate::scheduler::AffinityCache::new(
                4_096,
                crate::RESPONSE_AFFINITY_TTL_MS,
            ))),
        }
    }
}

impl GatewayRuntime {
    pub(crate) fn agent_message_context(
        &self,
        local_key_id: &str,
    ) -> crate::protocol::agent_messages::AgentMessageContext {
        crate::protocol::agent_messages::AgentMessageContext::new(
            self.agent_message_cipher.clone(),
            local_key_id,
        )
    }
}

impl fmt::Debug for RuntimeContinuationStores {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeContinuationStores")
            .finish_non_exhaustive()
    }
}

impl RuntimeContinuationStores {
    /// Restore the host's durable, independently generated agent-carrier key.
    /// The host owns protected persistence; no messages or provider keys belong here.
    pub fn with_agent_message_key(key: [u8; 32]) -> Self {
        Self {
            agent_messages: Arc::new(
                crate::protocol::agent_messages::AgentMessageCipher::from_key(key),
            ),
            ..Self::default()
        }
    }

    /// Read already-accounted byte counts without cloning or serializing history.
    /// A busy store stays unavailable and is never pruned by a measurement.
    pub fn memory_diagnostics(&self) -> Vec<crate::gateway::CacheMemoryDiagnostics> {
        [
            (
                "adapter_continuations",
                self.adapters
                    .try_lock()
                    .ok()
                    .map(|store| store.memory_usage()),
            ),
            (
                "native_replays",
                self.native_responses
                    .try_lock()
                    .ok()
                    .map(|store| store.memory_usage()),
            ),
        ]
        .into_iter()
        .map(|(key, usage)| crate::gateway::CacheMemoryDiagnostics {
            key,
            entries: usage.map(|value| value.0),
            bytes: usage.map(|value| value.1),
            limit_bytes: usage.map(|value| value.2),
            basis: "serialized_state",
        })
        .collect()
    }
}

#[cfg(test)]
mod memory_tests {
    use super::*;

    #[test]
    fn continuation_memory_is_numeric_nonblocking_and_does_not_mutate_stores() {
        let stores = RuntimeContinuationStores::default();
        let snapshot = stores.memory_diagnostics();
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.iter().all(|entry| entry.bytes == Some(0)));
        let _busy = stores.adapters.lock().unwrap();
        let busy = stores.memory_diagnostics();
        assert_eq!(busy[0].bytes, None);
        assert_eq!(busy[1].bytes, Some(0));
    }
}

pub(super) mod request;

const CODEX_TURN_STATE_TTL_MS: u64 = 60 * 60 * 1_000;

#[derive(Default)]
pub(super) struct CodexTurnStateStore {
    origins: Mutex<BTreeMap<String, CodexTurnStateOrigin>>,
}

struct CodexTurnStateOrigin {
    expires_at_ms: u64,
    authorization: AuthorizationIncarnation,
}

pub(crate) struct CodexTurnStateScope<'a> {
    pub local_key_id: &'a str,
    pub session_id: &'a str,
    pub account_id: &'a str,
    pub model: &'a str,
    pub endpoint: &'a str,
}

impl CodexTurnStateStore {
    fn key(scope: &CodexTurnStateScope<'_>, state: &[u8], credential: &[u8]) -> Option<String> {
        if state.is_empty()
            || state.len() > 8192
            || credential.is_empty()
            || [
                scope.local_key_id,
                scope.session_id,
                scope.account_id,
                scope.model,
                scope.endpoint,
            ]
            .iter()
            .any(|session_id| session_id.is_empty())
        {
            return None;
        }
        let mut digest = Sha256::new();
        for part in [
            scope.local_key_id.as_bytes(),
            scope.session_id.as_bytes(),
            scope.account_id.as_bytes(),
            scope.model.as_bytes(),
            scope.endpoint.as_bytes(),
            state,
            credential,
        ] {
            digest.update((part.len() as u64).to_be_bytes());
            digest.update(part);
        }
        Some(hex::encode(digest.finalize()))
    }

    fn note(
        &self,
        scope: &CodexTurnStateScope<'_>,
        state: &[u8],
        credential: &[u8],
        authorization: &AuthorizationIncarnation,
        now_ms: u64,
    ) {
        let Some(key) = Self::key(scope, state, credential) else {
            return;
        };
        let mut origins = crate::poison::mutex(&self.origins);
        origins.retain(|_, origin| origin.expires_at_ms > now_ms);
        if origins.len() >= 4096
            && !origins.contains_key(&key)
            && let Some(oldest) = origins
                .iter()
                .min_by_key(|(_, origin)| origin.expires_at_ms)
                .map(|(key, _)| key.clone())
        {
            origins.remove(&oldest);
        }
        origins.insert(
            key,
            CodexTurnStateOrigin {
                expires_at_ms: now_ms.saturating_add(CODEX_TURN_STATE_TTL_MS),
                authorization: authorization.clone(),
            },
        );
    }

    fn contains(
        &self,
        scope: &CodexTurnStateScope<'_>,
        state: &[u8],
        credential: &[u8],
        authorization: &AuthorizationIncarnation,
        now_ms: u64,
    ) -> bool {
        let Some(key) = Self::key(scope, state, credential) else {
            return false;
        };
        let mut origins = crate::poison::mutex(&self.origins);
        let Some(origin) = origins.get(&key) else {
            return false;
        };
        if origin.expires_at_ms <= now_ms {
            origins.remove(&key);
            return false;
        }
        origin.authorization == *authorization
    }
}

impl GatewayRuntime {
    pub(crate) fn note_codex_turn_state(
        &self,
        scope: &CodexTurnStateScope<'_>,
        state: &[u8],
        credential: &[u8],
        authorization: &AuthorizationIncarnation,
        now_ms: u64,
    ) {
        self.codex_turn_state_store
            .note(scope, state, credential, authorization, now_ms);
    }

    pub(crate) fn codex_turn_state_matches(
        &self,
        scope: &CodexTurnStateScope<'_>,
        state: &[u8],
        credential: &[u8],
        authorization: &AuthorizationIncarnation,
        now_ms: u64,
    ) -> bool {
        self.codex_turn_state_store
            .contains(scope, state, credential, authorization, now_ms)
    }
}

mod affinity;

#[cfg(test)]
mod turn_state_tests {
    use super::*;

    #[test]
    fn turn_state_origin_blocks_cross_account_echo_until_expiry() {
        let store = CodexTurnStateStore::default();
        let scope = CodexTurnStateScope {
            local_key_id: "key",
            session_id: "thread",
            account_id: "account-a",
            model: "synthetic",
            endpoint: "https://example.invalid/backend-api/codex/responses",
        };
        let authorization = AuthorizationIncarnation::Source;
        store.note(&scope, b"state-a", b"credential", &authorization, 10);
        assert!(store.contains(&scope, b"state-a", b"credential", &authorization, 11));
        assert!(!store.contains(&scope, b"state-b", b"credential", &authorization, 11));
        assert!(!store.contains(&scope, b"state-a", b"refreshed", &authorization, 11));
        assert!(!store.contains(
            &CodexTurnStateScope {
                account_id: "account-b",
                ..scope
            },
            b"state-a",
            b"credential",
            &authorization,
            11
        ));
        assert!(!store.contains(
            &CodexTurnStateScope {
                model: "another",
                ..scope
            },
            b"state-a",
            b"credential",
            &authorization,
            11
        ));
        assert!(!store.contains(
            &CodexTurnStateScope {
                local_key_id: "other-key",
                ..scope
            },
            b"state-a",
            b"credential",
            &authorization,
            11
        ));
        assert!(!store.contains(
            &CodexTurnStateScope {
                session_id: "other-thread",
                ..scope
            },
            b"state-a",
            b"credential",
            &authorization,
            11
        ));
        assert!(!store.contains(
            &scope,
            b"state-a",
            b"credential",
            &authorization,
            10 + CODEX_TURN_STATE_TTL_MS
        ));
    }

    #[test]
    fn late_responses_do_not_reassign_another_turn_state() {
        let store = CodexTurnStateStore::default();
        let first = CodexTurnStateScope {
            local_key_id: "key",
            session_id: "thread",
            account_id: "a",
            model: "synthetic",
            endpoint: "https://example.invalid/backend-api/codex/responses",
        };
        let second = CodexTurnStateScope {
            account_id: "b",
            ..first
        };
        let authorization = AuthorizationIncarnation::Source;
        store.note(&second, b"new-state", b"new-credential", &authorization, 10);
        store.note(&first, b"old-state", b"old-credential", &authorization, 11);
        assert!(store.contains(&second, b"new-state", b"new-credential", &authorization, 12));
        assert!(!store.contains(&first, b"new-state", b"old-credential", &authorization, 12));
    }
}
