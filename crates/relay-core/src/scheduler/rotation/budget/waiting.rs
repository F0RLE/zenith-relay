//! One accumulated admission budget, including recovery and transport handoffs.

use super::SharedRequestBudget;
use std::time::Duration;
use tokio::time::Instant;

mod diagnostics;
pub(crate) use diagnostics::RequestWaitKind;
use diagnostics::WaitStats;
pub use diagnostics::{RequestWaitDiagnostics, WaitSummary};

const MAX_ACCUMULATED_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionStopReason {
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
    kind: RequestWaitKind,
    stats: [WaitStats; 3],
    last_recovery_delay_ms: Option<u64>,
}

impl SharedRequestBudget {
    /// Conservative retained-envelope accounting. Repairs may grow it, but a
    /// driver/transport change cannot shrink or reset an existing charge.
    pub(crate) fn retain_input_bytes(&self, bytes: usize) {
        let mut waiting = crate::poison::mutex(&self.waiting);
        waiting.retained_bytes = waiting.retained_bytes.max(bytes);
    }

    pub(crate) fn retained_input_bytes(&self) -> usize {
        crate::poison::mutex(&self.waiting).retained_bytes
    }

    pub(crate) fn admission_stop_reason(&self) -> Option<AdmissionStopReason> {
        crate::poison::mutex(&self.waiting).stopped
    }

    pub(crate) fn stop_admission(&self, reason: AdmissionStopReason) {
        self.with_budget(|budget| budget.admission_stopped = true);
        crate::poison::mutex(&self.waiting)
            .stopped
            .get_or_insert(reason);
    }

    pub(crate) fn begin_queue_wait(&self, kind: RequestWaitKind) -> bool {
        let mut waiting = crate::poison::mutex(&self.waiting);
        if waiting.started.is_some() || waiting.stopped.is_some() {
            return false;
        }
        waiting.started = Some(Instant::now());
        waiting.kind = kind;
        let stats = &mut waiting.stats[kind.index()];
        stats.count = stats.count.saturating_add(1);
        if let RequestWaitKind::Recovery { delay_ms } = kind {
            waiting.last_recovery_delay_ms = Some(delay_ms);
        }
        true
    }

    pub(crate) fn finish_queue_wait(&self) {
        let mut waiting = crate::poison::mutex(&self.waiting);
        if let Some(started) = waiting.started.take() {
            let elapsed = started.elapsed();
            waiting.elapsed = waiting.elapsed.saturating_add(elapsed);
            let index = waiting.kind.index();
            let stats = &mut waiting.stats[index];
            stats.elapsed = stats.elapsed.saturating_add(elapsed);
        }
    }

    pub(crate) fn queue_deadline(&self, persistent_enabled: bool) -> Option<Instant> {
        let retry_deadline = self.with_budget(|budget| budget.retry_deadline);
        let waiting = crate::poison::mutex(&self.waiting);
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
