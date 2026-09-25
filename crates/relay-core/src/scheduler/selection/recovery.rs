use super::*;

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
        let recovered = {
            let Some(candidate) = self.candidates.get_mut(candidate_id) else {
                return false;
            };
            candidate.cooldowns.retain(|candidate_model, retry_at_ms| {
                let applies = candidate_model == "*" || candidate_model.eq_ignore_ascii_case(model);
                !applies || *retry_at_ms > now_ms
            });
            candidate.last_used_at = Some(now_ms);
            let recovered = !candidate
                .cooldowns
                .values()
                .any(|retry_at_ms| *retry_at_ms > now_ms);
            recovered
        };
        if let Some(candidate) = self.candidates.get(candidate_id) {
            self.cooldown_reasons
                .retain(|(cooled_candidate, scope), _| {
                    cooled_candidate != candidate_id || candidate.cooldowns.contains_key(scope)
                });
        }
        recovered
    }

    pub fn set_cooldown(&mut self, candidate_id: &str, model: &str, retry_at_ms: u64) -> bool {
        self.set_cooldown_with_reason(candidate_id, model, retry_at_ms, CooldownReason::Mandatory)
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

                retry_at_ms,
                reason,
            },
        )
    }

    pub(crate) fn set_cooldown_with_reason_for_model_at(
        &mut self,
        candidate_id: &str,
        request: CooldownRequest<'_>,
    ) -> bool {
        self.set_cooldown_with_reason_inner(candidate_id, request)
    }

    fn set_cooldown_with_reason_inner(
        &mut self,
        candidate_id: &str,
        request: CooldownRequest<'_>,
    ) -> bool {
        // Transient health is reduced exactly once by the dispatched rotation
        // lease. This map stores only provider/mandatory rate deadlines.
        if request.reason == CooldownReason::Transient {
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
        true
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
}
