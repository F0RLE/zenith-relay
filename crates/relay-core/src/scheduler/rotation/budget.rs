//! Request identity, dispatch budget and replay evidence.

use super::*;

mod waiting;

#[cfg(test)]
mod regression;
#[cfg(test)]
mod tests;
pub(crate) use waiting::{AdmissionStopReason, QueueWaitBudget};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RequestId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AttemptId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LeaseId(pub u64);

/// The one budget shared by every upstream dispatch belonging to a request.
///
/// A failed connection attempt still consumes a dispatch slot.  This prevents
/// nested adapter/executor retries from multiplying the request's real work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestBudget {
    request_id: RequestId,
    max_dispatches: u8,
    dispatches: u8,
    max_wire_attempts: u16,
    wire_attempts: u16,
    replay_stopped: bool,
    admission_stopped: bool,
    retry_started_at: Option<tokio::time::Instant>,
    retry_deadline: Option<tokio::time::Instant>,
    retry_window_ms: Option<u64>,
}

impl RequestBudget {
    /// Allocate an identity for a real incoming request. Recursive repair and
    /// auth replay must carry this same budget rather than calling this again.
    pub fn for_incoming_request(configured_limit: usize) -> Self {
        // The configured policy is authoritative.  The earlier prototype
        // silently reduced every value above the fixture default to three,
        // which made a persisted retry policy impossible to reason about.
        // Keep only the representation bound here; validation of the product
        // setting belongs to the owning runtime/configuration layer.
        let limit = u8::try_from(configured_limit.max(1)).unwrap_or(u8::MAX);
        Self::with_limits(
            RequestId(NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)),
            limit,
            u16::from(limit).saturating_mul(2),
        )
        .expect("clamped dispatch budget is non-zero")
    }

    pub fn new(request_id: RequestId, max_dispatches: u8) -> Option<Self> {
        Self::with_limits(request_id, max_dispatches, u16::from(max_dispatches))
    }

    pub fn with_limits(
        request_id: RequestId,
        max_dispatches: u8,
        max_wire_attempts: u16,
    ) -> Option<Self> {
        (max_dispatches > 0).then_some(Self {
            request_id,
            max_dispatches,
            dispatches: 0,
            max_wire_attempts: max_wire_attempts.max(1),
            wire_attempts: 0,
            replay_stopped: false,
            admission_stopped: false,
            retry_started_at: None,
            retry_deadline: None,
            retry_window_ms: None,
        })
    }

    pub fn default_for(request_id: RequestId) -> Self {
        // The constant is non-zero by construction; keeping the fallback here
        // makes the invariant obvious if the test profile is ever changed.
        Self::new(request_id, DEFAULT_MAX_DISPATCHES).expect("default dispatch budget is non-zero")
    }

    pub const fn request_id(self) -> RequestId {
        self.request_id
    }

    pub const fn max_dispatches(self) -> u8 {
        self.max_dispatches
    }

    pub const fn dispatches(self) -> u8 {
        self.dispatches
    }

    pub const fn max_wire_attempts(self) -> u16 {
        self.max_wire_attempts
    }

    pub const fn wire_attempts(self) -> u16 {
        self.wire_attempts
    }

    pub fn can_start_wire(self) -> bool {
        !self.replay_stopped
            && !self.admission_stopped
            && self.retry_window_open()
            && self.wire_attempts < self.max_wire_attempts
    }

    /// Counts a transport/handshake attempt. It is independent from the work
    /// dispatch counter so a failed connection cannot create an unbounded
    /// reconnect loop or consume more generations than the work policy allows.
    pub fn start_wire_attempt(&mut self) -> Option<u16> {
        if !self.can_start_wire() {
            return None;
        }
        self.wire_attempts = self.wire_attempts.saturating_add(1);
        Some(self.wire_attempts)
    }

    pub const fn remaining(self) -> u8 {
        self.max_dispatches.saturating_sub(self.dispatches)
    }

    pub fn can_dispatch(self) -> bool {
        !self.replay_stopped
            && !self.admission_stopped
            && self.retry_window_open()
            && self.dispatches < self.max_dispatches
    }

    /// Once remote execution is accepted or uncertain, no adapter may restart
    /// this request through another transport or compatibility branch.
    pub fn observe_execution(&mut self, observation: ExecutionObservation) {
        self.replay_stopped |= observation.certainty != ExecutionCertainty::NotSent
            || observation.commit_stage == CommitStage::ResponseCommitted;
        if !self.replay_stopped {
            let started = *self
                .retry_started_at
                .get_or_insert_with(tokio::time::Instant::now);
            self.retry_deadline = self
                .retry_window_ms
                .map(|window_ms| started + std::time::Duration::from_millis(window_ms));
        }
    }

    fn retry_window_open(self) -> bool {
        self.retry_deadline
            .is_none_or(|deadline| tokio::time::Instant::now() < deadline)
    }

    /// Window policy can change explicitly while waiting; its monotonic start
    /// cannot change when a driver, protocol, owner or compatibility path does.
    fn configure_retry_window(&mut self, window_ms: u64, persistent: bool) {
        self.retry_window_ms = (!persistent).then_some(window_ms);
        self.retry_deadline = self
            .retry_started_at
            .filter(|_| !persistent)
            .map(|started| started + std::time::Duration::from_millis(window_ms));
    }

    /// Counts one actual upstream dispatch start.  Reservation and preview do
    /// not consume the budget.
    pub fn start_dispatch(&mut self) -> Option<AttemptId> {
        if !self.can_dispatch() {
            return None;
        }
        self.dispatches = self.dispatches.saturating_add(1);
        Some(AttemptId(u64::from(self.dispatches)))
    }

    pub fn retry_decision(&self, observation: ExecutionObservation) -> RetryDecision {
        if observation.commit_stage == CommitStage::ResponseCommitted {
            return RetryDecision::Stop(RetryStopReason::ResponseCommitted);
        }
        if observation.certainty == ExecutionCertainty::Unknown {
            return RetryDecision::Stop(RetryStopReason::RemoteOutcomeUnknown);
        }
        // Repeatable bytes alone do not prove rejection or portable ownership.
        // The ordinary path permits only a proven pre-execution outcome.
        let replay_safe = observation.certainty == ExecutionCertainty::NotSent;
        if !replay_safe {
            return RetryDecision::Stop(RetryStopReason::ReplayUnavailable);
        }
        if !self.can_dispatch() {
            return RetryDecision::Stop(RetryStopReason::BudgetExhausted);
        }
        RetryDecision::Retry {
            next_dispatch: self.dispatches.saturating_add(1),
        }
    }

    pub fn retry_decision_with_evidence(
        &self,
        observation: ExecutionObservation,
        evidence: RetryEvidence,
    ) -> RetryDecision {
        if observation.commit_stage == CommitStage::ResponseCommitted {
            return RetryDecision::Stop(RetryStopReason::ResponseCommitted);
        }
        let execution_matches = match evidence.execution {
            ExecutionEvidence::NotSent | ExecutionEvidence::RejectedBeforeExecution => {
                observation.certainty == ExecutionCertainty::NotSent
            }
            ExecutionEvidence::Accepted => observation.certainty == ExecutionCertainty::Accepted,
            ExecutionEvidence::Unknown => observation.certainty == ExecutionCertainty::Unknown,
            ExecutionEvidence::Terminal => false,
        };
        if !evidence.input_repeatable
            || !evidence.target_portable
            || !execution_matches
            || !evidence.execution.replay_allowed(evidence.idempotency)
        {
            return RetryDecision::Stop(match evidence.execution {
                ExecutionEvidence::Unknown => RetryStopReason::RemoteOutcomeUnknown,
                ExecutionEvidence::Terminal => RetryStopReason::TerminalOutcome,
                _ => RetryStopReason::ReplayUnavailable,
            });
        }
        if !self.can_dispatch() {
            return RetryDecision::Stop(RetryStopReason::BudgetExhausted);
        }
        RetryDecision::Retry {
            next_dispatch: self.dispatches.saturating_add(1),
        }
    }
}

/// The four independent proofs required before a non-trivial operation may be
/// sent to another route.  A repeatable body alone is deliberately not enough.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryEvidence {
    pub input_repeatable: bool,
    pub execution: ExecutionEvidence,
    pub target_portable: bool,
    pub idempotency: IdempotencyContract,
}

impl RetryEvidence {
    pub const fn pre_execution() -> Self {
        Self {
            input_repeatable: true,
            execution: ExecutionEvidence::RejectedBeforeExecution,
            target_portable: true,
            idempotency: IdempotencyContract::None,
        }
    }

    pub const fn unknown() -> Self {
        Self {
            input_repeatable: false,
            execution: ExecutionEvidence::Unknown,
            target_portable: false,
            idempotency: IdempotencyContract::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionEvidence {
    NotSent,
    RejectedBeforeExecution,
    Accepted,
    Unknown,
    Terminal,
}

impl ExecutionEvidence {
    const fn replay_allowed(self, idempotency: IdempotencyContract) -> bool {
        matches!(self, Self::NotSent | Self::RejectedBeforeExecution)
            || (matches!(self, Self::Accepted)
                && matches!(idempotency, IdempotencyContract::Proven))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdempotencyContract {
    None,
    /// The adapter has verified deduplication for this exact operation,
    /// identity, endpoint and retention window.
    Proven,
}

/// A request may cross an async transport boundary (WebSocket to HTTP), but
/// its dispatch counter must not be copied or held locked across an await.
#[derive(Clone, Debug)]
pub(crate) struct SharedRequestBudget {
    budget: Arc<Mutex<RequestBudget>>,
    waiting: Arc<Mutex<QueueWaitBudget>>,
    /// Admission incompatibility excludes an exact route. A real dispatch
    /// instead visits a physical member, across every driver of this request.
    /// At most one entry per dispatch can be added, so this is budget-bounded.
    attempted_members: Arc<Mutex<BTreeSet<String>>>,
}

impl SharedRequestBudget {
    pub(crate) fn for_incoming_request(configured_limit: usize) -> Self {
        Self {
            budget: Arc::new(Mutex::new(RequestBudget::for_incoming_request(
                configured_limit,
            ))),
            attempted_members: Arc::default(),
            waiting: Arc::default(),
        }
    }

    pub(crate) fn can_dispatch(&self) -> bool {
        self.budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .can_dispatch()
    }

    #[cfg(test)]
    pub(crate) fn start_dispatch(&self) -> Option<AttemptId> {
        self.with_budget(RequestBudget::start_dispatch)
    }

    pub(crate) fn dispatches(&self) -> u8 {
        self.budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .dispatches()
    }

    pub(crate) fn request_id(&self) -> RequestId {
        self.budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .request_id()
    }

    pub(crate) fn with_budget<R>(&self, callback: impl FnOnce(&mut RequestBudget) -> R) -> R {
        let mut budget = self
            .budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        callback(&mut budget)
    }

    pub(crate) fn start_wire_attempt(&self) -> Option<u16> {
        self.budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .start_wire_attempt()
    }

    pub(crate) fn configure_retry_window(&self, window_ms: u64, persistent: bool) {
        self.with_budget(|budget| budget.configure_retry_window(window_ms, persistent));
        self.waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .persistent = persistent;
    }

    pub(crate) fn attempted_members(&self) -> BTreeSet<String> {
        self.attempted_members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn record_member_attempt(&self, member: &str) {
        self.attempted_members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(member.to_owned());
    }

    /// A verified compatibility repair may retry its owner; it cannot refund
    /// dispatches or undo unknown/commit/cancel evidence.
    pub(crate) fn allow_member_repair(&self, member: &str) {
        self.attempted_members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(member);
    }

    /// Called only after the controller awaited actionable scheduler recovery.
    pub(crate) fn begin_recovery_pass(&self) {
        self.attempted_members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub(crate) fn retry_wait_deadline(&self, window_ms: u64) -> tokio::time::Instant {
        self.with_budget(|budget| {
            let started = *budget
                .retry_started_at
                .get_or_insert_with(tokio::time::Instant::now);
            budget.retry_deadline = budget
                .retry_window_ms
                .map(|window_ms| started + std::time::Duration::from_millis(window_ms));
            started + std::time::Duration::from_millis(window_ms)
        })
    }

    pub(crate) fn observe_rejection(&self) {
        self.with_budget(|budget| budget.observe_execution(ExecutionObservation::not_sent()));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionCertainty {
    /// The adapter proved that no upstream request bytes were accepted.
    NotSent,
    /// The provider accepted the request, but a complete response has not
    /// necessarily been observed.  Retry requires an explicit replay contract.
    Accepted,
    /// The transport ended without proving whether the provider accepted work.
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitStage {
    BeforeOutput,
    ResponseCommitted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionObservation {
    pub certainty: ExecutionCertainty,
    pub commit_stage: CommitStage,
}

impl ExecutionObservation {
    pub const fn not_sent() -> Self {
        Self {
            certainty: ExecutionCertainty::NotSent,
            commit_stage: CommitStage::BeforeOutput,
        }
    }

    pub const fn accepted() -> Self {
        Self {
            certainty: ExecutionCertainty::Accepted,
            commit_stage: CommitStage::BeforeOutput,
        }
    }

    pub const fn unknown() -> Self {
        Self {
            certainty: ExecutionCertainty::Unknown,
            commit_stage: CommitStage::BeforeOutput,
        }
    }

    pub const fn committed() -> Self {
        Self {
            certainty: ExecutionCertainty::Accepted,
            commit_stage: CommitStage::ResponseCommitted,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryStopReason {
    Succeeded,
    TerminalOutcome,
    BudgetExhausted,
    ResponseCommitted,
    RemoteOutcomeUnknown,
    ReplayUnavailable,
    Cancelled,
    TerminalHealthObservation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryDecision {
    Retry { next_dispatch: u8 },
    Stop(RetryStopReason),
}
