use super::SharedRequestBudget;
use crate::accounts::TokenDispatchRevision;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::time::Duration;
use tokio::time::Instant;

/// A request-local preference following a proven pre-execution rejection.
/// Continuation ownership and every physical dispatch keep their existing fences.
#[derive(Clone)]
pub(crate) struct RetryCandidate {
    pub(crate) candidate_id: String,
    pub(crate) model: String,
    pub(crate) permission_revision: u64,
    pub(crate) token_revision: Option<TokenDispatchRevision>,
    pub(crate) not_before: Instant,
    pub(crate) deadline: Instant,
    pub(crate) allow_fallback: bool,
    active: bool,
    failures: u32,
}

impl std::fmt::Debug for RetryCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RetryCandidate")
            .field("active", &self.active)
            .field("failures", &self.failures)
            .finish_non_exhaustive()
    }
}

impl SharedRequestBudget {
    pub(crate) fn retry_candidate(&self) -> Option<RetryCandidate> {
        crate::poison::mutex(&self.retry_candidate)
            .as_ref()
            .filter(|candidate| candidate.active)
            .cloned()
    }

    pub(crate) fn retry_candidate_deadline(&self) -> Option<Instant> {
        crate::poison::mutex(&self.retry_candidate)
            .as_ref()
            .filter(|candidate| candidate.active)
            .map(|candidate| candidate.deadline)
    }

    pub(crate) fn finish_candidate_retry(&self) {
        if let Some(candidate) = crate::poison::mutex(&self.retry_candidate).as_mut() {
            candidate.active = false;
        }
    }

    pub(crate) fn abandon_candidate_retry(&self, allow_fallback: bool) {
        *crate::poison::mutex(&self.retry_candidate) = None;
        if !allow_fallback {
            self.with_budget(|budget| budget.replay_stopped = true);
        }
    }

    pub(crate) fn prefer_retry_candidate(
        &self,
        candidate_id: &str,
        model: &str,
        permission_revision: u64,
        token_revision: Option<TokenDispatchRevision>,
        allow_fallback: bool,
        window_ms: u64,
    ) {
        if !self.can_dispatch() || self.is_image_generation() {
            return;
        }
        let now = Instant::now();
        let request_id = self.request_id();
        let mut retained = crate::poison::mutex(&self.retry_candidate);
        let previous = retained.as_ref().filter(|candidate| {
            candidate.candidate_id == candidate_id
                && candidate.model == model
                && candidate.permission_revision == permission_revision
        });
        let failures = previous.map_or(1, |candidate| candidate.failures.saturating_add(1));
        let deadline = previous.map_or_else(
            || now + Duration::from_millis(window_ms.min(30_000)),
            |candidate| candidate.deadline,
        );
        let jitter = RandomState::new().hash_one((request_id.0, candidate_id, failures)) % 251;
        let delay_ms = 1_000_u64
            .saturating_mul(1_u64 << failures.saturating_sub(1).min(5))
            .min(30_000)
            .saturating_add(jitter);
        *retained = Some(RetryCandidate {
            candidate_id: candidate_id.into(),
            model: model.into(),
            permission_revision,
            token_revision,
            not_before: now + Duration::from_millis(delay_ms),
            deadline,
            allow_fallback,
            active: true,
            failures,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefer(budget: &SharedRequestBudget, allow_fallback: bool) {
        budget.prefer_retry_candidate("candidate", "model", 1, None, allow_fallback, 30_000);
    }

    #[tokio::test(start_paused = true)]
    async fn handoff_preserves_backoff_deadline_and_physical_budget() {
        let budget = SharedRequestBudget::for_incoming_request(4);
        assert!(budget.start_dispatch().is_some());
        prefer(&budget, true);
        let first = budget.retry_candidate().unwrap();
        assert!((1_000..=1_250).contains(&(first.not_before - Instant::now()).as_millis()));
        budget.finish_candidate_retry();
        assert!(budget.retry_candidate().is_none());
        tokio::time::advance(Duration::from_secs(2)).await;
        let handoff = budget.clone();
        assert!(handoff.start_dispatch().is_some());
        prefer(&handoff, true);
        let second = handoff.retry_candidate().unwrap();
        assert_eq!(second.deadline, first.deadline);
        assert!((2_000..=2_250).contains(&(second.not_before - Instant::now()).as_millis()));
        assert_eq!(budget.dispatches(), 2);
        handoff.finish_candidate_retry();
        tokio::time::advance(Duration::from_secs(31)).await;
        prefer(&handoff, true);
        assert_eq!(handoff.retry_candidate_deadline(), Some(first.deadline));
    }

    #[tokio::test]
    async fn image_and_exhausted_requests_cannot_gain_an_extra_dispatch() {
        let image = SharedRequestBudget::for_incoming_request(4);
        image.mark_image_generation();
        prefer(&image, true);
        assert!(image.retry_candidate().is_none());
        let exhausted = SharedRequestBudget::for_incoming_request(1);
        assert!(exhausted.start_dispatch().is_some());
        prefer(&exhausted, true);
        assert!(exhausted.retry_candidate().is_none());
        for execution in [
            super::super::ExecutionObservation::unknown(),
            super::super::ExecutionObservation::committed(),
        ] {
            let started = SharedRequestBudget::for_incoming_request(4);
            started.with_budget(|budget| budget.observe_execution(execution));
            prefer(&started, true);
            assert!(started.retry_candidate().is_none());
        }
    }

    #[tokio::test]
    async fn a_nonportable_route_cannot_fall_back_when_its_wait_ends() {
        let strict = SharedRequestBudget::for_incoming_request(4);
        prefer(&strict, false);
        strict.abandon_candidate_retry(false);
        assert!(!strict.can_dispatch());
        let portable = SharedRequestBudget::for_incoming_request(4);
        prefer(&portable, true);
        portable.abandon_candidate_retry(true);
        assert!(portable.can_dispatch());
        assert!(portable.retry_candidate().is_none());
    }
}
