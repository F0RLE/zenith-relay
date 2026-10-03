//! Desktop adapter for the shared account and source refresh lifecycle. The store
//! supplies revision/eligibility events; provider readers only apply fenced
//! observations. No queue snapshots, per-caller settlement or polling scans.

mod automations;
pub(crate) mod sources;

mod account;
pub(crate) use account::request;
pub(super) use account::reset_due_delay;
pub(in crate::local_pool) use account::{request_authorization, request_authorization_now};

use super::{
    accounts::quota_refresh::{AccountQuotaRefreshResponse, PreparedAccountAuthorization},
    commands::current_time_ms,
    error::{ErrorCode, LocalPoolError, Result},
    models::LocalAccountRecord,
    state::DesktopState,
};
use std::{
    collections::BTreeSet,
    sync::{atomic::Ordering, Arc},
};
use zenith_relay_core::scheduler::{
    account_member_key,
    refresh::{service::RefreshWaitError, RefreshKind},
};

#[derive(Clone, Debug)]
pub(crate) enum RefreshRead {
    Quota(Box<AccountQuotaRefreshResponse>),
    Models {
        succeeded: bool,
    },
    /// Only shared with current waiters; credentials never enter observation cache.
    Authorization(Box<PreparedAccountAuthorization>),
    SourceModels(Box<super::models::ProviderSourceRecord>),
    SourceStats(zenith_relay_core::scheduler::refresh::SourceStatsObservation),
}

impl zenith_relay_core::scheduler::refresh::SourceStatsRead for RefreshRead {
    fn source_stats(
        &self,
    ) -> Option<&zenith_relay_core::scheduler::refresh::SourceStatsObservation> {
        match self {
            Self::SourceStats(observation) => Some(observation),
            _ => None,
        }
    }
}

pub(crate) type RefreshReadResult = Result<RefreshRead>;

pub(crate) fn cache_observation(value: &RefreshReadResult) -> bool {
    matches!(value, Ok(read) if !matches!(read, RefreshRead::Authorization(_)))
}

impl DesktopState {
    /// Opening storage alone must never start provider work (including tests
    /// and recovery tools). The native application explicitly starts this owner.
    pub(crate) fn start_refresh(&self) -> Result<()> {
        let mut changes = self.store()?.refresh_changes();
        if self.refresh_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let weak = Arc::downgrade(&self.owner);
        let event_owner = weak.clone();
        let mut progress = self.refresh.progress();
        tauri::async_runtime::spawn(async move {
            while progress.changed().await.is_ok() {
                let Some(owner) = event_owner.upgrade() else {
                    break;
                };
                owner.oauth_events.emit_state_changed();
            }
        });
        tauri::async_runtime::spawn(async move {
            loop {
                changes.borrow_and_update();
                let Some(owner) = weak.upgrade() else {
                    break;
                };
                let state = DesktopState { owner };
                if let Err(error) = account::reconcile(&state).await {
                    crate::diagnostics::record_error(
                        "refresh",
                        Some("reconcile_failed"),
                        &error.message,
                        &[],
                    );
                }
                // Never retain the host while idle, including via a callback.
                drop(state);
                if changes.changed().await.is_err() {
                    break;
                }
            }
        });
        Ok(())
    }

    /// A focused host event, not another queue. Newly eligible/revived accounts
    /// are registered by the store watcher after the setup transaction finishes.
    pub(crate) fn sync_account_quota_refresh(
        &self,
        account_id: &str,
        due_at_ms: u64,
    ) -> Result<bool> {
        let store = self.store()?;
        let Some(account) = store.account(account_id) else {
            return Ok(self.refresh.remove_member(&account_member_key(account_id)));
        };
        let eligible = automatic_eligible(account);
        if eligible {
            let (_, fence) = store.account_refresh_scope(account_id)?;
            self.refresh.schedule_after(
                &fence.identity(),
                RefreshKind::Quota,
                due_at_ms.saturating_sub(current_time_ms()),
            );
        }
        store.notify_refresh_changed();
        Ok(eligible)
    }

    pub(crate) fn remove_account_refresh(&self, account_id: &str) -> bool {
        self.refresh.remove_member(&account_member_key(account_id))
    }

    pub(crate) fn quota_refresh_in_flight(&self, account_id: &str) -> Result<bool> {
        let store = self.store()?;
        if store.account(account_id).is_none() {
            return Ok(false);
        }
        let (_, fence) = store.account_refresh_scope(account_id)?;
        Ok(self
            .refresh
            .in_flight(&fence.identity(), RefreshKind::Quota))
    }
}

fn automatic_eligible(account: &LocalAccountRecord) -> bool {
    account.remote_location.is_none() && account.account.is_automatic_quota_monitoring_eligible()
}

async fn active_members(state: &DesktopState) -> BTreeSet<String> {
    state
        .gateway
        .runtime()
        .await
        .map(|runtime| {
            runtime.active_member_keys(
                current_time_ms(),
                zenith_relay_core::scheduler::refresh::RECENT_ACTIVITY_WINDOW_MS,
            )
        })
        .unwrap_or_default()
}

fn recently_used(at: Option<u64>) -> bool {
    zenith_relay_core::scheduler::refresh::recently_active(at, current_time_ms())
}

fn wait_error(error: RefreshWaitError) -> LocalPoolError {
    let (code, message) = match error {
        RefreshWaitError::Stale => (ErrorCode::Conflict, "connection changed during refresh"),
        RefreshWaitError::Full => (ErrorCode::InvalidState, "refresh capacity is full"),
        RefreshWaitError::Stopped | RefreshWaitError::Interrupted => {
            (ErrorCode::InvalidState, "refresh was interrupted")
        }
    };
    LocalPoolError::new(code, message)
}

#[cfg(test)]
mod tests;
