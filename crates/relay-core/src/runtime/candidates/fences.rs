use super::super::{ExecutionFence, GatewayRuntime};
use crate::CandidateHealth;
use std::sync::atomic::AtomicBool;

impl GatewayRuntime {
    /// Immediately blocks sibling OAuth candidates that share the same
    /// ChatGPT Team/workspace identity. This is intentionally an in-memory
    /// circuit breaker; the owning local/server store persists the triggering
    /// request through the normal usage callback.
    pub(crate) fn trip_chatgpt_team_breaker(&self, candidate_id: &str, now_ms: u64) -> bool {
        let team_key = self
            .chatgpt_team_members
            .iter()
            .find_map(|(team, members)| members.contains(candidate_id).then_some(team.clone()));
        let Some(team_key) = team_key else {
            return false;
        };
        {
            let mut recent = self
                .chatgpt_team_breaker_recent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if recent.get(&team_key).is_some_and(|until| *until > now_ms) {
                return false;
            }
            recent.retain(|_, until| *until > now_ms);
            recent.insert(
                team_key.clone(),
                now_ms.saturating_add(super::super::CHATGPT_TEAM_BREAKER_DEDUP_MS),
            );
        }
        let siblings = self
            .chatgpt_team_members
            .get(&team_key)
            .into_iter()
            .flat_map(|members| members.iter())
            .filter(|member| member.as_str() != candidate_id)
            .cloned()
            .collect::<Vec<_>>();
        let mut changed = false;
        for sibling in &siblings {
            changed |= self.set_candidate_health(sibling, CandidateHealth::Blocked);
        }
        if let Ok(callback) = self.chatgpt_team_breaker_callback.lock() {
            callback(siblings.clone());
        }
        changed
    }

    /// Hold while a host commits a credential or permission edit and applies
    /// it to this runtime. Acquire before the durable edit; release only after
    /// the new runtime state is published or the old state is restored.
    pub fn fence_candidate_dispatch(&self, candidate_id: &str) -> Option<ExecutionFence> {
        let epoch = self.lock_scheduler().begin_execution_fence(candidate_id)?;
        self.candidate_availability.notify_waiters();
        Some(ExecutionFence {
            scheduler: self.scheduler.clone(),
            availability: self.candidate_availability.clone(),
            candidate_id: candidate_id.to_string(),
            epoch,
            released: AtomicBool::new(false),
        })
    }

    /// Fence every physical protocol route of one API source while its host
    /// changes the source's credential, endpoint or permission policy.
    pub fn fence_source_dispatch(&self, source_id: &str) -> Vec<ExecutionFence> {
        self.source_candidate_bindings
            .iter()
            .filter(|(_, binding)| binding.source_id == source_id)
            .filter_map(|(candidate_id, _)| self.fence_candidate_dispatch(candidate_id))
            .collect()
    }

    pub(crate) fn fence_execution(&self, candidate_id: &str) -> Option<ExecutionFence> {
        self.fence_candidate_dispatch(candidate_id)
    }
}
