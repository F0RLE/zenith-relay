//! Incident-scoped outcomes, circuit transitions and bounded recovery pacing.

use super::*;

impl RotationEngine {
    pub(super) fn decayed_failure_score(&self, health: &HealthRuntime, now_ms: u64) -> u32 {
        let elapsed = health
            .observed_at_ms
            .map(|observed_at_ms| now_ms.saturating_sub(observed_at_ms))
            .unwrap_or_default();
        let mut score = health.failure_score_milli;
        for _ in 0..(elapsed / HEALTH_HALF_LIFE_MS).min(16) {
            score /= 2;
        }
        score
    }

    pub(super) fn observe_health(
        &mut self,
        candidate_id: &str,
        route_key: &str,
        observation: HealthObservation,
        now_ms: u64,
    ) {
        if !matches!(
            observation,
            HealthObservation::Success | HealthObservation::CountableTransient { .. }
        ) {
            return;
        }
        let health = self
            .health
            .entry((candidate_id.to_owned(), route_key.to_owned()))
            .or_default();
        let elapsed = health
            .observed_at_ms
            .map(|observed_at_ms| now_ms.saturating_sub(observed_at_ms))
            .unwrap_or_default();
        for _ in 0..(elapsed / HEALTH_HALF_LIFE_MS).min(16) {
            health.failure_score_milli /= 2;
        }
        match observation {
            HealthObservation::CountableTransient { .. } => {
                let alpha = HEALTH_EWMA_ALPHA_MILLI;
                health.failure_score_milli = health
                    .failure_score_milli
                    .saturating_mul(1_000 - alpha)
                    .saturating_add(1_000_u32.saturating_mul(alpha))
                    / 1_000;
            }
            HealthObservation::Success => {
                health.failure_score_milli = health
                    .failure_score_milli
                    .saturating_mul(1_000 - HEALTH_EWMA_ALPHA_MILLI)
                    / 1_000;
            }
            HealthObservation::Busy
            | HealthObservation::Cancelled
            | HealthObservation::ClientError
            | HealthObservation::LocalError
            | HealthObservation::MonitoringFailure
            | HealthObservation::Unknown => return,
        }
        health.observed_at_ms = Some(now_ms);
    }

    pub(super) fn observe_first_output_latency(
        &mut self,
        candidate_id: &str,
        route_key: &str,
        first_output_latency_ms: u64,
        now_ms: u64,
    ) {
        let health = self
            .health
            .entry((candidate_id.to_owned(), route_key.to_owned()))
            .or_default();
        health.first_output_latency_ms = Some(match health.first_output_latency_ms {
            Some(previous) => {
                let alpha = u64::from(HEALTH_EWMA_ALPHA_MILLI);
                previous
                    .saturating_mul(1_000 - alpha)
                    .saturating_add(first_output_latency_ms.saturating_mul(alpha))
                    / 1_000
            }
            None => first_output_latency_ms,
        });
        health.latency_observed_at_ms = Some(now_ms);
    }

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
    ) -> (CircuitSnapshot, bool) {
        let key = (lease.candidate_id.clone(), lease.route_key.clone());
        let circuit = self.circuits.entry(key).or_default();
        if circuit.incident != lease.circuit_incident {
            return (circuit.snapshot(), false);
        }
        if circuit.half_open_lease == Some(lease.lease_id) {
            circuit.half_open_lease = None;
        }
        let mut counted = false;
        match health {
            HealthObservation::Success => {
                if circuit.epoch != lease.circuit_epoch {
                    // A newer failure/refresh superseded this attempt. Its
                    // late success must not close the new circuit or shorten
                    // a cooldown.
                    if lease.recovery {
                        circuit.state = CircuitState::Open;
                    }
                    return (circuit.snapshot(), false);
                }
                counted = true;
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
                    return (circuit.snapshot(), false);
                }
                counted = true;
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
        (circuit.snapshot(), counted)
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
