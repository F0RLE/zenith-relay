//! Reservation ownership from dispatch through settlement and cancellation.

use super::*;
use crate::scheduler::rotation::{
    AttemptId as RotationAttemptId, AttemptObservation as RotationAttemptObservation,
    DispatchStartError as RotationDispatchStartError,
    ExecutionObservation as RotationExecutionObservation,
    HealthObservation as RotationHealthObservation, RotationSettlement,
    SettlementError as RotationSettlementError, SharedRequestBudget,
};

pub(crate) struct CandidateLease {
    pub(super) scheduler: Arc<Mutex<PoolScheduler>>,
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

impl CandidateLease {
    pub(crate) fn candidate_id(&self) -> &str {
        &self.candidate_id
    }

    pub(crate) fn has_dispatched(&self) -> bool {
        self.rotation_started.load(Ordering::Acquire)
    }

    /// The final scope/config/availability gate and the generation debit are
    /// one scheduler transaction. A lost race never consumes a generation.
    #[cfg(test)]
    pub(crate) fn begin_rotation_dispatch(
        &self,
    ) -> std::result::Result<RotationAttemptId, RotationDispatchStartError> {
        self.begin_rotation_dispatch_with_wire(false, || Some(()))
    }

    pub(crate) fn begin_rotation_dispatch_for(
        &self,
        prepared: &PreparedAuthorization,
        runtime: &GatewayRuntime,
    ) -> std::result::Result<RotationAttemptId, RotationDispatchStartError> {
        self.begin_rotation_dispatch_with_wire(false, || {
            prepared.dispatch_guard(runtime, &self.candidate_id)
        })
    }

    /// HTTP has no separate handshake: a rejected per-dispatch fence must
    /// consume neither its wire-attempt allowance nor a generation. WebSocket
    /// charges its connection attempt separately before sending the payload.
    #[cfg(test)]
    pub(crate) fn begin_rotation_http_dispatch(
        &self,
    ) -> std::result::Result<RotationAttemptId, RotationDispatchStartError> {
        self.begin_rotation_dispatch_with_wire(true, || Some(()))
    }

    pub(crate) fn begin_rotation_http_dispatch_for(
        &self,
        prepared: &PreparedAuthorization,
        runtime: &GatewayRuntime,
    ) -> std::result::Result<RotationAttemptId, RotationDispatchStartError> {
        self.begin_rotation_dispatch_with_wire(true, || {
            prepared.dispatch_guard(runtime, &self.candidate_id)
        })
    }

    fn begin_rotation_dispatch_with_wire<G>(
        &self,
        charge_wire: bool,
        guard_authorization: impl FnOnce() -> Option<G>,
    ) -> std::result::Result<RotationAttemptId, RotationDispatchStartError> {
        let budget = &self.rotation_budget;
        let result = budget.with_budget(|request_budget| {
            if charge_wire && !request_budget.can_start_wire() {
                return Err(RotationDispatchStartError::BudgetExhausted);
            }
            let scope = self
                .principal_scope
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.principal_scope_revision.0.load(Ordering::Acquire)
                != self.principal_scope_revision.1
            {
                return Err(RotationDispatchStartError::CandidateChanged);
            }
            // Keep the credential read lock until after scheduler admission
            // and both budget debits. Replacement cannot linearize between
            // authorization validation and the generation start.
            let _authorization =
                guard_authorization().ok_or(RotationDispatchStartError::CandidateChanged)?;
            let mut scheduler = self
                .scheduler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self
                .image_bridge_revision
                .as_ref()
                .is_some_and(|(revision, captured)| revision.load(Ordering::Acquire) != *captured)
            {
                return Err(RotationDispatchStartError::CandidateChanged);
            }
            if self.response_owner.as_ref().is_some_and(|(key, revision)| {
                scheduler
                    .response_affinity_binding(key, runtime_now_ms())
                    .is_none_or(|(owner, current)| {
                        owner != self.candidate_id || current != *revision
                    })
            }) {
                return Err(RotationDispatchStartError::CandidateChanged);
            }
            if scheduler.candidate_permission_revision(&self.candidate_id)
                != self.candidate_permission_revision
            {
                return Err(RotationDispatchStartError::CandidateChanged);
            }
            if !scheduler.dispatch_visible(
                &self.candidate_id,
                &self.model,
                &self.allowed_protocols,
                &scope,
                runtime_now_ms(),
            ) {
                return Err(RotationDispatchStartError::CandidateChanged);
            }
            let attempt = scheduler.begin_rotation_dispatch(self.reservation_id, request_budget)?;
            if charge_wire {
                // Both budget counters are guarded by the same lock. The
                // availability check above makes this infallible.
                request_budget
                    .start_wire_attempt()
                    .expect("wire capacity was checked under the budget lock");
            }
            Ok(attempt)
        });
        if result.is_ok() {
            self.rotation_started.store(true, Ordering::Release);
            budget.record_member_attempt(&self.member_key);
        }
        result
    }

    pub(crate) fn settle_rotation(
        &self,
        observation: RotationAttemptObservation,
        now_ms: u64,
    ) -> std::result::Result<Option<RotationSettlement>, RotationSettlementError> {
        self.settle_rotation_with(observation, now_ms, |_| {})
    }

    /// Reduce mandatory observations before making a released rotation permit
    /// visible to another admission. The callback runs under the same
    /// scheduler lock as settlement, never for duplicate or stale outcomes.
    pub(super) fn settle_rotation_with(
        &self,
        observation: RotationAttemptObservation,
        now_ms: u64,
        apply: impl FnOnce(&mut PoolScheduler),
    ) -> std::result::Result<Option<RotationSettlement>, RotationSettlementError> {
        let budget = &self.rotation_budget;
        if self.rotation_settled.load(Ordering::Acquire) {
            return Ok(None);
        }
        let result = budget.with_budget(|request_budget| {
            if self.rotation_settled.load(Ordering::Acquire) {
                return Ok(None);
            }
            request_budget.observe_execution(observation.execution);
            let mut scheduler = self
                .scheduler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let settlement = scheduler.settle_rotation(
                self.reservation_id,
                observation,
                request_budget,
                now_ms,
            )?;
            if settlement.observation_current {
                apply(&mut scheduler);
            }
            self.rotation_settled.store(true, Ordering::Release);
            Ok(Some(settlement))
        });
        let result = result?;
        self.release();
        Ok(result)
    }

    pub(crate) fn settle_rotation_success(&self, now_ms: u64) {
        let _ = self.settle_rotation(
            RotationAttemptObservation {
                execution: RotationExecutionObservation::committed(),
                health: RotationHealthObservation::Success,
            },
            now_ms,
        );
    }

    #[cfg(test)]
    pub(crate) fn settle_rotation_transient(
        &self,
        provider_not_before_ms: Option<u64>,
        now_ms: u64,
    ) {
        let _ = self.settle_rotation(
            RotationAttemptObservation {
                execution: RotationExecutionObservation::not_sent(),
                health: if self.rotation_started.load(Ordering::Acquire) {
                    RotationHealthObservation::CountableTransient {
                        provider_not_before_ms,
                    }
                } else {
                    RotationHealthObservation::LocalError
                },
            },
            now_ms,
        );
    }

    pub(crate) fn settle_rotation_terminal(&self, now_ms: u64) {
        let _ = self.settle_rotation(
            RotationAttemptObservation {
                execution: RotationExecutionObservation::accepted(),
                health: RotationHealthObservation::ClientError,
            },
            now_ms,
        );
    }

    /// A driver proved an explicit per-execution compatibility rejection and
    /// a supported repair. It may release the lease without blocking health.
    pub(crate) fn settle_rotation_repair(&self, now_ms: u64) {
        let _ = self.settle_rotation(
            RotationAttemptObservation {
                execution: RotationExecutionObservation::not_sent(),
                health: RotationHealthObservation::ClientError,
            },
            now_ms,
        );
        self.allow_rotation_repair();
    }

    pub(crate) fn allow_rotation_repair(&self) {
        self.rotation_budget.allow_member_repair(&self.member_key);
    }

    pub(crate) fn settle_rotation_unknown(&self, now_ms: u64) {
        let _ = self.settle_rotation(
            RotationAttemptObservation {
                execution: RotationExecutionObservation::unknown(),
                health: RotationHealthObservation::Unknown,
            },
            now_ms,
        );
    }

    pub(crate) fn release(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        // Match settlement's budget -> scheduler lock order. A started lease
        // without an explicit result is cancellation with an unknown outcome.
        {
            let budget = &self.rotation_budget;
            if !self.rotation_settled.swap(true, Ordering::AcqRel) {
                budget.with_budget(|request_budget| {
                    let mut scheduler = self
                        .scheduler
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if self.rotation_started.load(Ordering::Acquire) {
                        request_budget.observe_execution(RotationExecutionObservation::unknown());
                        let _ = scheduler.cancel_rotation(
                            self.reservation_id,
                            request_budget,
                            crate::unix_time_ms(),
                        );
                    } else {
                        let _ = scheduler.release_rotation_unstarted(self.reservation_id);
                    }
                });
            }
        }
        let activity = {
            let mut scheduler = self
                .scheduler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let released = scheduler.release_reservation(self.reservation_id);
            if !released {
                None
            } else {
                let (in_flight, active_request_count, active_models) =
                    scheduler.runtime_activity_for(&self.candidate_id);
                Some(RuntimeActivitySnapshot {
                    runtime_id: self.activity_runtime_id,
                    revision: self.activity_revision.fetch_add(1, Ordering::AcqRel) + 1,
                    candidate_id: self.candidate_id.clone(),
                    member_key: self.member_key.clone(),
                    in_flight,
                    active_request_count,
                    active_models,
                })
            }
        };
        if let Some(activity) = activity {
            self.availability.notify_waiters();
            let callback = self
                .activity_callback
                .lock()
                .ok()
                .map(|callback| callback.clone());
            if let Some(callback) = callback {
                callback(activity);
            }
        }
    }
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
