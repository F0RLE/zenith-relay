//! Atomic reservations, final dispatch fences and idempotent settlement.

use super::*;

impl RotationEngine {
    /// Releases an admission that never reached a wire dispatch. This is used
    /// when a second, owner-local validation loses a race after rotation reserved a
    /// candidate. It does not create a health observation or spend recovery.
    pub fn release_unstarted(&mut self, lease_id: LeaseId) -> bool {
        let Some(lease) = self.leases.get(&lease_id) else {
            return false;
        };
        if lease.attempt_id.is_some() {
            return false;
        }
        let lease = self.leases.remove(&lease_id).expect("lease was present");
        self.release_capacity(&lease);
        true
    }

    pub(super) fn release_capacity(&mut self, lease: &LeaseRuntime) {
        if self.candidate_generations.get(&lease.candidate_id).copied()
            == Some(lease.candidate_generation)
        {
            if let Some(runtime) = self.candidates.get_mut(&lease.candidate_id) {
                runtime.in_flight = runtime.in_flight.saturating_sub(1);
            }
        }
        let remove_capacity =
            if let Some(count) = self.capacity_in_flight.get_mut(&lease.capacity_key) {
                *count = count.saturating_sub(1);
                *count == 0
            } else {
                false
            };
        if remove_capacity {
            self.capacity_in_flight.remove(&lease.capacity_key);
        }
        if lease.recovery {
            self.recovery_in_flight = self.recovery_in_flight.saturating_sub(1);
            if let Some(circuit) = self
                .circuits
                .get_mut(&(lease.candidate_id.clone(), lease.route_key.clone()))
            {
                if circuit.half_open_lease == Some(lease.lease_id) {
                    circuit.half_open_lease = None;
                    circuit.state = CircuitState::Open;
                }
            }
        }
    }

    pub fn reserve(
        &mut self,
        request: &RotationRequest,
        now_ms: u64,
    ) -> Result<RotationLease, AdmissionError> {
        let selection = self
            .select(request, now_ms)
            .ok_or(AdmissionError::NoEligibleCandidate)?;
        if !matches!(
            self.candidate_availability(request, &selection.candidate_id, now_ms),
            CandidateAvailability::Ready { .. }
        ) {
            return Err(AdmissionError::CandidateChanged);
        }
        let Some(runtime) = self.candidates.get(&selection.candidate_id) else {
            return Err(AdmissionError::InvalidCandidate);
        };
        if runtime.candidate.max_concurrency > 0
            && runtime.in_flight >= runtime.candidate.max_concurrency
        {
            return Err(AdmissionError::CandidateChanged);
        }
        let (ready, ordinary_alternatives) = self.selection_candidates(request, now_ms);
        let group = self.selection_group(&ready);
        let selected = self
            .choose_for_request(request, &group)
            .ok_or(AdmissionError::NoEligibleCandidate)?;
        if selected.id != selection.candidate_id {
            return Err(AdmissionError::CandidateChanged);
        }
        let next_lease_id = self
            .next_lease_id
            .checked_add(1)
            .ok_or(AdmissionError::InvalidCandidate)?;
        let lease_id = LeaseId(next_lease_id);
        let recovery = selected.recovery;
        let candidate_id = selected.id.clone();
        if recovery && self.recovery_in_flight >= self.recovery_policy.max_in_flight {
            return Err(AdmissionError::CandidateChanged);
        }
        let circuit_epoch = self.circuit(&candidate_id, &request.route_key).epoch;
        let circuit_incident = self
            .circuits
            .get(&(candidate_id.clone(), request.route_key.clone()))
            .map_or(0, |circuit| circuit.incident);
        let candidate_generation = self
            .candidate_generations
            .get(&candidate_id)
            .copied()
            .unwrap_or_default();
        if recovery {
            let key = (candidate_id.clone(), request.route_key.clone());
            let circuit = self.circuits.entry(key).or_default();
            if circuit.half_open_lease.is_some() {
                return Err(AdmissionError::CandidateChanged);
            }
            circuit.state = CircuitState::HalfOpen;
            circuit.half_open_lease = Some(lease_id);
        }
        let Some(runtime) = self.candidates.get_mut(&candidate_id) else {
            return Err(AdmissionError::InvalidCandidate);
        };
        self.next_lease_id = next_lease_id;
        runtime.in_flight = runtime.in_flight.saturating_add(1);
        let capacity_key = runtime.candidate.capacity_key.clone();
        self.capacity_in_flight
            .entry(capacity_key.clone())
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
        if recovery {
            // A due trial with no ordinary alternative is demand-driven and
            // does not consume the exploration credit.  With a healthy
            // alternative present it consumes exactly one pool permit when
            // the application request is actually dispatched (not on a
            // preview or reservation that loses a race).
            self.recovery_in_flight = self.recovery_in_flight.saturating_add(1);
        }
        self.leases.insert(
            lease_id,
            LeaseRuntime {
                candidate_id: candidate_id.clone(),
                candidate_generation,
                lease_id,
                request_id: request.request_id,
                route_key: request.route_key.clone(),
                route: RotationRoute {
                    model: request.model.clone(),
                    operation: request.operation,
                },
                capacity_key,
                recovery,
                recovery_credit_required: recovery && ordinary_alternatives,
                attempt_id: None,
                circuit_epoch,
                circuit_incident,
                quota: runtime.candidate.quota,
                rate: runtime.candidate.rate,
                route_rate: runtime
                    .candidate
                    .route_rates
                    .get(&request.route_key)
                    .copied(),
            },
        );
        self.advance_weighted_credit(&request.route_key, &group, &candidate_id);
        Ok(RotationLease {
            lease_id,
            candidate_id,
            request_id: request.request_id,
            route_key: request.route_key.clone(),
            recovery,
        })
    }

    pub fn begin_dispatch(
        &mut self,
        lease_id: LeaseId,
        budget: &mut RequestBudget,
    ) -> Result<AttemptId, DispatchStartError> {
        self.begin_dispatch_internal(lease_id, budget, false)
    }

    /// Only the transport controller can retain a lease for an explicitly
    /// rejected auth/endpoint repair. Budget charging follows every fence.
    pub(crate) fn begin_transport_dispatch(
        &mut self,
        lease_id: LeaseId,
        budget: &mut RequestBudget,
    ) -> Result<AttemptId, DispatchStartError> {
        self.begin_dispatch_internal(lease_id, budget, true)
    }

    pub(super) fn begin_dispatch_internal(
        &mut self,
        lease_id: LeaseId,
        budget: &mut RequestBudget,
        allow_retained_lease: bool,
    ) -> Result<AttemptId, DispatchStartError> {
        let Some(pending) = self.leases.get(&lease_id).cloned() else {
            return Err(DispatchStartError::UnknownLease);
        };
        if pending.request_id != budget.request_id() {
            return Err(DispatchStartError::BudgetRequestMismatch);
        }
        // A verified auth/endpoint repair can retain the lease, but it must
        // present a newly charged, monotonically increasing wire dispatch.
        if pending.attempt_id.is_some_and(|previous| {
            !allow_retained_lease || previous.0 > u64::from(budget.dispatches())
        }) {
            return Err(DispatchStartError::AlreadyStarted);
        }
        let still_current = self
            .candidates
            .get(&pending.candidate_id)
            .is_some_and(|runtime| {
                self.candidate_generations.get(&pending.candidate_id)
                    == Some(&pending.candidate_generation)
                    && runtime.candidate.enabled
                    && !runtime.candidate.draining
                    && runtime.candidate.auth == AuthState::Ready
                    && runtime.candidate.quota == pending.quota
                    && runtime.candidate.rate == pending.rate
                    && runtime
                        .candidate
                        .route_rates
                        .get(&pending.route_key)
                        .copied()
                        == pending.route_rate
                    && runtime.candidate.capacity_key == pending.capacity_key
                    && runtime
                        .candidate
                        .routes
                        .get(&pending.route_key)
                        .is_some_and(|route| {
                            route.model.eq_ignore_ascii_case(&pending.route.model)
                                && route.operation == pending.route.operation
                        })
            });
        if !still_current {
            return Err(DispatchStartError::CandidateChanged);
        }
        let circuit = self.circuit(&pending.candidate_id, &pending.route_key);
        if circuit.epoch != pending.circuit_epoch && circuit.state != CircuitState::Closed {
            return Err(DispatchStartError::CandidateChanged);
        }
        if pending.recovery_credit_required && self.recovery_credits == 0 {
            return Err(DispatchStartError::RecoveryBudgetExhausted);
        }
        let attempt_id = budget
            .start_dispatch()
            .ok_or(DispatchStartError::BudgetExhausted)?;
        if pending.recovery_credit_required {
            self.recovery_credits = self.recovery_credits.saturating_sub(1);
        }
        let Some(lease) = self.leases.get_mut(&lease_id) else {
            return Err(DispatchStartError::UnknownLease);
        };
        lease.recovery_credit_required = false;
        lease.attempt_id = Some(attempt_id);
        Ok(attempt_id)
    }

    pub fn settle(
        &mut self,
        lease_id: LeaseId,
        observation: AttemptObservation,
        budget: &RequestBudget,
        now_ms: u64,
    ) -> Result<RotationSettlement, SettlementError> {
        let Some(pending) = self.leases.get(&lease_id) else {
            return Err(SettlementError::UnknownLease);
        };
        if budget.request_id() != pending.request_id {
            return Err(SettlementError::BudgetRequestMismatch);
        }
        if pending.attempt_id.is_none()
            && matches!(
                observation.health,
                HealthObservation::Success | HealthObservation::CountableTransient { .. }
            )
        {
            return Err(SettlementError::NotDispatched);
        }
        let lease = self
            .leases
            .remove(&lease_id)
            .ok_or(SettlementError::UnknownLease)?;
        let current_generation = self.candidate_generations.get(&lease.candidate_id).copied();
        let observation_current = current_generation == Some(lease.candidate_generation)
            && self
                .candidates
                .get(&lease.candidate_id)
                .and_then(|runtime| runtime.candidate.routes.get(&lease.route_key))
                == Some(&lease.route);
        self.release_capacity(&lease);
        if observation_current
            && !lease.recovery
            && lease.attempt_id.is_some()
            && matches!(observation.health, HealthObservation::Success)
        {
            self.successful_requests_since_recovery =
                self.successful_requests_since_recovery.saturating_add(1);
            if self.successful_requests_since_recovery
                >= self.recovery_policy.successful_requests_per_credit
            {
                self.recovery_credits = self
                    .recovery_credits
                    .saturating_add(1)
                    .min(self.recovery_policy.initial_credits);
                self.successful_requests_since_recovery = 0;
            }
        }
        // Health origin and replay evidence are independent. The driver must
        // prove a countable upstream failure; that never authorizes a retry
        // of possibly accepted work.
        let health = observation.health;
        let circuit = if observation_current {
            self.update_circuit(&lease, health, now_ms)
        } else {
            // A removed candidate may be re-added under the same display ID.
            // Its old outcome can release only its own capacity lease; it must
            // not publish health or cooldown state into the new identity.
            self.circuit(&lease.candidate_id, &lease.route_key)
        };
        let retry = match health {
            HealthObservation::Success => RetryDecision::Stop(RetryStopReason::Succeeded),
            HealthObservation::Busy
            | HealthObservation::Cancelled
            | HealthObservation::ClientError
            | HealthObservation::LocalError
            | HealthObservation::MonitoringFailure => {
                RetryDecision::Stop(if observation.health == HealthObservation::Cancelled {
                    RetryStopReason::Cancelled
                } else {
                    RetryStopReason::TerminalHealthObservation
                })
            }
            HealthObservation::CountableTransient { .. } | HealthObservation::Unknown => {
                budget.retry_decision(observation.execution)
            }
        };
        Ok(RotationSettlement {
            candidate_id: lease.candidate_id,
            attempt_id: lease.attempt_id,
            retry,
            circuit,
            observation_current,
        })
    }

    pub fn cancel(
        &mut self,
        lease_id: LeaseId,
        budget: &RequestBudget,
        now_ms: u64,
    ) -> Result<RotationSettlement, SettlementError> {
        self.settle(
            lease_id,
            AttemptObservation {
                execution: ExecutionObservation::unknown(),
                health: HealthObservation::Cancelled,
            },
            budget,
            now_ms,
        )
    }
}
