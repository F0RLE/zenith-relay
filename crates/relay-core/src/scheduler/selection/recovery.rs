use super::*;

impl PoolScheduler {
    pub fn record_success(&mut self, candidate_id: &str, model: &str, now_ms: u64) -> bool {
        let recovered = {
            let Some(candidate) = self.candidates.get_mut(candidate_id) else {
                return false;
            };
            candidate.cooldowns.retain(|candidate_model, retry_at_ms| {
                let applies = candidate_model == "*" || candidate_model.eq_ignore_ascii_case(model);
                !applies || *retry_at_ms > now_ms
            });
            candidate.last_used_at = Some(now_ms);
            !candidate
                .cooldowns
                .values()
                .any(|retry_at_ms| *retry_at_ms > now_ms)
        };
        if let Some(candidate) = self.candidates.get(candidate_id) {
            self.cooldown_metadata
                .retain(|(cooled_candidate, scope), _| {
                    cooled_candidate != candidate_id || candidate.cooldowns.contains_key(scope)
                });
        }
        recovered
    }

    pub fn set_cooldown(&mut self, candidate_id: &str, model: &str, retry_at_ms: u64) -> bool {
        self.set_cooldown_with_reason(candidate_id, model, retry_at_ms, CooldownReason::Mandatory)
    }

    pub(crate) fn provider_backoff_for(
        &self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
    ) -> Option<ProviderBackoff> {
        self.candidates
            .get(candidate_id)?
            .cooldowns
            .iter()
            .filter(|(scope, effective_deadline)| {
                **effective_deadline > now_ms
                    && (scope.as_str() == "*" || scope.eq_ignore_ascii_case(model))
            })
            .filter_map(|(scope, effective_deadline)| {
                let clock = self
                    .cooldown_metadata
                    .get(&(candidate_id.to_string(), scope.clone()))?
                    .provider_clock?;
                if clock.not_before_ms <= now_ms || clock.not_before_ms > *effective_deadline {
                    return None;
                }
                Some(ProviderBackoff {
                    scope: scope.clone(),
                    not_before_ms: clock.not_before_ms,
                    observed_at_ms: clock.observed_at_ms,
                })
            })
            .max_by_key(|backoff| (backoff.not_before_ms, backoff.scope == "*"))
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
                provider_clock: None,
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
        let existing_deadline = self
            .candidates
            .get(candidate_id)
            .and_then(|candidate| candidate.cooldowns.get(&scope).copied());
        let mut metadata = self
            .cooldown_metadata
            .get(&(candidate_id.to_string(), scope.clone()))
            .copied()
            .unwrap_or_default();
        let previous_reason = metadata.reason;
        let should_store_reason = existing_deadline.is_none_or(|current_deadline| {
            request.retry_at_ms > current_deadline
                || (request.retry_at_ms == current_deadline
                    && request.reason.is_rate_limit()
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
                .and_modify(|cooldown_deadline_ms| {
                    *cooldown_deadline_ms = (*cooldown_deadline_ms).max(request.retry_at_ms)
                })
                .or_insert(request.retry_at_ms);
        }
        if should_store_reason {
            metadata.reason = Some(request.reason);
        }
        metadata.provider_clock = ProviderClock::latest(
            metadata.provider_clock.into_iter().chain(
                request
                    .provider_clock
                    .filter(|clock| clock.not_before_ms <= request.retry_at_ms),
            ),
        );
        if metadata.reason.is_some() || metadata.provider_clock.is_some() {
            self.cooldown_metadata
                .insert((candidate_id.to_string(), scope.clone()), metadata);
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
            self.cooldown_metadata
                .retain(|(cooled_candidate, scope), _| {
                    cooled_candidate != candidate_id || !scope.eq_ignore_ascii_case(model)
                });
        }
        removed
    }
}
