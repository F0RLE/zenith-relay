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
    quota::{QuotaRefreshFailure, QuotaSnapshot, QuotaTransition, QuotaWindow, Subscription},
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
                if let Some((incoming, retryable)) = model_refresh_error_kind(error.code) {
                    let current = account.account.last_error_code.clone();
                    let code = prefer_model_refresh_error(current.as_deref(), incoming);
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
    // Passive headers move reset timers while a full /wham/usage read is in
    // flight. That clock shift is not a newer remainder, so it must not discard
    // the complete response. A changed remainder, limit, credit ledger, or
    // supplemental window still wins over the late read.
    QuotaSchedulingEvidence::from_snapshot(&account.account.quota)
        != QuotaSchedulingEvidence::from_snapshot(&scope.before.account.quota)
}

/// The quota fields that make an in-flight refresh stale. Reset timers are
/// intentionally absent.
#[derive(Clone, Debug, Eq, PartialEq)]
struct QuotaSchedulingEvidence {
    primary_remaining: Option<u16>,
    secondary_remaining: Option<u16>,
    limit_reached: bool,
    reset_credits_available: Option<u32>,
    available_credits_micro_units: Option<u64>,
    provider_credits_available: bool,
    provider_credits_unlimited: bool,
    direct_balance_micro_usd: Option<u64>,
    supplemental: Vec<(String, Option<u16>)>,
}

impl QuotaSchedulingEvidence {
    fn from_snapshot(quota: &QuotaSnapshot) -> Self {
        Self {
            primary_remaining: window_remaining(quota.primary.as_ref()),
            secondary_remaining: window_remaining(quota.secondary.as_ref()),
            limit_reached: quota.limit_reached,
            reset_credits_available: quota.reset_credits_available,
            available_credits_micro_units: quota.available_credits_micro_units,
            provider_credits_available: quota.provider_credits_available,
            provider_credits_unlimited: quota.provider_credits_unlimited,
            direct_balance_micro_usd: quota.direct_balance_micro_usd,
            supplemental: quota
                .supplemental
                .iter()
                .map(|window| (window.id.clone(), window.window.available_basis_points))
                .collect(),
        }
    }
}

fn window_remaining(window: Option<&QuotaWindow>) -> Option<u16> {
    window.and_then(|window| window.available_basis_points)
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

/// A local refresh failure is not a ChatGPT catalog response. It must not
/// replace a provider code such as `models_transport` or `models_unauthorized`.
pub(in crate::local_pool) fn prefer_model_refresh_error<'a>(
    current: Option<&'a str>,
    incoming: &'a str,
) -> &'a str {
    let current_is_provider =
        current.is_some_and(|code| code.starts_with("models_") && model_code_is_provider(code));
    let incoming_is_local = !model_code_is_provider(incoming);
    if current_is_provider && incoming_is_local {
        current.unwrap_or(incoming)
    } else {
        incoming
    }
}

fn model_code_is_provider(code: &str) -> bool {
    !matches!(
        code,
        error_codes::MODELS_PREPARE
            | error_codes::MODELS_SECRET_STORE
            | error_codes::MODELS_STORAGE
            | error_codes::MODELS_ACCOUNT_LOCATION
            | error_codes::MODELS_PROFILE_RESTORE
            | error_codes::MODELS_PROXY_UNAVAILABLE
            | error_codes::MODELS_CLIENT_INIT
    )
}

#[cfg(test)]
mod tests;
