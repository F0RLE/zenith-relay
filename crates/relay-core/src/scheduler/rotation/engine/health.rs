//! Incident-scoped outcomes, circuit transitions and bounded recovery pacing.

use super::*;

impl RotationEngine {
    pub fn circuit(&self, candidate_id: &str, route_key: &str) -> CircuitSnapshot {
        self.circuits
            .get(&(candidate_id.to_owned(), route_key.to_owned()))
            .map_or_else(
                || CircuitRuntime::default().snapshot(),
                CircuitRuntime::snapshot,
            )
    }

    pub(super) fn update_circuit(
        &mut self,
        lease: &LeaseRuntime,
        health: HealthObservation,
        now_ms: u64,
    ) -> CircuitSnapshot {
        let key = (lease.candidate_id.clone(), lease.route_key.clone());
        let circuit = self.circuits.entry(key).or_default();
        if circuit.incident != lease.circuit_incident {
            return circuit.snapshot();
        }
        if circuit.half_open_lease == Some(lease.lease_id) {
            circuit.half_open_lease = None;
        }
        match health {
            HealthObservation::Success => {
                if circuit.epoch != lease.circuit_epoch {
                    // A newer failure/refresh superseded this attempt. Its
                    // late success must not close the new circuit or shorten
                    // a cooldown.
                    if lease.recovery {
                        circuit.state = CircuitState::Open;
                    }
                    return circuit.snapshot();
                }
                circuit.state = CircuitState::Closed;
                circuit.failure_streak = 0;
                circuit.counted_requests.clear();
                circuit.not_before_ms = None;
                circuit.epoch = circuit.epoch.saturating_add(1);
                circuit.incident = circuit.incident.saturating_add(1);
                circuit.last_failure_at_ms = None;
                circuit.last_transition = CircuitTransition::Success;
            }
            HealthObservation::CountableTransient {
                provider_not_before_ms,
            } => {
                if circuit
                    .last_failure_at_ms
                    .is_some_and(|at| now_ms.saturating_sub(at) > FAILURE_WINDOW_MS)
                {
                    circuit.failure_streak = 0;
                    circuit.counted_requests.clear();
                }
                if !circuit.counted_requests.insert(lease.request_id) {
                    if lease.recovery {
                        circuit.state = CircuitState::Open;
                        circuit.not_before_ms = Some(
                            now_ms
                                .saturating_add(FIRST_TRANSIENT_PACING_MS)
                                .max(provider_not_before_ms.unwrap_or_default())
                                .max(circuit.not_before_ms.unwrap_or_default()),
                        );
                    }
                    return circuit.snapshot();
                }
                // Only recent independent requests matter after the
                // threshold. Do not retain every failed RequestId forever.
                if circuit.counted_requests.len() > 64 {
                    circuit.counted_requests.pop_first();
                }
                circuit.failure_streak = circuit.failure_streak.saturating_add(1);
                circuit.last_failure_at_ms = Some(now_ms);
                let local_not_before =
                    now_ms.saturating_add(failure_backoff_ms(circuit.failure_streak.max(1)));
                let not_before_ms = provider_not_before_ms
                    .unwrap_or_default()
                    .max(local_not_before)
                    .max(circuit.not_before_ms.unwrap_or_default());
                circuit.not_before_ms = Some(not_before_ms);
                circuit.epoch = circuit.epoch.saturating_add(1);
                circuit.last_transition = CircuitTransition::Failure;
                circuit.state =
                    if lease.recovery || circuit.failure_streak >= CIRCUIT_FAILURE_THRESHOLD {
                        CircuitState::Open
                    } else {
                        CircuitState::Degraded
                    };
            }
            HealthObservation::Cancelled
            | HealthObservation::Busy
            | HealthObservation::ClientError
            | HealthObservation::LocalError
            | HealthObservation::MonitoringFailure
            | HealthObservation::Unknown => {
                if lease.recovery {
                    // A canceled/unknown recovery attempt does not vote as a
                    // failure, but it must not leave the one-permit gate stuck.
                    circuit.state = CircuitState::Open;
                    circuit.not_before_ms = Some(
                        now_ms
                            .saturating_add(FIRST_TRANSIENT_PACING_MS)
                            .max(circuit.not_before_ms.unwrap_or_default()),
                    );
                    circuit.epoch = circuit.epoch.saturating_add(1);
                    circuit.last_transition = CircuitTransition::RecoveryAbort;
                }
            }
        }
        circuit.snapshot()
    }
}

pub(super) fn failure_backoff_ms(failure_streak: u32) -> u64 {
    match failure_streak {
        0 | 1 => FIRST_TRANSIENT_PACING_MS,
        2 => SECOND_TRANSIENT_PACING_MS,
        streak => OPEN_BACKOFF_MS
            .saturating_mul(2_u64.saturating_pow(streak.saturating_sub(3).min(5)))
            .min(MAX_OPEN_BACKOFF_MS),
    }
}
