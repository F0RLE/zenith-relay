//! Revision-checked application of desktop quota and model observations.
//! Provider I/O stays with the readers; this owner never saves their old record.

use super::{
    import_orchestrator::{
        apply_model_discovery, apply_model_discovery_failure, apply_quota_outcome_with_transitions,
        preserve_newer_account_state,
    },
    quota_refresh::AccountQuotaOutcome,
    quota_service::apply_quota_failure,
};
use crate::local_pool::{
    commands::current_time_ms,
    error::{ErrorCode, LocalPoolError, Result},
    models::LocalAccountRecord,
    state::DesktopState,
    store::{AccountRefreshFence, AppliedAccountRefresh, LocalPoolStore},
};
use zenith_relay_core::{
    error_codes,
    providers::chatgpt::{ModelDiscoveryFailure, QuotaRefreshOutcome},
    quota::{QuotaRefreshFailure, QuotaTransition, Subscription},
};

pub(in crate::local_pool) struct AccountRefreshScope {
    pub before: LocalAccountRecord,
    pub fence: AccountRefreshFence,
    pub started_at_ms: u64,
}

impl AccountRefreshScope {
    pub async fn capture(state: &DesktopState, account_id: &str) -> Result<Self> {
        // A login/proxy mutation changes both secret-backed and public state.
        // Never capture a scope halfway through that transaction or its rollback.
        let _mutation = state.setup_guard().await;
        let (before, fence) = state.store()?.account_refresh_scope(account_id)?;
        Ok(Self {
            before,
            fence,
            started_at_ms: current_time_ms(),
        })
    }

    pub fn validate(&self, state: &DesktopState) -> Result<()> {
        state.store()?.ensure_account_refresh_current(&self.fence)
    }

    pub fn http_scope(
        &self,
        state: &DesktopState,
    ) -> zenith_relay_core::scheduler::refresh::http::ManagementHttpScope {
        let state = state.clone();
        let fence = self.fence.clone();
        zenith_relay_core::scheduler::refresh::http::ManagementHttpScope::checked(move || {
            state
                .store()
                .is_ok_and(|store| store.ensure_account_refresh_current(&fence).is_ok())
        })
    }
}

pub(in crate::local_pool) struct AppliedQuotaRead {
    pub outcome: AccountQuotaOutcome,
    pub exhaustion_transitions: Vec<QuotaTransition>,
    pub models_changed: bool,
}

pub(in crate::local_pool) fn apply_quota_read(
    store: &mut LocalPoolStore,
    scope: &AccountRefreshScope,
    quota: QuotaRefreshOutcome,
    subscription: Subscription,
    models: Option<std::result::Result<Vec<String>, ModelDiscoveryFailure>>,
) -> Result<AppliedAccountRefresh<AppliedQuotaRead>> {
    store.apply_account_refresh(&scope.fence, |account| {
        let current = account.clone();
        let (outcome, exhaustion_transitions) = if quota_is_newer(account, scope) {
            // No transition may be emitted from superseded evidence, including
            // a late exhaustion/full result that could trigger a paid action.
            (AccountQuotaOutcome::Skipped, Vec::new())
        } else {
            let applied = apply_quota_outcome_with_transitions(account, quota, scope.started_at_ms);
            if current.account.subscription != scope.before.account.subscription {
                account.account.subscription = current.account.subscription.clone();
            } else if subscription != scope.before.account.subscription {
                account.account.subscription = subscription;
            }
            applied
        };
        let models_changed = models
            .map(|models| apply_model_discovery(account, models))
            .unwrap_or(false);
        preserve_newer_account_state(account, &scope.before, &current);
        Ok(AppliedQuotaRead {
            outcome,
            exhaustion_transitions,
            models_changed,
        })
    })
}

pub(in crate::local_pool) fn apply_models_read(
    store: &mut LocalPoolStore,
    scope: &AccountRefreshScope,
    models: std::result::Result<Vec<String>, ModelDiscoveryFailure>,
) -> Result<AppliedAccountRefresh<bool>> {
    store.apply_account_refresh(&scope.fence, |account| {
        let current = account.clone();
        let changed = apply_model_discovery(account, models);
        preserve_newer_account_state(account, &scope.before, &current);
        Ok(changed)
    })
}

#[derive(Clone, Copy)]
pub(in crate::local_pool) enum RefreshReadKind {
    Quota,
    Models,
}

/// A failed caller may no longer own this account. Error persistence has the
/// same fence as success and belongs to the read, never an outer UI/queue waiter.
pub(in crate::local_pool) async fn record_read_error(
    state: &DesktopState,
    scope: &AccountRefreshScope,
    kind: RefreshReadKind,
    error: &LocalPoolError,
) {
    let _mutation = state.setup_guard().await;
    let result = (|| -> Result<()> {
        let mut store = state.store()?;
        if store.ensure_account_refresh_current(&scope.fence).is_err() {
            return Ok(());
        }
        apply_read_error(&mut store, scope, kind, error)
    })();
    if let Err(error) = result {
        crate::diagnostics::record_error(
            "account-refresh",
            Some("persist_refresh_error_failed"),
            &error.message,
            &[],
        );
    }
}

pub(in crate::local_pool) fn apply_read_error(
    store: &mut LocalPoolStore,
    scope: &AccountRefreshScope,
    kind: RefreshReadKind,
    error: &LocalPoolError,
) -> Result<()> {
    store.apply_account_refresh(&scope.fence, |account| {
        let current = account.clone();
        match kind {
            RefreshReadKind::Quota => {
                if !quota_is_newer(account, scope) {
                    if let Some(code) = quota_refresh_error_kind(error.code) {
                        apply_quota_failure(
                            account,
                            &QuotaRefreshFailure::new(code, true),
                            scope.started_at_ms,
                        );
                    }
                }
            }
            RefreshReadKind::Models => {
                if let Some((code, retryable)) = model_refresh_error_kind(error.code) {
                    if !account.account.auth_state.requires_fresh_login()
                        || code != error_codes::MODELS_PREPARE
                    {
                        apply_model_discovery_failure(account, code, retryable);
                    }
                }
            }
        }
        preserve_newer_account_state(account, &scope.before, &current);
        Ok(())
    })?;
    Ok(())
}

fn quota_is_newer(account: &LocalAccountRecord, scope: &AccountRefreshScope) -> bool {
    let quota = &account.account.quota;
    let observed_at_ms = quota
        .updated_at_ms
        .into_iter()
        .chain(quota.error.as_ref().map(|error| error.occurred_at_ms))
        .max();
    // A host wall-clock correction must not make a concurrently persisted
    // observation look older than the read. The captured snapshot is also a
    // fence, not only its provider timestamp.
    quota != &scope.before.account.quota
        || observed_at_ms.is_some_and(|at| at > scope.started_at_ms)
}

fn quota_refresh_error_kind(code: ErrorCode) -> Option<&'static str> {
    Some(match code {
        ErrorCode::SecretStoreUnavailable => error_codes::QUOTA_SECRET_STORE,
        ErrorCode::GatewayUnavailable => error_codes::QUOTA_PROXY_UNAVAILABLE,
        ErrorCode::Conflict => error_codes::QUOTA_ACCOUNT_LOCATION,
        ErrorCode::Io | ErrorCode::RecoveryRequired => error_codes::QUOTA_STORAGE,
        ErrorCode::InvalidState
        | ErrorCode::SourceTestFailed
        | ErrorCode::ProfileRestoreBlocked
        | ErrorCode::UnsupportedSchema => error_codes::QUOTA_PREPARE,
        ErrorCode::NotFound | ErrorCode::SourceProbeStale => return None,
    })
}

pub(in crate::local_pool) fn model_refresh_error_kind(
    code: ErrorCode,
) -> Option<(&'static str, bool)> {
    Some(match code {
        ErrorCode::SecretStoreUnavailable => (error_codes::MODELS_SECRET_STORE, true),
        ErrorCode::GatewayUnavailable => (error_codes::MODELS_PROXY_UNAVAILABLE, true),
        ErrorCode::Conflict => (error_codes::MODELS_ACCOUNT_LOCATION, false),
        ErrorCode::Io | ErrorCode::RecoveryRequired => (error_codes::MODELS_STORAGE, true),
        ErrorCode::InvalidState | ErrorCode::SourceTestFailed | ErrorCode::UnsupportedSchema => {
            (error_codes::MODELS_PREPARE, true)
        }
        ErrorCode::ProfileRestoreBlocked => (error_codes::MODELS_PROFILE_RESTORE, false),
        ErrorCode::NotFound | ErrorCode::SourceProbeStale => return None,
    })
}

#[cfg(test)]
mod tests;
