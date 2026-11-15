use super::model_failure_code;
use crate::local_pool::accounts::quota_refresh::AccountQuotaOutcome;
use crate::local_pool::models::LocalAccountRecord;
use zenith_relay_core::accounts::{
    accept_discovered_models,
    apply_model_discovery_failure as apply_account_model_discovery_failure,
};
use zenith_relay_core::providers::chatgpt::{ModelDiscoveryFailure, QuotaRefreshOutcome};
use zenith_relay_core::quota::QuotaTransition;

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
    zenith_relay_core::providers::chatgpt::connector::observations::apply_quota_outcome(
        &mut account.account,
        outcome,
        now_ms,
    )
}

pub(in crate::local_pool::accounts) fn apply_model_discovery(
    account: &mut LocalAccountRecord,
    discovery_result: std::result::Result<Vec<String>, ModelDiscoveryFailure>,
) -> bool {
    let previous_models = account.effective_models().to_vec();
    match discovery_result {
        Ok(models) => {
            let account_state = &mut account.account;
            if accept_discovered_models(
                &mut account.models,
                &mut account.discovered_models,
                &mut account_state.auth_state,
                &mut account_state.health,
                &mut account_state.last_error_code,
                crate::local_pool::models::normalized_values(models),
            ) {
                account.normalize();
            }
        }
        Err(error) => {
            // Keep the last good catalog for routing, but retain the model
            // discovery failure so the management UI can show stale model
            // availability instead of silently looking healthy.
            apply_model_discovery_failure(account, model_failure_code(&error), error.retryable)
        }
    }
    account.effective_models() != previous_models
}

pub(in crate::local_pool::accounts) fn apply_model_discovery_failure(
    account: &mut LocalAccountRecord,
    code: &str,
    retryable: bool,
) {
    let account_state = &mut account.account;
    apply_account_model_discovery_failure(
        &mut account_state.auth_state,
        &mut account_state.health,
        &mut account_state.last_error_code,
        code,
        retryable,
    );
}
