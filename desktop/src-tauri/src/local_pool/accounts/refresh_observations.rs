//! Revision-checked application of desktop quota and model observations.
//! Provider I/O stays with the readers; this owner never saves their old record.

use super::{
    import_orchestrator::{
        apply_model_discovery, apply_model_discovery_failure, preserve_newer_account_state,
    },
    quota_refresh::AccountQuotaOutcome,
    quota_service::apply_quota_failure,
};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::LocalAccountRecord,
    runtime::{current_time_ms, sync_account_quota_references},
    state::DesktopState,
    store::{AccountRefreshFence, AppliedAccountRefresh, LocalPoolStore},
};
#[cfg(test)]
use zenith_relay_core::quota::Subscription;
use zenith_relay_core::{
    error_codes,
    providers::chatgpt::{ModelDiscoveryFailure, OAuthClientKind, QuotaRefreshOutcome},
    quota::{QuotaRefreshFailure, QuotaTransition},
};

pub(in crate::local_pool) struct AccountRefreshScope {
    pub initial_account: LocalAccountRecord,
    pub fence: AccountRefreshFence,
    pub started_at_ms: u64,
}

impl AccountRefreshScope {
    pub async fn capture(state: &DesktopState, account_id: &str) -> Result<Self> {
        // A login/proxy mutation changes both secret-backed and public state.
        // Never capture a scope halfway through that transaction or its rollback.
        let _mutation = state.setup_guard().await;
        let (initial_account, fence) = state.store()?.account_refresh_scope(account_id)?;
        Ok(Self {
            initial_account,
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
        let desktop_state = state.clone();
        let fence = self.fence.clone();
        zenith_relay_core::scheduler::refresh::http::ManagementHttpScope::checked(move || {
            desktop_state
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

pub(in crate::local_pool) use zenith_relay_core::providers::chatgpt::connector::observations::SubscriptionObservation;

pub(in crate::local_pool) fn apply_quota_read(
    store: &mut LocalPoolStore,
    scope: &AccountRefreshScope,
    quota: QuotaRefreshOutcome,
    subscription: impl Into<SubscriptionObservation>,
    models: Option<std::result::Result<Vec<String>, ModelDiscoveryFailure>>,
) -> Result<AppliedAccountRefresh<AppliedQuotaRead>> {
    store.apply_account_refresh(&scope.fence, |account| {
        let account_snapshot = account.clone();
        let (outcome, exhaustion_transitions) =
            zenith_relay_core::providers::chatgpt::connector::observations::apply_quota_read(
                &mut account.account,
                &scope.initial_account.account,
                quota,
                subscription,
                scope.started_at_ms,
            );
        let models_changed = models
            .map(|models| apply_model_discovery(account, models))
            .unwrap_or(false);
        preserve_newer_account_state(account, &scope.initial_account, &account_snapshot);
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
    client_kind: OAuthClientKind,
    models: std::result::Result<Vec<String>, ModelDiscoveryFailure>,
) -> Result<AppliedAccountRefresh<bool>> {
    store.apply_account_refresh(&scope.fence, |account| {
        let account_snapshot = account.clone();
        let changed = match models {
            Ok(models) if client_kind == OAuthClientKind::Siwc && models.is_empty() => {
                // The selected grant returned no visible models. Keep the saved
                // baseline, but do not publish it as current SIWC entitlement.
                let changed = !account.effective_models().is_empty();
                account.discovered_models = Some(models);
                let state = &mut account.account;
                zenith_relay_core::accounts::recover_model_discovery_state(
                    &mut state.auth_state,
                    &mut state.health,
                    &mut state.last_error_code,
                );
                changed
            }
            result => apply_model_discovery(account, result),
        };
        preserve_newer_account_state(account, &scope.initial_account, &account_snapshot);
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
    let runtime = state.gateway.runtime().await;
    let apply_result = (|| -> Result<()> {
        let mut store = state.store()?;
        if store.ensure_account_refresh_current(&scope.fence).is_err() {
            return Ok(());
        }
        apply_read_error(&mut store, scope, kind, error)?;
        if let Some(runtime) = &runtime {
            sync_account_quota_references(state, runtime, &store)?;
        }
        Ok(())
    })();
    if let Err(error) = apply_result {
        crate::diagnostics::record_error(
            "account-refresh",
            Some("apply_refresh_error_failed"),
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
        let account_snapshot = account.clone();
        match kind {
            RefreshReadKind::Quota => {
                if !quota_is_newer(account, scope)
                    && let Some(code) = quota_refresh_error_kind(error.code)
                {
                    apply_quota_failure(
                        account,
                        &QuotaRefreshFailure::new(code, true),
                        scope.started_at_ms,
                    );
                }
            }
            RefreshReadKind::Models => {
                if let Some((incoming, retryable)) = model_refresh_error_kind(error.code) {
                    let previous_error_code = account.account.last_error_code.clone();
                    let code = prefer_model_refresh_error(previous_error_code.as_deref(), incoming);
                    if !account.account.auth_state.requires_fresh_login()
                        || code != error_codes::MODELS_PREPARE
                    {
                        apply_model_discovery_failure(account, code, retryable);
                    }
                }
            }
        }
        preserve_newer_account_state(account, &scope.initial_account, &account_snapshot);
        Ok(())
    })?;
    Ok(())
}

fn quota_is_newer(account: &LocalAccountRecord, scope: &AccountRefreshScope) -> bool {
    zenith_relay_core::providers::chatgpt::connector::observations::quota_is_newer(
        &account.account.quota,
        &scope.initial_account.account.quota,
    )
}

fn quota_refresh_error_kind(code: ErrorCode) -> Option<&'static str> {
    Some(match code {
        ErrorCode::SecretStoreUnavailable => error_codes::QUOTA_SECRET_STORE,
        ErrorCode::GatewayUnavailable => error_codes::QUOTA_PROXY_UNAVAILABLE,
        ErrorCode::Conflict => error_codes::RECOVERY_REQUIRED,
        ErrorCode::Io | ErrorCode::RecoveryRequired => error_codes::QUOTA_STORAGE,
        ErrorCode::InvalidState
        | ErrorCode::SourceTestFailed
        | ErrorCode::ProfileRestoreBlocked
        | ErrorCode::UnsupportedSchema => error_codes::QUOTA_PREPARE,
        ErrorCode::NotFound | ErrorCode::PoolModelsUnavailable => return None,
    })
}

pub(in crate::local_pool) fn model_refresh_error_kind(
    code: ErrorCode,
) -> Option<(&'static str, bool)> {
    Some(match code {
        ErrorCode::SecretStoreUnavailable => (error_codes::MODELS_SECRET_STORE, true),
        ErrorCode::GatewayUnavailable => (error_codes::MODELS_PROXY_UNAVAILABLE, true),
        ErrorCode::Conflict => (error_codes::RECOVERY_REQUIRED, false),
        ErrorCode::Io | ErrorCode::RecoveryRequired => (error_codes::MODELS_STORAGE, true),
        ErrorCode::InvalidState | ErrorCode::SourceTestFailed | ErrorCode::UnsupportedSchema => {
            (error_codes::MODELS_PREPARE, true)
        }
        ErrorCode::ProfileRestoreBlocked => (error_codes::MODELS_PROFILE_RESTORE, false),
        ErrorCode::NotFound | ErrorCode::PoolModelsUnavailable => return None,
    })
}

/// A local refresh failure is not a ChatGPT catalog response. It must not
/// replace a provider code such as `models_transport` or `models_unauthorized`.
pub(in crate::local_pool) fn prefer_model_refresh_error<'a>(
    previous_error_code: Option<&'a str>,
    incoming: &'a str,
) -> &'a str {
    let current_is_provider = previous_error_code
        .is_some_and(|code| code.starts_with("models_") && model_code_is_provider(code));
    let incoming_is_local = !model_code_is_provider(incoming);
    if current_is_provider && incoming_is_local {
        previous_error_code.unwrap_or(incoming)
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
            | error_codes::RECOVERY_REQUIRED
            | error_codes::MODELS_PROFILE_RESTORE
            | error_codes::MODELS_PROXY_UNAVAILABLE
            | error_codes::MODELS_CLIENT_INIT
    )
}

#[cfg(test)]
mod tests;
