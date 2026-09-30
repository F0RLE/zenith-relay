use super::super::{
    WakeCompletion, WakeCompletionOutcome, WakeCoordinator, WakeCycle, WakeCycleStatus,
    WakeDecision, WakeHistory, WakeOutcome, WakePermit, WakeTask,
};
use super::history::{canceled_history, deterministic_jitter_ms, natural_use_history, same_cycle};
use crate::accounts::AccountRecord;
use crate::error::safe_error_code;
use crate::quota::QuotaTransition;

impl WakeCoordinator {
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

    pub(super) fn cancel_matching(
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

    pub(super) fn complete_by_natural_use(
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
}
