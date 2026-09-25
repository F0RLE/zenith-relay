//! Bounded response ownership and optional prompt-cache affinity.

use super::*;

impl PoolScheduler {
    pub fn bind_response_affinity(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        self.response_affinity.bind(key, candidate_id, now_ms);
        true
    }

    pub fn bind_prompt_affinity(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        self.prompt_affinity.bind(key, candidate_id, now_ms);
        true
    }

    /// Persist prompt affinity without allowing a temporary spillover
    /// candidate to become the new durable owner.  The existing owner is
    /// refreshed when the same candidate completes successfully and can only
    /// be replaced after it has been invalidated by the failure path.
    pub fn bind_prompt_affinity_sticky(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        self.prompt_affinity
            .bind_if_unbound_or_same(key, candidate_id, now_ms)
    }

    pub fn restore_response_affinity(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        expires_at_ms: u64,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) || expires_at_ms <= now_ms {
            return false;
        }
        self.response_affinity
            .restore(key, candidate_id, expires_at_ms, now_ms);
        true
    }

    pub fn restore_prompt_affinity(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        expires_at_ms: u64,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) || expires_at_ms <= now_ms {
            return false;
        }
        self.prompt_affinity
            .restore(key, candidate_id, expires_at_ms, now_ms);
        true
    }

    pub fn has_response_affinity(&mut self, key: &str, now_ms: u64) -> bool {
        self.response_affinity.contains(key, now_ms)
    }

    pub(crate) fn response_affinity_candidate(&mut self, key: &str, now_ms: u64) -> Option<String> {
        self.response_affinity.get(key, now_ms).map(str::to_string)
    }

    pub(crate) fn response_affinity_binding(
        &mut self,
        key: &str,
        now_ms: u64,
    ) -> Option<(String, u64)> {
        self.response_affinity
            .get_with_revision(key, now_ms)
            .map(|(candidate_id, revision)| (candidate_id.to_owned(), revision))
    }

    /// Returns whether the current affinity owner can structurally serve this
    /// route. Health, quota, capacity, and cooldown state are intentionally
    /// excluded: they are temporary and must not discard an opaque response
    /// continuation.
    pub(crate) fn response_affinity_owner_supports_route(
        &mut self,
        key: &str,
        model: &str,
        allowed_protocols: &[WireApi],
        request_scope: &CandidateScope,
        now_ms: u64,
    ) -> Option<bool> {
        let candidate_id = self.response_affinity.get(key, now_ms)?;
        self.candidates.get(candidate_id).map(|candidate| {
            // Pool membership and per-candidate policy are structural route
            // constraints. If the affinity owner left the key scope, the
            // opaque continuation cannot be sent there and the caller may
            // safely reset it before selecting another provider. Temporary
            // health, quota, capacity, and cooldown state remain excluded so
            // those conditions continue to wait for the original owner.
            candidate.is_configured(model, allowed_protocols, request_scope)
        })
    }

    /// Returns whether the current affinity owner is eligible for a new
    /// optional request. Unlike `response_affinity_owner_supports_route`,
    /// this includes mutable health, quota, and cooldown state. Callers must
    /// only use it when the request does not carry an opaque continuation.
    pub(crate) fn response_affinity_owner_is_eligible(
        &mut self,
        key: &str,
        model: &str,
        allowed_protocols: &[WireApi],
        request_scope: &CandidateScope,
        now_ms: u64,
    ) -> Option<bool> {
        let candidate_id = self.response_affinity.get(key, now_ms)?;
        self.candidates.get(candidate_id).map(|candidate| {
            self.is_eligible(candidate, model, allowed_protocols, request_scope, now_ms)
        })
    }

    /// Returns whether the affinity owner still matches the model and wire
    /// contract, without considering the caller's mutable pool scope. This
    /// distinction lets the request layer reset a model switch immediately,
    /// while giving an owner that merely left the pool a chance to replay its
    /// bounded local continuation first.
    pub(crate) fn response_affinity_owner_supports_model(
        &mut self,
        key: &str,
        model: &str,
        allowed_protocols: &[WireApi],
        now_ms: u64,
    ) -> Option<bool> {
        let candidate_id = self.response_affinity.get(key, now_ms)?;
        self.candidates.get(candidate_id).map(|candidate| {
            candidate.supports_model(model)
                && candidate.model_rules.allows(model)
                && allowed_protocols.contains(&candidate.protocol)
        })
    }

    pub fn has_prompt_affinity(&mut self, key: &str, now_ms: u64) -> bool {
        self.prompt_affinity.contains(key, now_ms)
    }

    pub fn invalidate_response_affinity(&mut self, key: &str) -> bool {
        self.response_affinity.invalidate(key)
    }

    pub fn invalidate_prompt_affinity(&mut self, key: &str) -> bool {
        self.prompt_affinity.invalidate(key)
    }
}
