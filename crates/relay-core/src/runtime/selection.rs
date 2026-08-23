use super::*;
use crate::{scheduler::CooldownReason, Selection, SelectionRequest};

impl GatewayRuntime {
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
                    released: AtomicBool::new(false),
                };
                (selection, lease)
            })
        });
        drop(scheduler);
        drop(scope);
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
