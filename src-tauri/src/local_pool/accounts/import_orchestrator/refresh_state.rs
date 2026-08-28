use super::model_failure_code;
use crate::local_pool::accounts::quota_refresh::AccountQuotaOutcome;
use crate::local_pool::accounts::quota_service::{apply_quota_failure, apply_quota_success};
use crate::local_pool::models::LocalAccountRecord;
use zenith_relay_core::accounts::{AccountAuthState, AccountHealthState};
use zenith_relay_core::providers::chatgpt::{ModelDiscoveryFailure, QuotaRefreshOutcome};
use zenith_relay_core::quota::{QuotaRefreshFailure, QuotaTransition};

pub(in crate::local_pool::accounts) fn apply_quota_outcome(
    account: &mut LocalAccountRecord,
    outcome: QuotaRefreshOutcome,
    now_ms: u64,
) -> AccountQuotaOutcome {
    apply_quota_outcome_with_transitions(account, outcome, now_ms).0
}

pub(in crate::local_pool::accounts) fn apply_quota_outcome_with_transitions(
    account: &mut LocalAccountRecord,
    outcome: QuotaRefreshOutcome,
    now_ms: u64,
) -> (AccountQuotaOutcome, Vec<QuotaTransition>) {
    match outcome {
        QuotaRefreshOutcome::Updated(data) => match apply_quota_success(account, data) {
            Ok(applied) => (
                AccountQuotaOutcome::Updated {
                    transitions: applied.transitions,
                    exhaustion_transitions: applied.exhaustion_transitions.clone(),
                },
                applied.exhaustion_transitions,
            ),
            Err(_) => {
                let failure = QuotaRefreshFailure::new("quota_invalid_response", false);
                apply_quota_failure(account, &failure, now_ms);
                (
                    AccountQuotaOutcome::Failed {
                        code: failure.code,
                        retryable: failure.retryable,
                    },
                    Vec::new(),
                )
            }
        },
        QuotaRefreshOutcome::Failed {
            failure,
            subscription,
        } => {
            account.account.subscription = subscription;
            apply_quota_failure(account, &failure, now_ms);
            (
                AccountQuotaOutcome::Failed {
                    code: failure.code,
                    retryable: failure.retryable,
                },
                Vec::new(),
            )
        }
    }
}

pub(in crate::local_pool::accounts) fn apply_model_discovery(
    account: &mut LocalAccountRecord,
    result: std::result::Result<Vec<String>, ModelDiscoveryFailure>,
) -> bool {
    let previous_models = account.effective_models().to_vec();
    match result {
        Ok(models) if !models.is_empty() => {
            let models = crate::local_pool::models::normalized_values(models);
            if account.models.is_empty() {
                account.models = models.clone();
            }
            account.discovered_models = Some(models);
            account.normalize();
            let recovered = account
                .account
                .last_error_code
                .as_deref()
                .is_some_and(|code| code.starts_with("models_"));
            if recovered {
                account.account.last_error_code = None;
                if !matches!(
                    account.account.auth_state,
                    AccountAuthState::RequiresReauth(_)
                ) {
                    if account.account.auth_state == AccountAuthState::Error {
                        account.account.auth_state = AccountAuthState::Active;
                    }
                    account.account.health = AccountHealthState::Healthy;
                }
            }
        }
        Ok(_) if account.effective_models().is_empty() => {
            apply_model_discovery_failure(account, "models_empty", false)
        }
        Err(error) => {
            // Keep the last good catalog for routing, but retain the model
            // discovery failure so the management UI can show stale model
            // availability instead of silently looking healthy.
            apply_model_discovery_failure(account, model_failure_code(&error), error.retryable)
        }
        Ok(_) => {}
    }
    account.effective_models() != previous_models
}

pub(in crate::local_pool::accounts) fn apply_model_discovery_failure(
    account: &mut LocalAccountRecord,
    code: &str,
    retryable: bool,
) {
    account.account.last_error_code = Some(code.to_string());
    match code {
        "models_unauthorized" | "models_invalid_access_token" | "models_invalid_account_id" => {
            // Reauthentication is a terminal, user-actionable auth state. A
            // later model probe must not downgrade it to the generic Error
            // state while preserving the last good catalog.
            if !matches!(
                account.account.auth_state,
                AccountAuthState::RequiresReauth(_)
            ) {
                account.account.auth_state = AccountAuthState::Error;
            }
            account.account.health = AccountHealthState::Unhealthy;
        }
        "models_forbidden" => account.account.health = AccountHealthState::Blocked,
        _ if retryable => account.account.health = AccountHealthState::Degraded,
        _ => account.account.health = AccountHealthState::Unhealthy,
    }
}
