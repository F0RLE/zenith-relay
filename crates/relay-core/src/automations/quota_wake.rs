use crate::accounts::{AccountAuthMode, AccountRecord};
use crate::error::safe_error_code;
use crate::quota::{QuotaTransition, QuotaWindow, QuotaWindowKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;

mod policy;

use policy::select_model;
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
}

#[derive(Clone)]
pub struct WakeCoordinator {
    state: WakeAutomationState,
}

impl WakeCoordinator {
    pub fn new(max_cycles: usize, max_history: usize) -> Result<Self, &'static str> {
        WakeAutomationState::new(max_cycles, max_history).map(|state| Self { state })
    }

    pub fn from_state(mut state: WakeAutomationState) -> Result<Self, &'static str> {
        if state.max_cycles == 0
            || state.max_history == 0
            || state.cycles.len() > state.max_cycles
            || state.history.len() > state.max_history
            || state.cycles.iter().any(|cycle| !valid_cycle(cycle))
        {
            return Err("wake state bounds are invalid");
        }
        for cycle in &mut state.cycles {
            if cycle.status == WakeCycleStatus::InFlight {
                if cycle.attempts_started < cycle.max_attempts {
                    cycle.status = WakeCycleStatus::Pending;
                    cycle.due_at_ms = cycle.recorded_at_ms.saturating_add(deterministic_jitter_ms(
                        &cycle.key,
                        cycle.attempts_started.saturating_add(1),
                        cycle.jitter_seconds,
                    ));
                } else {
                    cycle.finish(cycle.recorded_at_ms);
                }
            }
        }
        for history in &mut state.history {
            history.error_code = history.error_code.take().map(|code| safe_error_code(&code));
        }
        let mut coordinator = Self { state };
        coordinator.cancel_matching(
            |cycle| cycle.window_kind != QuotaWindowKind::Primary,
            0,
            "wake_window_redundant",
        );
        Ok(coordinator)
    }

    pub fn state(&self) -> &WakeAutomationState {
        &self.state
    }

    pub fn into_state(self) -> WakeAutomationState {
        self.state
    }

    pub fn pending(&self) -> Vec<WakeSchedule> {
        self.state
            .cycles
            .iter()
            .filter_map(WakeCycle::schedule)
            .collect()
    }

    pub fn next_automatic_due(&self) -> Option<u64> {
        self.state
            .cycles
            .iter()
            .filter(|cycle| {
                cycle.status == WakeCycleStatus::Pending
                    && !cycle.requires_confirmation
                    && cycle.attempts_started < cycle.max_attempts
            })
            .map(|cycle| cycle.due_at_ms)
            .min()
    }

    pub fn evaluate(
        &mut self,
        task: &WakeTask,
        account: &AccountRecord,
        transition: &QuotaTransition,
        last_natural_use_at_ms: Option<u64>,
        policy: &WakeAdapterPolicy,
        now_ms: u64,
    ) -> WakeDecision {
        if let Err(error) = task.validate() {
            return WakeDecision::Rejected(error);
        }
        if !policy.is_valid() {
            return WakeDecision::Rejected(WakeTaskValidationError::InvalidAdapterPolicy);
        }
        if !task.enabled
            || account.auth_mode != AccountAuthMode::OAuth
            || !task.account_selector.matches(account)
            || transition.window_kind != QuotaWindowKind::Primary
            || !task.window_kinds.contains(&QuotaWindowKind::Primary)
            || transition.fingerprint.trim().is_empty()
            || !account.is_wake_eligible()
            || !policy
                .windows_requiring_activity
                .contains(&transition.window_kind)
        {
            return WakeDecision::Skipped(WakeOutcome::SkippedIneligible);
        }

        let cycle_key = cycle_key(account, transition);
        if last_natural_use_at_ms
            .is_some_and(|last_used| last_used >= transition.transitioned_at_ms)
        {
            return self.complete_by_natural_use(task, account, transition, &cycle_key, now_ms);
        }
        if self.state.cycles.iter().any(|cycle| {
            same_cycle(
                cycle,
                &account.id,
                transition.window_kind,
                &transition.fingerprint,
            )
        }) {
            return WakeDecision::Skipped(WakeOutcome::SkippedDuplicate);
        }

        let Some(model_id) = select_model(&task.model_policy, &policy.models) else {
            return WakeDecision::Skipped(WakeOutcome::SkippedIneligible);
        };
        let request = WakeExecutionRequest {
            account_id: account.id.clone(),
            model_id,
            window_kind: transition.window_kind,
            output_token_cap: policy.output_token_cap,
        };
        let verification = WakeVerificationMetadata {
            window_kind: transition.window_kind,
            baseline_window: account.quota.window(transition.window_kind).cloned(),
            verify_after_ms: policy.verification_delay_ms,
        };
        let due_at_ms =
            now_ms.saturating_add(deterministic_jitter_ms(&cycle_key, 1, task.jitter_seconds));
        let cycle = WakeCycle {
            key: cycle_key,
            status: WakeCycleStatus::Pending,
            task_id: task.id.clone(),
            account_id: account.id.clone(),
            window_kind: transition.window_kind,
            transition_fingerprint: transition.fingerprint.clone(),
            trigger: task.trigger,
            requires_confirmation: task.execution_policy
                == WakeExecutionPolicy::RequireConfirmation,
            request: Some(request),
            verification: Some(verification),
            jitter_seconds: task.jitter_seconds,
            max_attempts: task.max_attempts_per_cycle,
            attempts_started: 0,
            due_at_ms,
            recorded_at_ms: now_ms,
        };
        let schedule = cycle
            .schedule()
            .expect("new pending wake cycle must contain execution metadata");
        if !self.reserve_cycle(cycle) {
            return WakeDecision::Skipped(WakeOutcome::SkippedCapacity);
        }
        WakeDecision::Scheduled(schedule)
    }

    pub fn claim_due(&mut self, now_ms: u64, max_claims: usize) -> Vec<WakePermit> {
        self.claim_due_matching(now_ms, max_claims, WakeClaimMode::Any)
    }

    pub fn claim_due_automatic(&mut self, now_ms: u64, max_claims: usize) -> Vec<WakePermit> {
        self.claim_due_matching(now_ms, max_claims, WakeClaimMode::Automatic)
    }

    pub fn claim_due_confirmations(&mut self, now_ms: u64, max_claims: usize) -> Vec<WakePermit> {
        self.claim_due_matching(now_ms, max_claims, WakeClaimMode::Confirmation)
    }

    pub fn remove_pending_for_task(&mut self, task_id: &str, completed_at_ms: u64) -> usize {
        self.cancel_matching(
            |cycle| cycle.task_id == task_id,
            completed_at_ms,
            "wake_task_canceled",
        )
    }

    pub fn remove_pending_for_account(&mut self, account_id: &str, completed_at_ms: u64) -> usize {
        self.cancel_matching(
            |cycle| cycle.account_id == account_id,
            completed_at_ms,
            "wake_account_canceled",
        )
    }

    pub fn mark_natural_use_for_account(&mut self, account_id: &str, used_at_ms: u64) -> usize {
        let mut history = Vec::new();
        for cycle in &mut self.state.cycles {
            if cycle.status == WakeCycleStatus::Completed || cycle.account_id != account_id {
                continue;
            }
            history.push(natural_use_history(cycle, used_at_ms));
            cycle.finish(used_at_ms);
        }
        let completed = history.len();
        for entry in history {
            self.push_history(entry);
        }
        completed
    }

    pub fn is_permit_active(&self, permit: &WakePermit) -> bool {
        self.state.cycles.iter().any(|cycle| {
            cycle.key == permit.cycle_key
                && cycle.status == WakeCycleStatus::InFlight
                && cycle.attempts_started == permit.attempt
                && cycle.task_id == permit.task_id
                && cycle.account_id == permit.account_id
                && cycle.window_kind == permit.window_kind
                && cycle.transition_fingerprint == permit.transition_fingerprint
        })
    }

    fn claim_due_matching(
        &mut self,
        now_ms: u64,
        max_claims: usize,
        mode: WakeClaimMode,
    ) -> Vec<WakePermit> {
        if max_claims == 0 {
            return Vec::new();
        }
        let mut permits = Vec::new();
        for cycle in &mut self.state.cycles {
            if permits.len() >= max_claims {
                break;
            }
            if cycle.status != WakeCycleStatus::Pending
                || cycle.due_at_ms > now_ms
                || cycle.attempts_started >= cycle.max_attempts
                || !mode.matches(cycle)
            {
                continue;
            }
            let (Some(request), Some(verification)) =
                (cycle.request.clone(), cycle.verification.clone())
            else {
                continue;
            };
            cycle.status = WakeCycleStatus::InFlight;
            cycle.attempts_started = cycle.attempts_started.saturating_add(1);
            cycle.recorded_at_ms = now_ms;
            permits.push(WakePermit {
                cycle_key: cycle.key.clone(),
                task_id: cycle.task_id.clone(),
                account_id: cycle.account_id.clone(),
                window_kind: cycle.window_kind,
                transition_fingerprint: cycle.transition_fingerprint.clone(),
                model_id: request.model_id.clone(),
                trigger: cycle.trigger,
                requires_confirmation: cycle.requires_confirmation,
                verification_delay_ms: verification.verify_after_ms,
                output_token_cap: request.output_token_cap,
                attempt: cycle.attempts_started,
                due_at_ms: cycle.due_at_ms,
                reserved_at_ms: now_ms,
                request,
                verification,
            });
        }
        permits
    }

    pub fn complete(&mut self, permit: WakePermit, completion: WakeCompletion) -> bool {
        let Some(index) = self.state.cycles.iter().position(|cycle| {
            cycle.key == permit.cycle_key
                && cycle.status == WakeCycleStatus::InFlight
                && cycle.attempts_started == permit.attempt
        }) else {
            return false;
        };

        let completed_at_ms = completion.completed_at_ms.max(permit.reserved_at_ms);
        let outcome = match completion.outcome {
            WakeCompletionOutcome::Confirmed => WakeOutcome::Confirmed,
            WakeCompletionOutcome::Unconfirmed => WakeOutcome::Unconfirmed,
            WakeCompletionOutcome::Failed => WakeOutcome::Failed,
        };
        let history = {
            let cycle = &mut self.state.cycles[index];
            let model_id = cycle
                .request
                .as_ref()
                .map(|request| request.model_id.clone());
            if completion.outcome == WakeCompletionOutcome::Confirmed
                || cycle.attempts_started >= cycle.max_attempts
            {
                cycle.finish(completed_at_ms);
            } else {
                cycle.status = WakeCycleStatus::Pending;
                cycle.due_at_ms = completed_at_ms.saturating_add(deterministic_jitter_ms(
                    &cycle.key,
                    cycle.attempts_started.saturating_add(1),
                    cycle.jitter_seconds,
                ));
                cycle.recorded_at_ms = completed_at_ms;
            }
            WakeHistory {
                task_id: cycle.task_id.clone(),
                account_id: cycle.account_id.clone(),
                window_kind: cycle.window_kind,
                transition_fingerprint: cycle.transition_fingerprint.clone(),
                model_id,
                trigger: cycle.trigger,
                attempt: permit.attempt,
                outcome,
                started_at_ms: permit.reserved_at_ms,
                completed_at_ms,
                latency_ms: completion.latency_ms,
                input_tokens: completion.input_tokens,
                output_tokens: completion.output_tokens,
                error_code: completion.error_code.map(|code| safe_error_code(&code)),
            }
        };
        self.push_history(history);
        true
    }

    fn cancel_matching(
        &mut self,
        matches: impl Fn(&WakeCycle) -> bool,
        completed_at_ms: u64,
        error_code: &str,
    ) -> usize {
        let mut history = Vec::new();
        for cycle in &mut self.state.cycles {
            if cycle.status == WakeCycleStatus::Completed || !matches(cycle) {
                continue;
            }
            history.push(canceled_history(cycle, completed_at_ms, error_code));
            cycle.finish(completed_at_ms);
        }
        let completed = history.len();
        for entry in history {
            self.push_history(entry);
        }
        completed
    }

    fn complete_by_natural_use(
        &mut self,
        task: &WakeTask,
        account: &AccountRecord,
        transition: &QuotaTransition,
        cycle_key: &str,
        now_ms: u64,
    ) -> WakeDecision {
        if let Some(index) = self.state.cycles.iter().position(|cycle| {
            same_cycle(
                cycle,
                &account.id,
                transition.window_kind,
                &transition.fingerprint,
            )
        }) {
            if self.state.cycles[index].status == WakeCycleStatus::Completed {
                return WakeDecision::Skipped(WakeOutcome::SkippedDuplicate);
            }
            let history = natural_use_history(&self.state.cycles[index], now_ms);
            self.state.cycles[index].finish(now_ms);
            self.push_history(history);
            return WakeDecision::Skipped(WakeOutcome::SkippedAlreadyStarted);
        }

        let cycle = WakeCycle {
            key: cycle_key.to_string(),
            status: WakeCycleStatus::Completed,
            task_id: task.id.clone(),
            account_id: account.id.clone(),
            window_kind: transition.window_kind,
            transition_fingerprint: transition.fingerprint.clone(),
            trigger: task.trigger,
            requires_confirmation: false,
            request: None,
            verification: None,
            jitter_seconds: task.jitter_seconds,
            max_attempts: task.max_attempts_per_cycle,
            attempts_started: 0,
            due_at_ms: now_ms,
            recorded_at_ms: now_ms,
        };
        let history = natural_use_history(&cycle, now_ms);
        if !self.reserve_cycle(cycle) {
            return WakeDecision::Skipped(WakeOutcome::SkippedCapacity);
        }
        self.push_history(history);
        WakeDecision::Skipped(WakeOutcome::SkippedAlreadyStarted)
    }

    fn reserve_cycle(&mut self, cycle: WakeCycle) -> bool {
        if self.state.cycles.len() >= self.state.max_cycles {
            if let Some(index) = self
                .state
                .cycles
                .iter()
                .position(|cycle| cycle.status == WakeCycleStatus::Completed)
            {
                self.state.cycles.remove(index);
            } else {
                return false;
            }
        }
        self.state.cycles.push_back(cycle);
        true
    }

    fn push_history(&mut self, history: WakeHistory) {
        if self.state.history.len() >= self.state.max_history {
            self.state.history.pop_front();
        }
        self.state.history.push_back(history);
    }
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

#[derive(Clone, Copy)]
enum WakeClaimMode {
    Any,
    Automatic,
    Confirmation,
}

impl WakeClaimMode {
    fn matches(self, cycle: &WakeCycle) -> bool {
        match self {
            Self::Any => true,
            Self::Automatic => !cycle.requires_confirmation,
            Self::Confirmation => cycle.requires_confirmation,
        }
    }
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

fn valid_cycle(cycle: &WakeCycle) -> bool {
    !cycle.key.is_empty()
        && is_safe_id(&cycle.task_id)
        && !cycle.account_id.trim().is_empty()
        && !cycle.transition_fingerprint.trim().is_empty()
        && (1..=MAX_WAKE_ATTEMPTS).contains(&cycle.max_attempts)
        && cycle.attempts_started <= cycle.max_attempts
        && cycle.jitter_seconds <= MAX_WAKE_JITTER_SECONDS
        && (cycle.status == WakeCycleStatus::Completed
            || (cycle.request.is_some() && cycle.verification.is_some()))
}

fn natural_use_history(cycle: &WakeCycle, used_at_ms: u64) -> WakeHistory {
    WakeHistory {
        task_id: cycle.task_id.clone(),
        account_id: cycle.account_id.clone(),
        window_kind: cycle.window_kind,
        transition_fingerprint: cycle.transition_fingerprint.clone(),
        model_id: None,
        trigger: cycle.trigger,
        attempt: cycle.attempts_started,
        outcome: WakeOutcome::SkippedAlreadyStarted,
        started_at_ms: used_at_ms,
        completed_at_ms: used_at_ms,
        latency_ms: None,
        input_tokens: None,
        output_tokens: None,
        error_code: None,
    }
}

fn canceled_history(cycle: &WakeCycle, completed_at_ms: u64, error_code: &str) -> WakeHistory {
    WakeHistory {
        task_id: cycle.task_id.clone(),
        account_id: cycle.account_id.clone(),
        window_kind: cycle.window_kind,
        transition_fingerprint: cycle.transition_fingerprint.clone(),
        model_id: None,
        trigger: cycle.trigger,
        attempt: cycle.attempts_started,
        outcome: WakeOutcome::SkippedIneligible,
        started_at_ms: cycle.recorded_at_ms,
        completed_at_ms: completed_at_ms.max(cycle.recorded_at_ms),
        latency_ms: None,
        input_tokens: None,
        output_tokens: None,
        error_code: Some(safe_error_code(error_code)),
    }
}

fn cycle_key(account: &AccountRecord, transition: &QuotaTransition) -> String {
    hex::encode(Sha256::digest(
        format!(
            "{}\0{:?}\0{}",
            account.id, transition.window_kind, transition.fingerprint,
        )
        .as_bytes(),
    ))
}

fn same_cycle(
    cycle: &WakeCycle,
    account_id: &str,
    window_kind: QuotaWindowKind,
    transition_fingerprint: &str,
) -> bool {
    cycle.account_id == account_id
        && cycle.window_kind == window_kind
        && cycle.transition_fingerprint == transition_fingerprint
}

fn deterministic_jitter_ms(cycle_key: &str, attempt: u8, jitter_seconds: u32) -> u64 {
    if jitter_seconds == 0 {
        return 0;
    }
    let digest = Sha256::digest(format!("{cycle_key}\0{attempt}").as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    let seconds = u64::from_le_bytes(bytes) % (u64::from(jitter_seconds) + 1);
    seconds.saturating_mul(1_000)
}

pub(super) fn is_safe_id(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::{AccountAuthState, AccountHealthState, AccountIdentity};
    use crate::quota::{QuotaSnapshot, QuotaWindow, Subscription};

    fn quota_window(
        kind: QuotaWindowKind,
        available_basis_points: Option<u16>,
        explicitly_full: Option<bool>,
        reset_at_ms: Option<u64>,
        observed_at_ms: u64,
    ) -> QuotaWindow {
        QuotaWindow {
            kind,
            available_basis_points,
            explicitly_full,
            reset_at_ms,
            window_minutes: Some(300),
            observed_at_ms,
            full_transition_fingerprint: Some("cycle-1".into()),
        }
    }

    fn account() -> AccountRecord {
        AccountRecord {
            id: "account-1".into(),
            label: "Account".into(),
            identity: AccountIdentity::from_hashed_parts(
                "openai",
                "example.test",
                "identity-hash",
                "secret-hash",
                "default",
                None,
            )
            .unwrap(),
            auth_mode: AccountAuthMode::OAuth,
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            source_id: "openai".into(),
            secret_refs: vec!["account:account-1".into()],
            subscription: Subscription::default(),
            quota: QuotaSnapshot {
                primary: Some(quota_window(
                    QuotaWindowKind::Primary,
                    Some(10_000),
                    Some(true),
                    Some(10_000),
                    100,
                )),
                ..QuotaSnapshot::default()
            },
            token_generation: 1,
            token_updated_at_ms: Some(1),
            tags: ["default".to_string()].into(),
            enabled: true,
            in_pool: true,
            draining: false,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
        }
    }

    fn task(max_attempts: u8, jitter_seconds: u32) -> WakeTask {
        WakeTask {
            id: "task-1".into(),
            name: "Primary wake".into(),
            enabled: true,
            account_selector: AccountSelector::AllEligible,
            window_kinds: [QuotaWindowKind::Primary].into(),
            model_policy: WakeModelPolicy::LightestSupported,
            trigger: WakeTrigger::QuotaFull,
            fallback_schedule: None,
            execution_policy: WakeExecutionPolicy::Automatic,
            jitter_seconds,
            max_attempts_per_cycle: max_attempts,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn transition() -> QuotaTransition {
        transition_with_fingerprint("cycle-1")
    }

    fn transition_with_fingerprint(fingerprint: &str) -> QuotaTransition {
        QuotaTransition {
            window_kind: QuotaWindowKind::Primary,
            fingerprint: fingerprint.into(),
            transitioned_at_ms: 100,
        }
    }

    fn policy() -> WakeAdapterPolicy {
        WakeAdapterPolicy {
            windows_requiring_activity: [QuotaWindowKind::Primary].into(),
            models: vec![
                WakeModel {
                    id: "large".into(),
                    lightness_rank: 10,
                    wake_capable: true,
                },
                WakeModel {
                    id: "small".into(),
                    lightness_rank: 1,
                    wake_capable: true,
                },
            ],
            verification_delay_ms: 1_000,
            output_token_cap: 8,
        }
    }

    fn schedule(coordinator: &mut WakeCoordinator, task: &WakeTask, now_ms: u64) -> WakeSchedule {
        schedule_for(coordinator, task, &account(), &transition(), now_ms)
    }

    fn schedule_for(
        coordinator: &mut WakeCoordinator,
        task: &WakeTask,
        account: &AccountRecord,
        transition: &QuotaTransition,
        now_ms: u64,
    ) -> WakeSchedule {
        match coordinator.evaluate(task, account, transition, None, &policy(), now_ms) {
            WakeDecision::Scheduled(schedule) => schedule,
            decision => panic!("unexpected decision: {decision:?}"),
        }
    }

    fn completion(outcome: WakeCompletionOutcome, completed_at_ms: u64) -> WakeCompletion {
        WakeCompletion {
            outcome,
            completed_at_ms,
            latency_ms: Some(10),
            input_tokens: Some(1),
            output_tokens: Some(1),
            error_code: None,
        }
    }

    #[test]
    fn retry_cap_allows_only_one_explicit_retry() {
        let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
        let schedule = schedule(&mut coordinator, &task(2, 0), 110);
        let first = coordinator.claim_due(schedule.due_at_ms, 1).pop().unwrap();
        assert_eq!(first.attempt, 1);
        assert!(coordinator.complete(first, completion(WakeCompletionOutcome::Unconfirmed, 120)));

        let retry = coordinator.pending().pop().unwrap();
        let second = coordinator.claim_due(retry.due_at_ms, 1).pop().unwrap();
        assert_eq!(second.attempt, 2);
        assert!(coordinator.complete(second, completion(WakeCompletionOutcome::Failed, 130)));
        assert!(coordinator.pending().is_empty());
        assert_eq!(
            coordinator.evaluate(&task(2, 0), &account(), &transition(), None, &policy(), 140),
            WakeDecision::Skipped(WakeOutcome::SkippedDuplicate)
        );
        assert_eq!(
            coordinator
                .state()
                .history()
                .iter()
                .map(|entry| (entry.attempt, entry.outcome))
                .collect::<Vec<_>>(),
            vec![(1, WakeOutcome::Unconfirmed), (2, WakeOutcome::Failed)]
        );
    }

    #[test]
    fn confirmed_attempt_permanently_completes_cycle() {
        let task = task(2, 0);
        let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
        let schedule = schedule(&mut coordinator, &task, 110);
        let permit = coordinator.claim_due(schedule.due_at_ms, 1).pop().unwrap();
        assert!(coordinator.complete(permit, completion(WakeCompletionOutcome::Confirmed, 120)));
        assert!(coordinator.pending().is_empty());
        assert_eq!(
            coordinator.evaluate(&task, &account(), &transition(), None, &policy(), 130),
            WakeDecision::Skipped(WakeOutcome::SkippedDuplicate)
        );
        assert_eq!(
            coordinator.state().history().back().unwrap().outcome,
            WakeOutcome::Confirmed
        );
    }

    #[test]
    fn pending_and_retry_due_times_survive_restart() {
        let task = task(2, 30);
        let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
        let schedule = schedule(&mut coordinator, &task, 1_000);
        let serialized = serde_json::to_string(coordinator.state()).unwrap();
        let state: WakeAutomationState = serde_json::from_str(&serialized).unwrap();
        let mut coordinator = WakeCoordinator::from_state(state).unwrap();
        assert_eq!(coordinator.pending(), vec![schedule.clone()]);
        assert!(coordinator
            .claim_due(schedule.due_at_ms.saturating_sub(1), 1)
            .is_empty());

        let first = coordinator.claim_due(schedule.due_at_ms, 1).pop().unwrap();
        assert!(coordinator.complete(
            first,
            completion(WakeCompletionOutcome::Unconfirmed, schedule.due_at_ms + 10)
        ));
        let retry = coordinator.pending().pop().unwrap();
        let serialized = serde_json::to_string(coordinator.state()).unwrap();
        let state: WakeAutomationState = serde_json::from_str(&serialized).unwrap();
        let mut coordinator = WakeCoordinator::from_state(state).unwrap();
        assert_eq!(coordinator.pending(), vec![retry.clone()]);
        assert_eq!(coordinator.claim_due(retry.due_at_ms, 1)[0].attempt, 2);
    }

    #[test]
    fn jitter_is_deterministic_and_bounded() {
        let task = task(1, 30);
        let mut first = WakeCoordinator::new(8, 8).unwrap();
        let mut second = WakeCoordinator::new(8, 8).unwrap();
        let first = schedule(&mut first, &task, 5_000);
        let second = schedule(&mut second, &task, 5_000);
        assert_eq!(first.due_at_ms, second.due_at_ms);
        assert!((5_000..=35_000).contains(&first.due_at_ms));
    }

    #[test]
    fn verification_confirms_only_consumption_or_advanced_countdown() {
        let before = quota_window(
            QuotaWindowKind::Primary,
            Some(10_000),
            Some(true),
            Some(10_000),
            100,
        );
        let consumed = quota_window(
            QuotaWindowKind::Primary,
            Some(9_000),
            Some(false),
            Some(10_000),
            200,
        );
        assert_eq!(
            verify_wake_countdown(Some(&before), Some(&consumed)),
            WakeVerificationOutcome::ConfirmedQuotaConsumed
        );
        let advanced = quota_window(
            QuotaWindowKind::Primary,
            Some(10_000),
            Some(true),
            Some(20_000),
            200,
        );
        assert_eq!(
            verify_wake_countdown(Some(&before), Some(&advanced)),
            WakeVerificationOutcome::ConfirmedCountdownAdvanced
        );
        let unchanged = quota_window(
            QuotaWindowKind::Primary,
            Some(10_000),
            Some(true),
            Some(10_000),
            200,
        );
        assert_eq!(
            verify_wake_countdown(Some(&before), Some(&unchanged)),
            WakeVerificationOutcome::Unconfirmed
        );
        let unknown = quota_window(QuotaWindowKind::Primary, None, None, Some(20_000), 200);
        assert_eq!(
            verify_wake_countdown(Some(&before), Some(&unknown)),
            WakeVerificationOutcome::Unconfirmed
        );
        assert_eq!(
            verify_wake_countdown(None, Some(&consumed)),
            WakeVerificationOutcome::Unconfirmed
        );
        let wrong_kind = quota_window(
            QuotaWindowKind::Secondary,
            Some(9_000),
            Some(false),
            Some(20_000),
            200,
        );
        assert_eq!(
            verify_wake_countdown(Some(&before), Some(&wrong_kind)),
            WakeVerificationOutcome::Unconfirmed
        );
    }

    #[test]
    fn natural_use_permanently_completes_pending_cycle() {
        let task = task(2, 30);
        let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
        schedule(&mut coordinator, &task, 110);
        assert_eq!(
            coordinator.evaluate(&task, &account(), &transition(), Some(100), &policy(), 120,),
            WakeDecision::Skipped(WakeOutcome::SkippedAlreadyStarted)
        );
        assert!(coordinator.pending().is_empty());
        assert!(coordinator.claim_due(u64::MAX, 1).is_empty());
        assert_eq!(
            coordinator.evaluate(&task, &account(), &transition(), None, &policy(), 130),
            WakeDecision::Skipped(WakeOutcome::SkippedDuplicate)
        );
        assert_eq!(
            coordinator.state().history().back().unwrap().outcome,
            WakeOutcome::SkippedAlreadyStarted
        );
    }

    #[test]
    fn automatic_and_confirmation_claims_do_not_steal_each_other() {
        let automatic = task(1, 0);
        let mut confirmation = task(1, 0);
        confirmation.id = "task-confirm".into();
        confirmation.execution_policy = WakeExecutionPolicy::RequireConfirmation;
        let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
        schedule(&mut coordinator, &automatic, 110);
        schedule_for(
            &mut coordinator,
            &confirmation,
            &account(),
            &transition_with_fingerprint("cycle-2"),
            110,
        );

        assert_eq!(coordinator.next_automatic_due(), Some(110));
        let confirmations = coordinator.claim_due_confirmations(110, 8);
        assert_eq!(confirmations.len(), 1);
        assert_eq!(confirmations[0].task_id, confirmation.id);
        assert_eq!(coordinator.next_automatic_due(), Some(110));
        let automatic = coordinator.claim_due_automatic(110, 8);
        assert_eq!(automatic.len(), 1);
        assert_eq!(automatic[0].task_id, "task-1");
        assert_eq!(coordinator.next_automatic_due(), None);
        assert!(coordinator.pending().is_empty());
    }

    #[test]
    fn only_primary_recovery_can_schedule_or_survive_restart() {
        let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
        schedule(&mut coordinator, &task(1, 0), 110);
        coordinator.state.cycles[0].window_kind = QuotaWindowKind::Secondary;

        let restored = WakeCoordinator::from_state(coordinator.into_state()).unwrap();
        assert!(restored.pending().is_empty());
        assert_eq!(restored.state().history().len(), 1);
        assert_eq!(
            restored.state().history()[0].error_code.as_deref(),
            Some("wake_window_redundant")
        );

        let mut secondary = transition();
        secondary.window_kind = QuotaWindowKind::Secondary;
        let mut policy = policy();
        policy
            .windows_requiring_activity
            .insert(QuotaWindowKind::Secondary);
        let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
        assert_eq!(
            coordinator.evaluate(&task(1, 0), &account(), &secondary, None, &policy, 110),
            WakeDecision::Skipped(WakeOutcome::SkippedIneligible)
        );
        assert!(coordinator.pending().is_empty());
    }

    #[test]
    fn tasks_share_one_global_account_window_cycle() {
        let first = task(1, 0);
        let mut second = task(1, 0);
        second.id = "task-2".into();
        let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
        schedule(&mut coordinator, &first, 110);

        assert_eq!(
            coordinator.evaluate(&second, &account(), &transition(), None, &policy(), 110),
            WakeDecision::Skipped(WakeOutcome::SkippedDuplicate)
        );
        assert_eq!(coordinator.pending().len(), 1);
        assert_eq!(coordinator.claim_due_automatic(110, 8)[0].task_id, first.id);
    }

    #[test]
    fn task_and_account_cancellation_finish_pending_and_in_flight_cycles() {
        let first = task(1, 0);
        let mut second = task(1, 0);
        second.id = "task-2".into();
        let mut other_account = account();
        other_account.id = "account-2".into();
        let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
        schedule(&mut coordinator, &first, 110);
        schedule_for(
            &mut coordinator,
            &second,
            &account(),
            &transition_with_fingerprint("cycle-2"),
            110,
        );
        schedule_for(
            &mut coordinator,
            &second,
            &other_account,
            &transition(),
            110,
        );

        let first_permit = coordinator.claim_due_automatic(110, 1).pop().unwrap();
        assert!(coordinator.is_permit_active(&first_permit));
        assert_eq!(coordinator.remove_pending_for_task(&first.id, 120), 1);
        assert!(!coordinator.is_permit_active(&first_permit));
        assert!(!coordinator.complete(
            first_permit,
            completion(WakeCompletionOutcome::Unconfirmed, 121)
        ));
        assert_eq!(coordinator.remove_pending_for_account("account-1", 122), 1);
        assert_eq!(coordinator.pending().len(), 1);
        let permit = coordinator.claim_due_automatic(110, 1).pop().unwrap();
        assert_eq!(permit.account_id, other_account.id);
        assert_eq!(coordinator.remove_pending_for_account("account-2", 123), 1);
        assert!(!coordinator.is_permit_active(&permit));
        assert_eq!(coordinator.state().history().len(), 3);
        assert!(coordinator.state().history().iter().all(|entry| {
            entry.outcome == WakeOutcome::SkippedIneligible
                && entry.model_id.is_none()
                && entry.input_tokens.is_none()
                && entry.output_tokens.is_none()
                && entry
                    .error_code
                    .as_deref()
                    .is_some_and(|code| code.starts_with("wake_"))
        }));
    }

    #[test]
    fn natural_use_completes_pending_and_in_flight_cycles_with_redacted_history() {
        let first = task(1, 0);
        let mut second = task(1, 0);
        second.id = "task-2".into();
        let mut other_account = account();
        other_account.id = "account-2".into();
        let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
        schedule(&mut coordinator, &first, 110);
        schedule_for(
            &mut coordinator,
            &second,
            &account(),
            &transition_with_fingerprint("cycle-2"),
            110,
        );
        schedule_for(&mut coordinator, &first, &other_account, &transition(), 110);
        let in_flight = coordinator.claim_due_automatic(110, 1).pop().unwrap();
        assert!(coordinator.is_permit_active(&in_flight));

        assert_eq!(
            coordinator.mark_natural_use_for_account("account-1", 120),
            2
        );
        assert!(!coordinator.is_permit_active(&in_flight));
        assert!(!coordinator.complete(
            in_flight,
            completion(WakeCompletionOutcome::Unconfirmed, 121)
        ));
        assert_eq!(coordinator.pending().len(), 1);
        assert_eq!(coordinator.pending()[0].request.account_id, "account-2");
        let history = coordinator.state().history();
        assert_eq!(history.len(), 2);
        assert!(history.iter().all(|entry| {
            entry.account_id == "account-1"
                && entry.outcome == WakeOutcome::SkippedAlreadyStarted
                && entry.model_id.is_none()
                && entry.latency_ms.is_none()
                && entry.input_tokens.is_none()
                && entry.output_tokens.is_none()
                && entry.error_code.is_none()
        }));
        let serialized = serde_json::to_string(history).unwrap();
        for secret in ["small", "Bearer", "prompt", "response body"] {
            assert!(!serialized.contains(secret));
        }
    }

    #[test]
    fn state_and_history_never_serialize_request_or_response_content() {
        let mut coordinator = WakeCoordinator::new(8, 8).unwrap();
        let schedule = schedule(&mut coordinator, &task(1, 0), 110);
        let scheduled = serde_json::to_string(coordinator.state()).unwrap();
        assert!(scheduled.contains("outputTokenCap"));
        assert!(!scheduled.contains("prompt"));
        assert!(!scheduled.contains("response body"));
        let permit = coordinator.claim_due(schedule.due_at_ms, 1).pop().unwrap();
        assert!(coordinator.complete(
            permit,
            WakeCompletion {
                outcome: WakeCompletionOutcome::Failed,
                completed_at_ms: 120,
                latency_ms: None,
                input_tokens: None,
                output_tokens: None,
                error_code: Some("Bearer secret prompt response body".into()),
            }
        ));
        let serialized = serde_json::to_string(coordinator.state()).unwrap();
        assert!(!serialized.contains("Bearer"));
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("prompt"));
        assert!(!serialized.contains("response body"));
        assert!(serialized.contains("redacted"));
    }

    #[test]
    fn unsupported_schedules_and_attempt_limits_are_rejected() {
        let mut invalid = task(0, 0);
        assert_eq!(
            invalid.validate(),
            Err(WakeTaskValidationError::InvalidAttemptLimit)
        );
        invalid.max_attempts_per_cycle = MAX_WAKE_ATTEMPTS + 1;
        assert_eq!(
            invalid.validate(),
            Err(WakeTaskValidationError::InvalidAttemptLimit)
        );
        invalid.max_attempts_per_cycle = 1;
        invalid.trigger = WakeTrigger::Daily;
        assert_eq!(
            invalid.validate(),
            Err(WakeTaskValidationError::UnsupportedSchedule)
        );
        invalid.trigger = WakeTrigger::QuotaFull;
        invalid.fallback_schedule = Some(WakeTrigger::Interval(60));
        assert_eq!(
            invalid.validate(),
            Err(WakeTaskValidationError::UnsupportedSchedule)
        );
    }

    #[test]
    fn lightest_model_rank_prefers_nano_then_mini() {
        assert!(model_lightness_rank("gpt-nano", 9) < model_lightness_rank("gpt-mini", 1));
        assert!(model_lightness_rank("gpt-mini", 9) < model_lightness_rank("gpt-large", 0));
    }
}
