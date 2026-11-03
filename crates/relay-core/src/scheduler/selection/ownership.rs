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
        if self.is_retired() || !self.candidates.contains_key(candidate_id) {
            return false;
        }
        self.prompt_affinity.bind(key, candidate_id, now_ms);
        true
    }

    /// Persist prompt affinity without allowing a temporary spillover
    /// candidate to become the new durable owner.  The existing owner is
    /// refreshed when the same candidate completes successfully. A successful
    /// material resource rebalance may replace it using the admission margins;
    /// temporary load and completion order alone never move the owner.
    pub fn bind_prompt_affinity_sticky(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        now_ms: u64,
    ) -> bool {
        if self.is_retired() || !self.candidates.contains_key(candidate_id) {
            return false;
        }
        let key = key.into();
        let move_owner = self
            .prompt_affinity
            .get(&key, now_ms)
            .is_some_and(|owner_id| {
                owner_id != candidate_id
                    && self
                        .rotation
                        .has_material_resource_lead(candidate_id, owner_id, now_ms)
            });
        if move_owner {
            self.prompt_affinity.bind(key, candidate_id, now_ms);
            return true;
        }
        let bound = self
            .prompt_affinity
            .bind_if_unbound_or_same(&key, candidate_id, now_ms);
        if !bound {
            // Keep the durable owner, but remember successful spillovers so a
            // busy owner does not send every continuation to another cold member.
            self.prompt_affinity
                .remember_alternative(&key, candidate_id, now_ms);
        }
        bound
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
        let key = key.into();
        if self.is_retired()
            || !self.candidates.contains_key(candidate_id)
            || expires_at_ms <= now_ms
            || self.prompt_affinity.contains(&key, now_ms)
        {
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
                && candidate.supports_protocol(model, allowed_protocols)
        })
    }

    pub fn has_prompt_affinity(&mut self, key: &str, now_ms: u64) -> bool {
        self.prompt_affinity.contains(key, now_ms)
    }

    pub(crate) fn prompt_affinity_snapshot(&self, key: &str, now_ms: u64) -> Option<(String, u64)> {
        self.prompt_affinity
            .get_with_expiry(key, now_ms)
            .map(|(candidate_id, expires_at_ms)| (candidate_id.to_owned(), expires_at_ms))
    }

    pub fn invalidate_response_affinity(&mut self, key: &str) -> bool {
        self.response_affinity.invalidate(key)
    }

    pub fn invalidate_prompt_affinity(&mut self, key: &str) -> bool {
        !self.is_retired() && self.prompt_affinity.invalidate(key)
    }

    pub(crate) fn invalidate_prompt_affinity_for_candidate(
        &mut self,
        key: &str,
        candidate_id: &str,
    ) -> bool {
        !self.is_retired() && self.prompt_affinity.invalidate_if_owner(key, candidate_id)
    }

    /// Reuse ordinary cache-aware admission for the current physical owner.
    /// The caller must materialize saved history before clearing
    /// its binding; this predicate never moves the request or spends a trial.
    pub(crate) fn automatic_response_owner_should_yield(
        &mut self,
        affinity_key: &str,
        model: &str,
        allowed_protocols: &[WireApi],
        scope: &CandidateScope,
        tried: &HashSet<String>,
        now_ms: u64,
    ) -> bool {
        if self.rotation.mode() != RotationMode::Automatic {
            return false;
        }
        let Some(owner_id) = self
            .response_affinity
            .get(affinity_key, now_ms)
            .map(str::to_owned)
        else {
            return false;
        };
        let probe = SelectionRequest {
            model,
            allowed_protocols,
            scope,
            tried,
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms,
        };
        let Some(mut request) = self.prepare_rotation_request(&probe, RotationOperation::Text)
        else {
            return false;
        };
        let Some(owner_member) = self.member_key_for(&owner_id) else {
            return false;
        };
        // The probe has no hard owner, so projection deduplicates protocol
        // aliases. Prefer the admitted route of the same physical member;
        // the saved opaque response binding itself must remain unchanged.
        request.preferred = request.allowed_candidates.as_ref().and_then(|allowed| {
            allowed
                .iter()
                .find(|candidate_id| {
                    self.member_key_for(candidate_id).as_ref() == Some(&owner_member)
                })
                .cloned()
        });
        self.rotation
            .select(&request, now_ms)
            .is_some_and(|selected| {
                !selected.recovery
                    && self
                        .member_key_for(&selected.candidate_id)
                        .is_some_and(|member| member != owner_member)
            })
    }
}
