use super::{invalid_core_state, now_ms, DesktopState};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::AutomationRecords,
};
use std::sync::MutexGuard;
use zenith_relay_core::{
    accounts::AccountRecord,
    automations::{
        WakeAdapterPolicy, WakeCompletion, WakeCoordinator, WakeDecision, WakeOutcome, WakePermit,
        WakeTask,
    },
    quota::{QuotaRefreshPermit, QuotaRefreshQueue, QuotaTransition},
};

impl DesktopState {
    pub(crate) fn mark_quota_refresh(&self, account_id: &str, due_at_ms: u64) -> Result<bool> {
        let changed = self
            .quota_refresh_queue()?
            .mark_dirty(account_id, due_at_ms)
            .map_err(invalid_core_state)?;
        if changed {
            self.quota_refresh_notify.notify_one();
        }
        Ok(changed)
    }

    pub(crate) fn restore_quota_refresh(&self, previous: QuotaRefreshQueue) -> Result<()> {
        *self.quota_refresh_queue()? = previous;
        self.quota_refresh_notify.notify_one();
        Ok(())
    }

    pub(crate) fn quota_refresh_snapshot(&self) -> Result<QuotaRefreshQueue> {
        Ok(self.quota_refresh_queue()?.clone())
    }

    pub(crate) fn remove_quota_refresh(&self, account_id: &str) -> Result<bool> {
        let removed = self.quota_refresh_queue()?.remove(account_id);
        if removed {
            self.quota_refresh_notify.notify_one();
        }
        Ok(removed)
    }

    pub(crate) fn sync_account_quota_refresh(
        &self,
        account_id: &str,
        due_at_ms: u64,
    ) -> Result<bool> {
        let monitored_account = {
            let store = self.store()?;
            store.account(account_id).is_some_and(|account| {
                account.remote_location.is_none()
                    && account.account.is_automatic_quota_monitoring_eligible()
            })
        };
        if monitored_account {
            self.mark_quota_refresh(account_id, due_at_ms)
        } else {
            self.remove_quota_refresh(account_id)
        }
    }

    pub(crate) fn quota_refresh_in_flight(&self, account_id: &str) -> Result<bool> {
        Ok(self.quota_refresh_queue()?.is_in_flight(account_id))
    }

    pub(crate) fn claim_due_quota_refreshes(
        &self,
        now_ms: u64,
        max_claims: usize,
    ) -> Result<Vec<QuotaRefreshPermit>> {
        Ok(self.quota_refresh_queue()?.claim_due(now_ms, max_claims))
    }

    pub(crate) fn reschedule_quota_refresh(
        &self,
        permit: QuotaRefreshPermit,
        due_at_ms: u64,
    ) -> Result<bool> {
        let rescheduled = self.quota_refresh_queue()?.reschedule(permit, due_at_ms);
        if rescheduled {
            self.quota_refresh_notify.notify_one();
        }
        Ok(rescheduled)
    }

    pub(crate) fn complete_quota_refresh(&self, permit: QuotaRefreshPermit) -> Result<bool> {
        let completed = self.quota_refresh_queue()?.complete(permit);
        if completed {
            self.quota_refresh_notify.notify_one();
        }
        Ok(completed)
    }

    pub(crate) fn next_quota_refresh_due(&self) -> Result<Option<u64>> {
        Ok(self.quota_refresh_queue()?.next_due())
    }

    pub(crate) async fn wait_for_quota_refresh(&self) {
        self.quota_refresh_notify.notified().await;
    }

    pub(crate) fn evaluate_wake_transition(
        &self,
        task: &WakeTask,
        account: &AccountRecord,
        transition: &QuotaTransition,
        policy: &WakeAdapterPolicy,
        now_ms: u64,
    ) -> Result<WakeDecision> {
        let decision = self.update_wake(|coordinator| {
            coordinator.evaluate(
                task,
                account,
                transition,
                account.last_used_at_ms,
                policy,
                now_ms,
            )
        })?;
        let notify = matches!(
            decision,
            WakeDecision::Scheduled(_) | WakeDecision::Skipped(WakeOutcome::SkippedAlreadyStarted)
        );
        if notify {
            self.wake_notify.notify_one();
        }
        Ok(decision)
    }

    pub(crate) fn claim_due_automatic_wakes(
        &self,
        now_ms: u64,
        max_claims: usize,
    ) -> Result<Vec<WakePermit>> {
        self.update_wake(|coordinator| coordinator.claim_due_automatic(now_ms, max_claims))
    }

    pub(crate) fn claim_due_confirmation_wakes(
        &self,
        now_ms: u64,
        max_claims: usize,
    ) -> Result<Vec<WakePermit>> {
        self.update_wake(|coordinator| coordinator.claim_due_confirmations(now_ms, max_claims))
    }

    pub(crate) fn complete_wake(
        &self,
        permit: WakePermit,
        completion: WakeCompletion,
    ) -> Result<bool> {
        let completed = self.update_wake(|coordinator| coordinator.complete(permit, completion))?;
        if completed {
            self.wake_notify.notify_one();
        }
        Ok(completed)
    }

    pub(crate) fn is_wake_permit_active(&self, permit: &WakePermit) -> Result<bool> {
        Ok(self.wake_coordinator_lock()?.is_permit_active(permit))
    }

    pub(crate) fn remove_pending_wakes_for_task(&self, task_id: &str) -> Result<usize> {
        self.remove_pending_wakes(|coordinator| {
            coordinator.remove_pending_for_task(task_id, now_ms())
        })
    }

    pub(crate) fn remove_pending_wakes_for_account(&self, account_id: &str) -> Result<usize> {
        self.remove_pending_wakes(|coordinator| {
            coordinator.remove_pending_for_account(account_id, now_ms())
        })
    }

    pub(crate) fn wake_snapshot(&self) -> Result<WakeCoordinator> {
        Ok(self.wake_coordinator_lock()?.clone())
    }

    pub(crate) fn restore_wake(
        &self,
        previous: WakeCoordinator,
        mut automations: AutomationRecords,
    ) -> Result<()> {
        let mut store = self.store()?;
        let mut coordinator = self.wake_coordinator_lock()?;
        automations.state = previous.state().clone();
        store.replace_automations(automations)?;
        *coordinator = previous;
        drop(coordinator);
        drop(store);
        self.wake_notify.notify_one();
        Ok(())
    }

    pub(crate) fn next_automatic_wake_due(&self) -> Result<Option<u64>> {
        Ok(self.wake_coordinator_lock()?.next_automatic_due())
    }

    pub(crate) async fn wait_for_wake(&self) {
        self.wake_notify.notified().await;
    }

    fn quota_refresh_queue(&self) -> Result<MutexGuard<'_, QuotaRefreshQueue>> {
        self.quota_refresh
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "quota refresh queue lock poisoned"))
    }

    fn remove_pending_wakes(
        &self,
        remove: impl FnOnce(&mut WakeCoordinator) -> usize,
    ) -> Result<usize> {
        let removed = self.update_wake(remove)?;
        if removed > 0 {
            self.wake_notify.notify_one();
        }
        Ok(removed)
    }

    fn update_wake<T>(&self, update: impl FnOnce(&mut WakeCoordinator) -> T) -> Result<T> {
        let mut store = self.store()?;
        let mut coordinator = self.wake_coordinator_lock()?;
        let mut next = coordinator.clone();
        let output = update(&mut next);
        let mut automations = store.automations().clone();
        automations.state = next.state().clone();
        store.replace_automations(automations)?;
        *coordinator = next;
        Ok(output)
    }

    fn wake_coordinator_lock(&self) -> Result<MutexGuard<'_, WakeCoordinator>> {
        self.wake
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "wake coordinator lock poisoned"))
    }
}

pub(super) fn wake_coordinator(automations: &AutomationRecords) -> Result<WakeCoordinator> {
    WakeCoordinator::from_state(automations.state.clone()).map_err(invalid_core_state)
}
