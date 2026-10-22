//! Shared quota evidence feeds the existing per-candidate cooldown owner.

use super::*;

const OBSERVATION_TTL_MS: u64 = 10 * 60_000;
const MAX_OBSERVATIONS: usize = 4096;
const MIN_BACKOFF_MS: u64 = 5_000;
const MAX_BACKOFF_MS: u64 = 60_000;

#[derive(Clone, Debug)]
pub(super) struct RateLimitObservation {
    bucket: [u8; 32],
    observed_at_ms: u64,
    retry_at_ms: u64,
    backoff_ms: u64,
}

impl PoolScheduler {
    pub(crate) fn observe_shared_rate_limit(
        &mut self,
        candidate_id: &str,
        bucket: [u8; 32],
        mut request: CooldownRequest<'_>,
        now_ms: u64,
    ) {
        if !self.candidates.contains_key(candidate_id)
            || self.retired_candidates.contains(candidate_id)
        {
            return;
        }
        self.shared_rate_limits.retain(|_, observation| {
            now_ms.saturating_sub(observation.observed_at_ms) < OBSERVATION_TTL_MS
        });
        let model = request.scope.to_ascii_lowercase();
        let previous = self
            .shared_rate_limits
            .iter()
            .filter(|((_, scope), observation)| *scope == model && observation.bucket == bucket)
            .map(|(_, observation)| observation)
            .max_by_key(|observation| observation.retry_at_ms);
        let backoff_ms = previous.map_or(MIN_BACKOFF_MS, |observation| {
            if observation.retry_at_ms <= now_ms {
                observation.backoff_ms.saturating_mul(2)
            } else {
                observation.backoff_ms
            }
            .clamp(MIN_BACKOFF_MS, MAX_BACKOFF_MS)
        });
        request.retry_at_ms = request.retry_at_ms.max(now_ms.saturating_add(backoff_ms));
        if let Some(observation) = previous {
            request.retry_at_ms = request.retry_at_ms.max(observation.retry_at_ms);
        }
        let key = (candidate_id.to_string(), model.clone());
        if self.shared_rate_limits.len() >= MAX_OBSERVATIONS
            && !self.shared_rate_limits.contains_key(&key)
        {
            if let Some(oldest) = self
                .shared_rate_limits
                .iter()
                .min_by_key(|(_, observation)| observation.observed_at_ms)
                .map(|(key, _)| key.clone())
            {
                self.shared_rate_limits.remove(&oldest);
            }
        }
        self.shared_rate_limits.insert(
            key,
            RateLimitObservation {
                bucket,
                observed_at_ms: now_ms,
                retry_at_ms: request.retry_at_ms,
                backoff_ms,
            },
        );
        let mut members = Vec::new();
        for ((member, scope), observation) in &mut self.shared_rate_limits {
            if *scope == model && observation.bucket == bucket {
                observation.retry_at_ms = request.retry_at_ms;
                observation.backoff_ms = backoff_ms;
                members.push(member.clone());
            }
        }
        // Never infer membership from the connector type or an account's name.
        for member in members {
            self.set_cooldown_with_reason_for_model_at(&member, request);
        }
    }

    pub(crate) fn reset_shared_rate_limit_backoff(
        &mut self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
    ) {
        let key = (candidate_id.to_string(), model.to_ascii_lowercase());
        let Some(bucket) = self
            .shared_rate_limits
            .get(&key)
            .filter(|observation| observation.retry_at_ms <= now_ms)
            .map(|observation| observation.bucket)
        else {
            return;
        };
        for ((_, scope), observation) in &mut self.shared_rate_limits {
            if *scope == key.1 && observation.bucket == bucket {
                observation.backoff_ms = 0;
            }
        }
    }

    pub(super) fn forget_shared_rate_limits(&mut self, candidate_id: &str) {
        self.shared_rate_limits
            .retain(|(member, _), _| member != candidate_id);
    }
}
