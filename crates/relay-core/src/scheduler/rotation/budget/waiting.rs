//! One accumulated admission budget, including recovery and transport handoffs.

use super::SharedRequestBudget;
use std::time::Duration;
use tokio::time::Instant;

const MAX_ACCUMULATED_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AdmissionStopReason {
    QueueFull,
    WaitExpired,
}

#[derive(Debug, Default)]
pub(crate) struct QueueWaitBudget {
    pub(super) persistent: bool,
    retained_bytes: usize,
    elapsed: Duration,
    started: Option<Instant>,
    stopped: Option<AdmissionStopReason>,
}

impl SharedRequestBudget {
    /// Conservative retained-envelope accounting. Repairs may grow it, but a
    /// driver/transport change cannot shrink or reset an existing charge.
    pub(crate) fn retain_input_bytes(&self, bytes: usize) {
        let mut waiting = self
            .waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        waiting.retained_bytes = waiting.retained_bytes.max(bytes);
    }

    pub(crate) fn retained_input_bytes(&self) -> usize {
        self.waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retained_bytes
    }

    pub(crate) fn admission_stop_reason(&self) -> Option<AdmissionStopReason> {
        self.waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stopped
    }

    pub(crate) fn stop_admission(&self, reason: AdmissionStopReason) {
        self.with_budget(|budget| budget.admission_stopped = true);
        self.waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stopped
            .get_or_insert(reason);
    }

    pub(crate) fn begin_queue_wait(&self) -> bool {
        let mut waiting = self
            .waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if waiting.started.is_some() || waiting.stopped.is_some() {
            return false;
        }
        waiting.started = Some(Instant::now());
        true
    }

    pub(crate) fn finish_queue_wait(&self) {
        let mut waiting = self
            .waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(started) = waiting.started.take() {
            waiting.elapsed = waiting.elapsed.saturating_add(started.elapsed());
        }
    }

    pub(crate) fn queue_deadline(&self, persistent_enabled: bool) -> Option<Instant> {
        let retry_deadline = self.with_budget(|budget| budget.retry_deadline);
        let waiting = self
            .waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let queue_deadline = (!(waiting.persistent && persistent_enabled)).then(|| {
            waiting.started.unwrap_or_else(Instant::now)
                + MAX_ACCUMULATED_WAIT.saturating_sub(waiting.elapsed)
        });
        match (queue_deadline, retry_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}
