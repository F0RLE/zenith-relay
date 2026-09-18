use super::*;

const FAILURE_PENALTY_DECAY_MS: u64 = 60_000;

impl PoolScheduler {
    pub fn record_success(&mut self, candidate_id: &str, model: &str, now_ms: u64) -> bool {
        self.record_success_with_metrics(candidate_id, model, now_ms, None, 0)
    }

    pub fn record_success_with_metrics(
        &mut self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
        _output_tokens: Option<u64>,
        _latency_ms: u64,
    ) -> bool {
        let (provider_key, recovered) = {
            let Some(candidate) = self.candidates.get_mut(candidate_id) else {
                return false;
            };
            let provider_key = (candidate.source_id.clone(), model.to_ascii_lowercase());
            candidate.cooldowns.retain(|candidate_model, retry_at_ms| {
                let applies = candidate_model == "*" || candidate_model.eq_ignore_ascii_case(model);
                !applies || *retry_at_ms > now_ms
            });
            candidate.last_used_at = Some(now_ms);
            let recovered = !candidate
                .cooldowns
                .values()
                .any(|retry_at_ms| *retry_at_ms > now_ms);
            if recovered {
                candidate.health = CandidateHealth::Healthy;
                candidate.consecutive_failures = 0;
                self.failure_observed_at.remove(candidate_id);
            }
            (provider_key, recovered)
        };
        if let Some(candidate) = self.candidates.get(candidate_id) {
            self.cooldown_reasons
                .retain(|(cooled_candidate, scope), _| {
                    cooled_candidate != candidate_id || candidate.cooldowns.contains_key(scope)
                });
        }
        self.provider_storm_breakers.remove(&provider_key);
        recovered
    }

    pub fn record_failure(&mut self, candidate_id: &str) -> Option<u32> {
        self.record_failure_at(candidate_id, crate::unix_time_ms())
    }

    pub fn record_failure_at(&mut self, candidate_id: &str, now_ms: u64) -> Option<u32> {
        let candidate = self.candidates.get_mut(candidate_id)?;
        candidate.consecutive_failures = candidate.consecutive_failures.saturating_add(1);
        self.failure_observed_at
            .insert(candidate_id.to_owned(), now_ms);
        Some(candidate.consecutive_failures)
    }

    pub fn reset_failures(&mut self, candidate_id: &str) -> bool {
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        candidate.consecutive_failures = 0;
        self.failure_observed_at.remove(candidate_id);
        true
    }

    pub fn set_cooldown(&mut self, candidate_id: &str, model: &str, retry_at_ms: u64) -> bool {
        self.set_cooldown_with_reason(candidate_id, model, retry_at_ms, CooldownReason::Transient)
    }

    pub(crate) fn set_cooldown_with_reason(
        &mut self,
        candidate_id: &str,
        model: &str,
        retry_at_ms: u64,
        reason: CooldownReason,
    ) -> bool {
        self.set_cooldown_with_reason_inner(
            candidate_id,
            CooldownRequest {
                scope: model,
                policy_model: model,
                allowed_protocols: &[],
                request_scope: &CandidateScope::default(),
                retry_at_ms,
                reason,
                now_ms: 0,
            },
            false,
        )
    }

    pub(crate) fn set_cooldown_with_reason_for_model_at(
        &mut self,
        candidate_id: &str,
        request: CooldownRequest<'_>,
    ) -> bool {
        self.set_cooldown_with_reason_inner(candidate_id, request, true)
    }

    fn set_cooldown_with_reason_inner(
        &mut self,
        candidate_id: &str,
        request: CooldownRequest<'_>,
        enforce_policy: bool,
    ) -> bool {
        if enforce_policy
            && request.reason == CooldownReason::Transient
            && !self.transient_cooldown_allowed(
                candidate_id,
                request.policy_model,
                request.allowed_protocols,
                request.request_scope,
                request.now_ms,
            )
        {
            return false;
        }
        let scope = if request.scope == "*" {
            "*".to_string()
        } else {
            request.scope.to_ascii_lowercase()
        };
        let previous = self
            .candidates
            .get(candidate_id)
            .and_then(|candidate| candidate.cooldowns.get(&scope).copied());
        let previous_reason = self
            .cooldown_reasons
            .get(&(candidate_id.to_string(), scope.clone()))
            .copied();
        let should_store_reason = previous.is_none_or(|current| {
            request.retry_at_ms > current
                || (request.retry_at_ms == current
                    && request.reason == CooldownReason::RateLimit
                    && previous_reason != Some(CooldownReason::Mandatory))
                || (request.reason == CooldownReason::Mandatory
                    && previous_reason != Some(CooldownReason::Mandatory))
        });
        {
            let Some(candidate) = self.candidates.get_mut(candidate_id) else {
                return false;
            };
            candidate
                .cooldowns
                .entry(scope.clone())
                .and_modify(|current| *current = (*current).max(request.retry_at_ms))
                .or_insert(request.retry_at_ms);
        }
        if should_store_reason {
            self.cooldown_reasons
                .insert((candidate_id.to_string(), scope.clone()), request.reason);
        }
        if previous.is_none_or(|current| request.retry_at_ms > current) {
            self.reservations.invalidate_probe(candidate_id, &scope);
        }
        true
    }

    fn transient_cooldown_allowed(
        &self,
        candidate_id: &str,
        model: &str,
        allowed_protocols: &[WireApi],
        request_scope: &CandidateScope,
        now_ms: u64,
    ) -> bool {
        let Some(candidate) = self.candidates.get(candidate_id) else {
            return false;
        };
        if self.automatic_recovery_enabled() {
            return true;
        }
        if candidate.consecutive_failures < self.cooldown_after_failures {
            return false;
        }
        if !self.keep_last_candidate_available {
            return true;
        }
        !self.is_last_applicable_candidate(
            candidate_id,
            model,
            allowed_protocols,
            request_scope,
            now_ms,
        )
    }

    fn is_last_applicable_candidate(
        &self,
        candidate_id: &str,
        model: &str,
        allowed_protocols: &[WireApi],
        request_scope: &CandidateScope,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        self.candidates
            .values()
            .filter(|candidate| {
                self.is_eligible(candidate, model, allowed_protocols, request_scope, now_ms)
            })
            .map(unified::member_key)
            .collect::<BTreeSet<_>>()
            .len()
            <= 1
    }

    pub fn clear_cooldown(&mut self, candidate_id: &str, model: &str) -> bool {
        let removed = self
            .candidates
            .get_mut(candidate_id)
            .map(|candidate| {
                let previous_len = candidate.cooldowns.len();
                candidate
                    .cooldowns
                    .retain(|candidate_model, _| !candidate_model.eq_ignore_ascii_case(model));
                candidate.cooldowns.len() != previous_len
            })
            .unwrap_or(false);
        if removed {
            self.cooldown_reasons
                .retain(|(cooled_candidate, scope), _| {
                    cooled_candidate != candidate_id || !scope.eq_ignore_ascii_case(model)
                });
        }
        removed
    }

    pub(super) fn recent_failure_penalty(&self, candidate: &RuntimeCandidate, now_ms: u64) -> u32 {
        let Some(observed_at) = self.failure_observed_at.get(&candidate.id) else {
            // Persisted counts without an observation time are not fresh evidence.
            return 0;
        };
        let remaining =
            FAILURE_PENALTY_DECAY_MS.saturating_sub(now_ms.saturating_sub(*observed_at));
        let penalty = u64::from(candidate.consecutive_failures.min(4)) * 500;
        (penalty * remaining / FAILURE_PENALTY_DECAY_MS) as u32
    }
}
