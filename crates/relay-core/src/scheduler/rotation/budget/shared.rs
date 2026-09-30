use super::super::*;
#[cfg(test)]
use super::AttemptId;
use super::{ExecutionObservation, QueueWaitBudget, RequestBudget, RequestId};

/// A request may cross an async transport boundary (WebSocket to HTTP), but
/// its dispatch counter must not be copied or held locked across an await.
#[derive(Clone, Debug)]
pub(crate) struct SharedRequestBudget {
    budget: Arc<Mutex<RequestBudget>>,
    pub(super) waiting: Arc<Mutex<QueueWaitBudget>>,
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
