//! Exact routes, independent availability states and lifecycle observations.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthState {
    Ready,
    NeedsRefresh,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaState {
    Unknown,
    Available,
    Stale,
    Exhausted { reset_at_ms: Option<u64> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateState {
    Ready,
    Limited { not_before_ms: u64 },
}

/// Inference operations are explicit; an inventory entry is not permission
/// to execute every operation through every protocol.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RotationOperation {
    Text,
    Image,
    Compaction,
    Metadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationRoute {
    pub model: String,
    pub operation: RotationOperation,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RotationMode {
    #[default]
    Automatic,
    InOrder,
    RoundRobin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryPolicy {
    /// Initial demand/exploration permits while a healthy alternative exists.
    pub initial_credits: u32,
    /// One additional exploration permit after this many successful ordinary
    /// completions.  A value of zero is normalized to one.
    pub successful_requests_per_credit: u32,
    /// Maximum simultaneous recovery trials for the runtime/fault domain.
    pub max_in_flight: u32,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self {
            initial_credits: 1,
            successful_requests_per_credit: 8,
            max_in_flight: 1,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationCandidate {
    pub id: String,
    /// Verified physical resource identity.  Aliases with the same key share
    /// the capacity permit; an unknown relationship must use distinct keys.
    pub capacity_key: String,
    pub priority: i32,
    pub weight: u32,
    pub max_concurrency: u32,
    pub capacity_limit: u32,
    pub enabled: bool,
    pub draining: bool,
    /// Each adapter-owned route key binds one model, protocol/execution
    /// profile and operation. Never cross-product independent inventories.
    pub routes: BTreeMap<String, RotationRoute>,
    /// Provider cooldown observations may be scoped to one model route. The
    /// candidate-level rate is reserved for a verified global cooldown.
    pub route_rates: BTreeMap<String, RateState>,
    pub auth: AuthState,
    pub quota: QuotaState,
    pub rate: RateState,
}

impl RotationCandidate {
    pub fn new(
        id: impl Into<String>,
        route_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let id = id.into();
        let route_key = route_key.into();
        let routes = BTreeMap::from([(
            route_key.clone(),
            RotationRoute {
                model: model.into(),
                operation: RotationOperation::Text,
            },
        )]);
        Self {
            capacity_key: id.clone(),
            id,
            priority: 0,
            weight: 1,
            max_concurrency: 0,
            capacity_limit: 0,
            enabled: true,
            draining: false,
            routes,
            route_rates: BTreeMap::from([(route_key, RateState::Ready)]),
            auth: AuthState::Ready,
            quota: QuotaState::Unknown,
            rate: RateState::Ready,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.id.trim().is_empty()
            || self.id.trim() != self.id
            || self.id.len() > 256
            || self.id.chars().any(char::is_control)
        {
            return Err("rotation candidate id is invalid");
        }
        if self.capacity_key.trim().is_empty()
            || self.capacity_key.trim() != self.capacity_key
            || self.capacity_key.len() > 256
            || self.capacity_key.chars().any(char::is_control)
        {
            return Err("rotation candidate capacity key is invalid");
        }
        if !(1..=100).contains(&self.weight) {
            return Err("rotation candidate weight is invalid");
        }
        if self.routes.is_empty()
            || self.routes.len() > 4096
            || self.routes.iter().any(|(key, route)| {
                [key.as_str(), route.model.as_str()]
                    .into_iter()
                    .any(|value| {
                        value.trim().is_empty()
                            || value.trim() != value
                            || value.len() > 256
                            || value.chars().any(char::is_control)
                    })
            })
        {
            return Err("rotation candidate route or model is invalid");
        }
        Ok(())
    }

    pub fn with_capacity(mut self, capacity_key: impl Into<String>, limit: u32) -> Self {
        self.capacity_key = capacity_key.into();
        self.capacity_limit = limit;
        self
    }

    pub fn with_operation(mut self, operation: RotationOperation) -> Self {
        for route in self.routes.values_mut() {
            route.operation = operation;
        }
        self
    }

    pub fn add_route(
        &mut self,
        route_key: impl Into<String>,
        model: impl Into<String>,
        operation: RotationOperation,
    ) {
        let route_key = route_key.into();
        self.routes.insert(
            route_key.clone(),
            RotationRoute {
                model: model.into(),
                operation,
            },
        );
        self.route_rates
            .entry(route_key)
            .or_insert(RateState::Ready);
    }

    pub(super) fn supports(&self, request: &RotationRequest) -> Result<(), CandidateBlockReason> {
        if !self.enabled {
            return Err(CandidateBlockReason::Disabled);
        }
        if self.draining {
            return Err(CandidateBlockReason::Draining);
        }
        let Some(route) = self.routes.get(&request.route_key) else {
            return Err(CandidateBlockReason::RouteUnavailable);
        };
        if !route.model.eq_ignore_ascii_case(&request.model) {
            return Err(CandidateBlockReason::ModelUnavailable);
        }
        if route.operation != request.operation {
            return Err(CandidateBlockReason::OperationUnavailable);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CircuitState {
    #[default]
    Closed,
    Degraded,
    Open,
    HalfOpen,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CircuitSnapshot {
    pub state: CircuitState,
    pub failure_streak: u32,
    pub not_before_ms: Option<u64>,
    pub epoch: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateBlockReason {
    AlreadyTried,
    NotAllowed,
    OwnerMismatch,
    Disabled,
    Draining,
    RouteUnavailable,
    ModelUnavailable,
    OperationUnavailable,
    AuthRequired,
    AuthBlocked,
    QuotaExhausted,
    RateLimited,
    CircuitOpen,
    RecoveryBusy,
    CapacityBusy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateAvailability {
    Ready {
        recovery: bool,
    },
    Busy(CandidateBlockReason),
    WaitUntil {
        at_ms: u64,
        reason: CandidateBlockReason,
    },
    Blocked(CandidateBlockReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationRequest {
    pub request_id: RequestId,
    pub route_key: String,
    pub model: String,
    pub operation: RotationOperation,
    pub owner: Option<String>,
    /// Soft locality preference, considered only within the ordinary
    /// Automatic selection group. It cannot replace a recovery trial.
    pub preferred: Option<String>,
    pub tried: BTreeSet<String>,
    /// Principal/spend policy is a hard gate independent of priority. An
    /// empty set permits nothing; None is for an explicitly unrestricted pool.
    pub allowed_candidates: Option<BTreeSet<String>>,
}

impl RotationRequest {
    pub fn new(
        request_id: RequestId,
        route_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            request_id,
            route_key: route_key.into(),
            model: model.into(),
            operation: RotationOperation::Text,
            owner: None,
            preferred: None,
            tried: BTreeSet::new(),
            allowed_candidates: None,
        }
    }

    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    pub fn with_tried(mut self, candidate_id: impl Into<String>) -> Self {
        self.tried.insert(candidate_id.into());
        self
    }

    pub fn with_allowed_candidates(mut self, candidates: BTreeSet<String>) -> Self {
        self.allowed_candidates = Some(candidates);
        self
    }

    pub fn with_operation(mut self, operation: RotationOperation) -> Self {
        self.operation = operation;
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RotationSelectionReason {
    HardOwner,
    PrimaryFirst,
    WeightedRotation,
    Recovery,
    OnlyEligible,
    LeastLoaded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationSelection {
    pub candidate_id: String,
    pub priority: i32,
    pub recovery: bool,
    pub eligible_candidates: u32,
    pub reason: RotationSelectionReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthObservation {
    Success,
    CountableTransient { provider_not_before_ms: Option<u64> },
    Busy,
    Cancelled,
    ClientError,
    LocalError,
    MonitoringFailure,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttemptObservation {
    pub execution: ExecutionObservation,
    pub health: HealthObservation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationLease {
    pub lease_id: LeaseId,
    pub candidate_id: String,
    pub request_id: RequestId,
    pub route_key: String,
    pub recovery: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchStartError {
    UnknownLease,
    BudgetRequestMismatch,
    BudgetExhausted,
    AlreadyStarted,
    RecoveryBudgetExhausted,
    CandidateChanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    NoEligibleCandidate,
    CandidateChanged,
    InvalidCandidate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettlementError {
    UnknownLease,
    BudgetRequestMismatch,
    NotDispatched,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationSettlement {
    pub candidate_id: String,
    pub attempt_id: Option<AttemptId>,
    pub retry: RetryDecision,
    pub circuit: CircuitSnapshot,
    /// Late outcomes may release their own lease, but cannot update the
    /// observations of a replaced member or route.
    pub(crate) observation_current: bool,
}
