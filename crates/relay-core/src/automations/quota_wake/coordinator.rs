use super::{
    WakeAutomationState, WakeCoordinator, WakeCycle, WakeCycleStatus, WakeHistory, WakePermit,
    WakeSchedule,
};
use crate::error::safe_error_code;
use crate::quota::QuotaWindowKind;

mod claim;
mod completion;
mod history;

use history::{deterministic_jitter_ms, valid_cycle};

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
