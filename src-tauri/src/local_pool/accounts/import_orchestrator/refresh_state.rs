use super::model_failure_code;
use crate::local_pool::accounts::quota_refresh::AccountQuotaOutcome;
use crate::local_pool::accounts::quota_service::{apply_quota_failure, apply_quota_success};
use crate::local_pool::models::LocalAccountRecord;
use zenith_relay_core::accounts::{
    apply_model_discovery_failure as apply_account_model_discovery_failure,
    recover_model_discovery_state,
};
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
            let state = &mut account.account;
            recover_model_discovery_state(
                &mut state.auth_state,
                &mut state.health,
                &mut state.last_error_code,
            );
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
    let state = &mut account.account;
    apply_account_model_discovery_failure(
        &mut state.auth_state,
        &mut state.health,
        &mut state.last_error_code,
        code,
        retryable,
    );
}
