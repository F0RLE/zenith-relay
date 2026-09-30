use super::super::policy::select_model;
use super::super::{
    WakeAdapterPolicy, WakeCoordinator, WakeCycle, WakeCycleStatus, WakeDecision,
    WakeExecutionPolicy, WakeExecutionRequest, WakeOutcome, WakePermit, WakeTask,
    WakeTaskValidationError, WakeTrigger, WakeVerificationMetadata,
};
use super::history::{cycle_key, deterministic_jitter_ms, same_cycle};
use crate::accounts::{AccountAuthMode, AccountRecord};
use crate::quota::{QuotaTransition, QuotaWindowKind};

impl WakeCoordinator {
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
            || task.trigger != WakeTrigger::QuotaFull
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
