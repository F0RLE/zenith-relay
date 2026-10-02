use super::*;
use sha2::{Digest, Sha256};

const CODEX_TURN_STATE_TTL_MS: u64 = 60 * 60 * 1_000;

#[derive(Default)]
pub(super) struct CodexTurnStateStore {
    origins: Mutex<BTreeMap<String, u64>>,
}

pub(crate) struct CodexTurnStateScope<'a> {
    pub local_key_id: &'a str,
    pub session_id: &'a str,
    pub account_id: &'a str,
    pub model: &'a str,
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
            ]
            .iter()
            .any(|value| value.is_empty())
        {
            return None;
        }
        let mut digest = Sha256::new();
        for part in [
            scope.local_key_id.as_bytes(),
            scope.session_id.as_bytes(),
            scope.account_id.as_bytes(),
            scope.model.as_bytes(),
            state,
            credential,
        ] {
            digest.update((part.len() as u64).to_be_bytes());
            digest.update(part);
        }
        Some(hex::encode(digest.finalize()))
    }

    fn note(&self, scope: &CodexTurnStateScope<'_>, state: &[u8], credential: &[u8], now_ms: u64) {
        let Some(key) = Self::key(scope, state, credential) else {
            return;
        };
        let mut origins = crate::poison::mutex(&self.origins);
        origins.retain(|_, expires| *expires > now_ms);
        if origins.len() >= 4096 && !origins.contains_key(&key) {
            if let Some(oldest) = origins
                .iter()
                .min_by_key(|(_, expires)| *expires)
                .map(|(key, _)| key.clone())
            {
                origins.remove(&oldest);
            }
        }
        origins.insert(key, now_ms.saturating_add(CODEX_TURN_STATE_TTL_MS));
    }

    fn contains(
        &self,
        scope: &CodexTurnStateScope<'_>,
        state: &[u8],
        credential: &[u8],
        now_ms: u64,
    ) -> bool {
        let Some(key) = Self::key(scope, state, credential) else {
            return false;
        };
        let mut origins = crate::poison::mutex(&self.origins);
        let Some(expires) = origins.get(&key) else {
            return false;
        };
        if *expires <= now_ms {
            origins.remove(&key);
            return false;
        }
        true
    }
}

impl GatewayRuntime {
    pub(crate) fn note_codex_turn_state(
        &self,
        scope: &CodexTurnStateScope<'_>,
        state: &[u8],
        credential: &[u8],
        now_ms: u64,
    ) {
        self.codex_turn_state_store
            .note(scope, state, credential, now_ms);
    }

    pub(crate) fn codex_turn_state_matches(
        &self,
        scope: &CodexTurnStateScope<'_>,
        state: &[u8],
        credential: &[u8],
        now_ms: u64,
    ) -> bool {
        self.codex_turn_state_store
            .contains(scope, state, credential, now_ms)
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
        };
        store.note(&scope, b"state-a", b"credential", 10);
        assert!(store.contains(&scope, b"state-a", b"credential", 11));
        assert!(!store.contains(&scope, b"state-b", b"credential", 11));
        assert!(!store.contains(&scope, b"state-a", b"refreshed", 11));
        assert!(!store.contains(
            &CodexTurnStateScope {
                account_id: "account-b",
                ..scope
            },
            b"state-a",
            b"credential",
            11
        ));
        assert!(!store.contains(
            &CodexTurnStateScope {
                model: "another",
                ..scope
            },
            b"state-a",
            b"credential",
            11
        ));
        assert!(!store.contains(
            &CodexTurnStateScope {
                local_key_id: "other-key",
                ..scope
            },
            b"state-a",
            b"credential",
            11
        ));
        assert!(!store.contains(
            &CodexTurnStateScope {
                session_id: "other-thread",
                ..scope
            },
            b"state-a",
            b"credential",
            11
        ));
        assert!(!store.contains(
            &scope,
            b"state-a",
            b"credential",
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
        };
        let second = CodexTurnStateScope {
            account_id: "b",
            ..first
        };
        store.note(&second, b"new-state", b"new-credential", 10);
        store.note(&first, b"old-state", b"old-credential", 11);
        assert!(store.contains(&second, b"new-state", b"new-credential", 12));
        assert!(!store.contains(&first, b"new-state", b"old-credential", 12));
    }
}
