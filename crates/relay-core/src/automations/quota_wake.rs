use crate::quota::{QuotaWindow, QuotaWindowKind};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

mod coordinator;
mod policy;

pub use policy::{
    model_lightness_rank, AccountSelector, WakeAdapterPolicy, WakeExecutionPolicy, WakeModel,
    WakeModelPolicy, WakePolicyAdapter, WakeTask, WakeTaskValidationError, WakeTrigger,
};

const MAX_WAKE_ATTEMPTS: u8 = 2;
const MAX_WAKE_JITTER_SECONDS: u32 = 3_600;
const MAX_VERIFICATION_DELAY_MS: u64 = 10 * 60_000;
const MAX_OUTPUT_TOKEN_CAP: u16 = 256;
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeOutcome {
    Confirmed,
    Unconfirmed,
    SkippedAlreadyStarted,
    SkippedDuplicate,
    SkippedIneligible,
    SkippedCapacity,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeHistory {
    pub task_id: String,
    pub account_id: String,
    pub window_kind: QuotaWindowKind,
    pub transition_fingerprint: String,
    pub model_id: Option<String>,
    pub trigger: WakeTrigger,
    pub attempt: u8,
    pub outcome: WakeOutcome,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
    pub latency_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeExecutionRequest {
    pub account_id: String,
    pub model_id: String,
    pub window_kind: QuotaWindowKind,
    pub output_token_cap: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeVerificationMetadata {
    pub window_kind: QuotaWindowKind,
    pub baseline_window: Option<QuotaWindow>,
    pub verify_after_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeVerificationOutcome {
    ConfirmedQuotaConsumed,
    ConfirmedCountdownAdvanced,
    Unconfirmed,
}

pub fn verify_wake_countdown(
    before: Option<&QuotaWindow>,
    after: Option<&QuotaWindow>,
) -> WakeVerificationOutcome {
    let (Some(before), Some(after)) = (before, after) else {
        return WakeVerificationOutcome::Unconfirmed;
    };
    if before.kind != after.kind
        || after.observed_at_ms <= before.observed_at_ms
        || !known_full_state(before)
        || !known_full_state(after)
        || !before.is_fully_available()
    {
        return WakeVerificationOutcome::Unconfirmed;
    }
    if !after.is_fully_available() {
        return WakeVerificationOutcome::ConfirmedQuotaConsumed;
    }
    if matches!(
        (before.reset_at_ms, after.reset_at_ms),
        (Some(before_reset), Some(after_reset))
            if after_reset > before_reset && after_reset > after.observed_at_ms
    ) {
        return WakeVerificationOutcome::ConfirmedCountdownAdvanced;
    }
    WakeVerificationOutcome::Unconfirmed
}

fn known_full_state(window: &QuotaWindow) -> bool {
    window.explicitly_full.is_some() || window.available_basis_points.is_some()
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum WakeCycleStatus {
    Pending,
    InFlight,
    Completed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct WakeCycle {
    key: String,
    status: WakeCycleStatus,
    task_id: String,
    account_id: String,
    window_kind: QuotaWindowKind,
    transition_fingerprint: String,
    trigger: WakeTrigger,
    requires_confirmation: bool,
    request: Option<WakeExecutionRequest>,
    verification: Option<WakeVerificationMetadata>,
    jitter_seconds: u32,
    max_attempts: u8,
    attempts_started: u8,
    due_at_ms: u64,
    recorded_at_ms: u64,
}

impl WakeCycle {
    fn schedule(&self) -> Option<WakeSchedule> {
        if self.status != WakeCycleStatus::Pending {
            return None;
        }
        Some(WakeSchedule {
            cycle_key: self.key.clone(),
            due_at_ms: self.due_at_ms,
            attempts_started: self.attempts_started,
            max_attempts: self.max_attempts,
            request: self.request.clone()?,
        })
    }

    fn finish(&mut self, now_ms: u64) {
        self.status = WakeCycleStatus::Completed;
        self.request = None;
        self.verification = None;
        self.recorded_at_ms = now_ms;
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeAutomationState {
    max_cycles: usize,
    max_history: usize,
    cycles: VecDeque<WakeCycle>,
    history: VecDeque<WakeHistory>,
}

impl WakeAutomationState {
    pub fn new(max_cycles: usize, max_history: usize) -> Result<Self, &'static str> {
        if max_cycles == 0 || max_history == 0 {
            return Err("wake state bounds must be positive");
        }
        Ok(Self {
            max_cycles,
            max_history,
            cycles: VecDeque::new(),
            history: VecDeque::new(),
        })
    }

    pub fn history(&self) -> &VecDeque<WakeHistory> {
        &self.history
    }

    /// Migrate a task's unfinished cycles without changing attempts, due times,
    /// or completed history. Used when a host retires manual execution.
    pub fn clear_task_confirmation_requirement(&mut self, task_id: &str) {
        for cycle in &mut self.cycles {
            if cycle.task_id == task_id && cycle.status != WakeCycleStatus::Completed {
                cycle.requires_confirmation = false;
            }
        }
    }
}

#[derive(Clone)]
pub struct WakeCoordinator {
    state: WakeAutomationState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WakeDecision {
    Scheduled(WakeSchedule),
    Skipped(WakeOutcome),
    Rejected(WakeTaskValidationError),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeSchedule {
    pub cycle_key: String,
    pub due_at_ms: u64,
    pub attempts_started: u8,
    pub max_attempts: u8,
    pub request: WakeExecutionRequest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakePermit {
    pub cycle_key: String,
    pub task_id: String,
    pub account_id: String,
    pub window_kind: QuotaWindowKind,
    pub transition_fingerprint: String,
    pub model_id: String,
    pub trigger: WakeTrigger,
    pub requires_confirmation: bool,
    pub verification_delay_ms: u64,
    pub output_token_cap: u16,
    pub attempt: u8,
    pub due_at_ms: u64,
    pub reserved_at_ms: u64,
    pub request: WakeExecutionRequest,
    pub verification: WakeVerificationMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeCompletionOutcome {
    Confirmed,
    Unconfirmed,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WakeCompletion {
    pub outcome: WakeCompletionOutcome,
    pub completed_at_ms: u64,
    pub latency_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub error_code: Option<String>,
}

pub(super) fn is_safe_id(identifier: &str) -> bool {
    crate::is_ascii_token(identifier.trim(), 64)
}

#[cfg(test)]
mod tests;
