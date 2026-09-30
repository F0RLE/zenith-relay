//! Selection, physical reservations, circuit fencing and recovery permits.

use super::*;

#[derive(Clone, Debug, Default)]
struct CircuitRuntime {
    state: CircuitState,
    failure_streak: u32,
    counted_requests: BTreeSet<RequestId>,
    not_before_ms: Option<u64>,
    epoch: u64,
    incident: u64,
    last_failure_at_ms: Option<u64>,
    half_open_lease: Option<LeaseId>,
    last_transition: CircuitTransition,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum CircuitTransition {
    #[default]
    None,
    Failure,
    Success,
    RecoveryAbort,
}

impl CircuitRuntime {
    fn snapshot(&self) -> CircuitSnapshot {
        CircuitSnapshot {
            state: self.state,
            failure_streak: self.failure_streak,
            not_before_ms: self.not_before_ms,
            epoch: self.epoch,
        }
    }
}

#[derive(Clone, Debug)]
struct CandidateRuntime {
    candidate: RotationCandidate,
    in_flight: u32,
}

#[derive(Clone, Debug)]
struct LeaseRuntime {
    lease_id: LeaseId,
    candidate_id: String,
    candidate_generation: u64,
    request_id: RequestId,
    route_key: String,
    route: RotationRoute,
    capacity_key: String,
    recovery: bool,
    recovery_credit_required: bool,
    attempt_id: Option<AttemptId>,
    circuit_epoch: u64,
    circuit_incident: u64,
    quota: QuotaState,
    rate: RateState,
    route_rate: Option<RateState>,
}

/// Deterministic pool-rotation admission and state reducer.
#[derive(Clone, Debug)]
pub struct RotationEngine {
    mode: RotationMode,
    max_in_flight: u32,
    candidates: BTreeMap<String, CandidateRuntime>,
    candidate_generations: BTreeMap<String, u64>,
    quota_revisions: BTreeMap<String, u64>,
    circuits: BTreeMap<(String, String), CircuitRuntime>,
    rotation_credit: BTreeMap<(String, String), i64>,
    capacity_in_flight: BTreeMap<String, u32>,
    capacity_limits: BTreeMap<String, u32>,
    leases: BTreeMap<LeaseId, LeaseRuntime>,
    next_lease_id: u64,
    next_candidate_generation: u64,
    recovery_policy: RecoveryPolicy,
    recovery_credits: u32,
    recovery_in_flight: u32,
    successful_requests_since_recovery: u32,
}

impl Default for RotationEngine {
    fn default() -> Self {
        let policy = RecoveryPolicy::default();
        Self {
            mode: RotationMode::Automatic,
            // Safety ceiling of this standalone engine, not a migration of a
            // saved member limit. Hosts must install their validated policy.
            max_in_flight: 1024,
            candidates: BTreeMap::new(),
            candidate_generations: BTreeMap::new(),
            quota_revisions: BTreeMap::new(),
            circuits: BTreeMap::new(),
            rotation_credit: BTreeMap::new(),
            capacity_in_flight: BTreeMap::new(),
            capacity_limits: BTreeMap::new(),
            leases: BTreeMap::new(),
            next_lease_id: 0,
            next_candidate_generation: 0,
            recovery_credits: policy.initial_credits,
            recovery_in_flight: 0,
            successful_requests_since_recovery: 0,
            recovery_policy: policy,
        }
    }
}

#[derive(Clone, Debug)]
struct ReadyCandidate {
    id: String,
    capacity_key: String,
    in_flight: u32,
    effective_capacity: u32,
    priority: i32,
    weight: u32,
    recovery: bool,
    due_at_ms: Option<u64>,
}

mod admission;
mod health;
mod lifecycle;
mod registry;

#[cfg(test)]
use health::failure_backoff_ms;

#[cfg(test)]
mod regression;
#[cfg(test)]
mod tests;
