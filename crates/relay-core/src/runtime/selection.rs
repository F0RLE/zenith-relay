use super::*;
use crate::{scheduler::CooldownReason, Selection, SelectionRequest};
use std::time::Duration;
use tokio::time::sleep;

impl GatewayRuntime {
    /// Waits for either a pool mutation or the next known cooldown to expire.
    /// The bounded poll prevents a missed `Notify` wake-up from turning a
    /// persistent ChatGPT request into a hot loop while still allowing
    /// cooldown-only recovery without another external mutation.
    pub(crate) async fn wait_for_candidate_availability(
        &self,
        retry_at_ms: Option<u64>,
        backoff: Duration,
        deadline: Option<tokio::time::Instant>,
    ) -> bool {
        let notified = self.candidate_availability.notified();
        let delay = retry_at_ms
            .map(|retry_at| retry_at.saturating_sub(crate::unix_time_ms()))
            .map(Duration::from_millis)
            .map(|delay| delay.min(Duration::from_secs(1)))
            .unwrap_or_else(|| Duration::from_secs(1))
            .min(backoff);
        if let Some(deadline) = deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            // A cooldown can elapse in the short gap between scheduler
            // selection and this wait. That is an immediate retry, not an
            // expired request window; otherwise a bounded request can fail at
            // the exact moment its only candidate becomes eligible.
            if delay.is_zero() {
                return true;
            }
            let delay = delay.min(remaining);
            return tokio::select! {
                _ = notified => true,
                _ = sleep(delay) => tokio::time::Instant::now() < deadline,
            };
        }
        // In persistent mode an already-expired cooldown must not be treated
        // as a deadline. Fall back to the bounded poll interval instead;
        // otherwise an `earliest_retry_at` equal to `now` would terminate the
        // supposedly unbounded wait immediately.
        let delay = if delay.is_zero() {
            backoff
                .min(Duration::from_secs(1))
                .max(Duration::from_millis(1))
        } else {
            delay
        };
        tokio::select! {
            _ = notified => true,
            _ = sleep(delay) => true,
        }
    }

    pub(crate) async fn select_and_reserve(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        affinity_keys: (Option<&str>, Option<&str>),
        now_ms: u64,
    ) -> Option<(Selection, CandidateLease)> {
        let (response_affinity_key, prompt_affinity_key) = affinity_keys;
        self.try_select_and_reserve_for(
            key,
            model,
            allowed_protocols,
            tried,
            response_affinity_key,
            prompt_affinity_key,
            now_ms,
            CandidateLeaseLane::Text,
        )
        .0
    }

    pub(crate) fn select_and_reserve_image(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        now_ms: u64,
    ) -> Option<(Selection, CandidateLease)> {
        self.try_select_and_reserve_for(
            key,
            model,
            allowed_protocols,
            tried,
            None,
            None,
            now_ms,
            CandidateLeaseLane::Image,
        )
        .0
    }

    #[allow(clippy::too_many_arguments)]
    fn try_select_and_reserve_for(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        response_affinity_key: Option<&str>,
        prompt_affinity_key: Option<&str>,
        now_ms: u64,
        lane: CandidateLeaseLane,
    ) -> (Option<(Selection, CandidateLease)>, bool) {
        if let (Some(key), Some(store)) =
            (response_affinity_key, self.response_affinity_store.as_ref())
        {
            let cached = self.lock_scheduler().has_response_affinity(key, now_ms);
            if !cached {
                if let Ok(Some(binding)) = store.find(key, now_ms) {
                    self.lock_scheduler().restore_response_affinity(
                        binding.key,
                        &binding.candidate_id,
                        binding.expires_at_ms,
                        now_ms,
                    );
                }
            }
        }
        if let (Some(key), Some(store)) =
            (prompt_affinity_key, self.response_affinity_store.as_ref())
        {
            let cached = self.lock_scheduler().has_prompt_affinity(key, now_ms);
            if !cached {
                if let Ok(Some(binding)) = store.find(key, now_ms) {
                    self.lock_scheduler().restore_prompt_affinity(
                        binding.key,
                        &binding.candidate_id,
                        binding.expires_at_ms,
                        now_ms,
                    );
                }
            }
        }
        // Keep authorization live through selection and reservation. A pool
        // mutation waits for this read lock, so it cannot race a stale scope
        // into a newly reserved lease.
        let scope = key.scope_read();
        let mut scheduler = self.lock_scheduler();
        let selection = match lane {
            CandidateLeaseLane::Text => scheduler.select(SelectionRequest {
                model,
                allowed_protocols,
                scope: &scope,
                tried,
                response_affinity_key,
                prompt_affinity_key,
                now_ms,
            }),
            CandidateLeaseLane::Image => scheduler.select_image(SelectionRequest {
                model,
                allowed_protocols,
                scope: &scope,
                tried,
                response_affinity_key,
                prompt_affinity_key,
                now_ms,
            }),
        };
        let reserved = selection.and_then(|selection| {
            let reserved = match lane {
                CandidateLeaseLane::Text => {
                    scheduler.reserve_for(&selection.candidate_id, model, now_ms)
                }
                CandidateLeaseLane::Image => {
                    scheduler.reserve_image_for(&selection.candidate_id, model, now_ms)
                }
            };
            reserved.then(|| {
                let lease = CandidateLease {
                    scheduler: self.scheduler.clone(),
                    availability: self.candidate_availability.clone(),
                    candidate_id: selection.candidate_id.clone(),
                    model: model.to_string(),
                    lane,
                    activity_callback: self.activity_callback.clone(),
                    activity_revision: self.activity_revision.clone(),
                    released: AtomicBool::new(false),
                };
                (selection, lease)
            })
        });
        let activity = reserved.as_ref().map(|(selection, _)| {
            let (in_flight, active_request_count, active_models) =
                scheduler.runtime_activity_for(&selection.candidate_id);
            RuntimeActivitySnapshot {
                revision: self.activity_revision.fetch_add(1, Ordering::AcqRel) + 1,
                candidate_id: selection.candidate_id.clone(),
                in_flight,
                active_request_count,
                active_models,
            }
        });
        drop(scheduler);
        drop(scope);
        if let Some(activity) = activity {
            self.emit_activity_changed(activity);
        }
        if let (Some((selection, _)), Some(key)) = (reserved.as_ref(), response_affinity_key) {
            if selection.response_affinity_hit {
                self.persist_response_affinity(key, &selection.candidate_id, now_ms);
            }
        }
        (reserved, false)
    }

    pub(crate) fn earliest_retry_at(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        response_affinity_key: Option<&str>,
        now_ms: u64,
    ) -> Option<u64> {
        let scope = key.scope_snapshot();
        self.lock_scheduler().earliest_retry_at(SelectionRequest {
            model,
            allowed_protocols,
            scope: &scope,
            tried,
            response_affinity_key,
            prompt_affinity_key: None,
            now_ms,
        })
    }

    pub(crate) fn all_applicable_cooldown(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        allowed_protocols: &[WireApi],
        tried: &HashSet<String>,
        response_affinity_key: Option<&str>,
        now_ms: u64,
    ) -> Option<(u64, CooldownReason)> {
        let scope = key.scope_snapshot();
        self.lock_scheduler()
            .all_applicable_cooldown(SelectionRequest {
                model,
                allowed_protocols,
                scope: &scope,
                tried,
                response_affinity_key,
                prompt_affinity_key: None,
                now_ms,
            })
    }
}
