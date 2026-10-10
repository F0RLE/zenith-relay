use super::super::*;
use super::CandidateLease;
use super::{
    RotationAttemptId, RotationAttemptObservation, RotationDispatchStartError,
    RotationExecutionObservation, RotationHealthObservation, RotationSettlement,
    RotationSettlementError,
};

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
        let dispatch_result = budget.with_budget(|request_budget| {
            if charge_wire && !request_budget.can_start_wire() {
                return Err(RotationDispatchStartError::BudgetExhausted);
            }
            let candidate_scope = crate::poison::read(&self.principal_scope);
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
            let mut scheduler = crate::poison::mutex(&self.scheduler);
            let hidden_models = crate::poison::read(&self.hidden_models);
            if hidden_models.contains(&crate::model_id_key(&self.model)) {
                return Err(RotationDispatchStartError::CandidateChanged);
            }
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
                    .is_none_or(|(owner, observed_revision)| {
                        owner != self.candidate_id || observed_revision != *revision
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
                &candidate_scope,
                runtime_now_ms(),
            ) {
                return Err(RotationDispatchStartError::CandidateChanged);
            }
            let rotation_attempt =
                scheduler.begin_rotation_dispatch(self.reservation_id, request_budget)?;
            if charge_wire {
                // Both budget counters are guarded by the same lock. The
                // availability check above makes this infallible.
                request_budget
                    .start_wire_attempt()
                    .expect("wire capacity was checked under the budget lock");
            }
            Ok(rotation_attempt)
        });
        if dispatch_result.is_ok() {
            self.rotation_started.store(true, Ordering::Release);
            budget.record_member_attempt(&self.member_key);
        }
        dispatch_result
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
        let settlement_result = budget.with_budget(|request_budget| {
            if self.rotation_settled.load(Ordering::Acquire) {
                return Ok(None);
            }
            request_budget.observe_execution(observation.execution);
            let mut scheduler = crate::poison::mutex(&self.scheduler);
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
        let settlement_result = settlement_result?;
        self.release();
        Ok(settlement_result)
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
                    let mut scheduler = crate::poison::mutex(&self.scheduler);
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
        let runtime_activity = {
            let mut scheduler = crate::poison::mutex(&self.scheduler);
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
        if let Some(runtime_activity) = runtime_activity {
            self.availability.notify_waiters();
            let callback = self
                .activity_callback
                .lock()
                .ok()
                .map(|callback| callback.clone());
            if let Some(callback) = callback {
                callback(runtime_activity);
            }
        }
    }
}
