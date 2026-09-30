//! Reservation ownership from dispatch through settlement and cancellation.

use super::*;

mod lease;
use crate::scheduler::rotation::{
    AttemptId as RotationAttemptId, AttemptObservation as RotationAttemptObservation,
    DispatchStartError as RotationDispatchStartError,
    ExecutionObservation as RotationExecutionObservation,
    HealthObservation as RotationHealthObservation, RotationSettlement,
    SettlementError as RotationSettlementError, SharedRequestBudget,
};

pub(crate) struct CandidateLease {
    pub(super) scheduler: Arc<Mutex<PoolScheduler>>,
    pub(super) hidden_models: Arc<RwLock<BTreeSet<String>>>,
    pub(super) availability: Arc<tokio::sync::Notify>,
    pub(super) candidate_id: String,
    pub(super) candidate_permission_revision: u64,
    pub(super) member_key: String,
    pub(super) principal_scope: Arc<RwLock<CandidateScope>>,
    pub(super) principal_scope_revision: (Arc<AtomicU64>, u64),
    pub(super) model: String,
    pub(super) allowed_protocols: Vec<WireApi>,
    /// A bound opaque continuation must still belong to this candidate when
    /// its pending lease finally reaches the wire. Soft prompt affinity is
    /// deliberately excluded: it is a preference, not an owner constraint.
    pub(super) response_owner: Option<(String, u64)>,
    /// Captured under the scheduler lock for a virtual OAuth image route.
    pub(super) image_bridge_revision: Option<(Arc<AtomicU64>, u64)>,
    pub(super) reservation_id: crate::scheduler::ReservationId,
    pub(super) rotation_budget: SharedRequestBudget,
    pub(super) rotation_started: AtomicBool,
    pub(super) rotation_settled: AtomicBool,
    pub(super) activity_callback: Arc<Mutex<RuntimeActivityCallback>>,
    pub(super) activity_runtime_id: u64,
    pub(super) activity_revision: Arc<AtomicU64>,
    pub(super) released: AtomicBool,
}

/// Blocks new reservations and pending final dispatches until dropped. An
/// already started attempt is allowed to settle; no provider work is canceled.
pub struct ExecutionFence {
    pub(super) scheduler: Arc<Mutex<PoolScheduler>>,
    pub(super) availability: Arc<tokio::sync::Notify>,
    pub(super) candidate_id: String,
    pub(super) epoch: u64,
    pub(super) released: AtomicBool,
}

#[derive(Clone, Copy)]
pub(super) enum CandidateLeaseLane {
    Text,
    Image,
}

impl GatewayRuntime {
    #[cfg(test)]
    pub(crate) fn set_cooldown_with_reason_for_model_at(
        &self,
        candidate_id: &str,
        request: CooldownRequest<'_>,
    ) -> bool {
        let mut scheduler = self.lock_scheduler();
        self.apply_cooldown_locked(&mut scheduler, candidate_id, request)
    }

    pub(crate) fn settle_rotation_failure(
        &self,
        lease: &CandidateLease,
        observation: crate::scheduler::rotation::AttemptObservation,
        cooldown: Option<CooldownRequest<'_>>,
        now_ms: u64,
    ) {
        let _ = lease.settle_rotation_with(observation, now_ms, |scheduler| {
            if let Some(request) = cooldown {
                self.apply_cooldown_locked(scheduler, lease.candidate_id(), request);
            }
        });
    }

    fn apply_cooldown_locked(
        &self,
        scheduler: &mut PoolScheduler,
        candidate_id: &str,
        request: CooldownRequest<'_>,
    ) -> bool {
        let applied = scheduler.set_cooldown_with_reason_for_model_at(candidate_id, request);
        if applied {
            if let Some(binding) = self.source_candidate_bindings.get(candidate_id) {
                let resource_failure = request.reason
                    == crate::scheduler::CooldownReason::RateLimit
                    || (request.scope == "*"
                        && request.reason == crate::scheduler::CooldownReason::Mandatory);
                let upstream = binding.adapter.upstream_protocol(binding.wire_api);
                for (id, sibling) in &self.source_candidate_bindings {
                    if id != candidate_id
                        && sibling.source_id == binding.source_id
                        && (resource_failure
                            || sibling.adapter.upstream_protocol(sibling.wire_api) == upstream)
                    {
                        scheduler.set_cooldown_with_reason(
                            id,
                            request.scope,
                            request.retry_at_ms,
                            request.reason,
                        );
                    }
                }
            }
        }
        applied
    }

    pub(crate) fn failure_state_for(
        &self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
    ) -> (u32, Option<(String, u64)>) {
        let scheduler = self.lock_scheduler();
        let (streak, circuit_deadline) = scheduler.circuit_state_for(candidate_id, model);
        let cooldown = scheduler
            .candidate(candidate_id)
            .into_iter()
            .flat_map(|candidate| &candidate.cooldowns)
            .filter(|(scope, deadline)| {
                (scope.as_str() == "*" || scope.eq_ignore_ascii_case(model)) && **deadline > now_ms
            })
            .map(|(scope, deadline)| (scope.clone(), *deadline))
            .chain(
                circuit_deadline
                    .filter(|at| *at > now_ms)
                    .map(|at| (model.to_owned(), at)),
            )
            // Equal circuit and mandatory deadlines still describe a global
            // block when the provider scoped it to the whole member.
            .max_by_key(|(scope, at)| (*at, scope == "*"));
        (streak, cooldown)
    }
}

impl Drop for CandidateLease {
    fn drop(&mut self) {
        self.release();
    }
}

impl Drop for ExecutionFence {
    fn drop(&mut self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        self.scheduler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .end_execution_fence(&self.candidate_id, self.epoch);
        self.availability.notify_waiters();
    }
}

#[cfg(test)]
mod tests;
