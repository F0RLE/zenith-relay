//! Fixed-size local wait counters. No route IDs, headers or request content.
use super::{AdmissionStopReason, SharedRequestBudget};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum RequestWaitKind {
    #[default]
    Capacity,
    Recovery {
        delay_ms: u64,
    },
    PoolEvent,
}

impl RequestWaitKind {
    pub(super) fn index(self) -> usize {
        match self {
            Self::Capacity => 0,
            Self::Recovery { .. } => 1,
            Self::PoolEvent => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct WaitStats {
    pub(super) count: u32,
    pub(super) elapsed: Duration,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitSummary {
    pub count: u32,
    pub duration_ms: u64,
}

/// Cumulative per logical request; never sum snapshots across its attempts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestWaitDiagnostics {
    pub total_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity: Option<WaitSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<WaitSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_event: Option<WaitSummary>,
    /// Scheduler's proposed recovery delay, not a raw provider Retry-After.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_recovery_delay_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<AdmissionStopReason>,
}

impl SharedRequestBudget {
    pub(crate) fn wait_diagnostics(&self) -> Option<RequestWaitDiagnostics> {
        let waiting = crate::poison::mutex(&self.waiting);
        if waiting.stats.iter().all(|stats| stats.count == 0) && waiting.stopped.is_none() {
            return None;
        }
        let active = waiting
            .started
            .map_or(Duration::ZERO, |start| start.elapsed());
        let summary = |index: usize| {
            let stats = waiting.stats[index];
            (stats.count > 0).then(|| WaitSummary {
                count: stats.count,
                duration_ms: millis(stats.elapsed.saturating_add(
                    if index == waiting.kind.index() {
                        active
                    } else {
                        Duration::ZERO
                    },
                )),
            })
        };
        Some(RequestWaitDiagnostics {
            total_ms: millis(waiting.elapsed.saturating_add(active)),
            capacity: summary(0),
            recovery: summary(1),
            pool_event: summary(2),
            last_recovery_delay_ms: waiting.last_recovery_delay_ms,
            stop_reason: waiting.stopped,
        })
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
