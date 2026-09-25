use crate::{
    app::{account_proxy_config, prepare_server_account_authorization},
    state::{AppState, ServerAccountRecord},
};
use std::{sync::Arc, time::Duration};
#[cfg(test)]
use zenith_relay_core::accounts::AccountAuthState;

use zenith_relay_core::{
    accounts::{
        apply_model_discovery_failure as apply_account_model_discovery_failure,
        recover_model_discovery_state,
    },
    error_codes,
    providers::chatgpt::{
        configured_codex_client_version, CodexModelsClient, ModelDiscoveryFailure,
        ModelDiscoveryFailureCode,
    },
    scheduler::account_candidate_health,
};

type ModelReadFailure = (String, bool, Option<u64>);
type ModelReadResult = Result<Vec<String>, ModelReadFailure>;

pub(super) async fn read_models(
    state: &Arc<AppState>,
    fence: &crate::store::AccountRefreshFence,
) -> Result<super::refresh::AccountRead, String> {
    let (checked, current) = state.store.account_refresh_scope(&fence.account_id)?;
    if &current != fence {
        return Err("account changed during refresh".into());
    }
    let account = &checked;
    let mut rejected_tokens = None;
    let mut model_result =
        discover_account_models(state, account, fence, &mut rejected_tokens).await;
    let reauth_state =
        if model_discovery_was_unauthorized(&model_result) && rejected_tokens.is_some() {
            match state
                .recover_account_tokens_after_unauthorized(
                    account,
                    rejected_tokens.as_ref().expect("checked bearer tokens"),
                )
                .await
            {
                Ok(_) => {
                    model_result =
                        discover_account_models(state, account, fence, &mut rejected_tokens).await;
                    None
                }
                Err(_) => state
                    .token_authority
                    .auth_state(&account.id)
                    .await
                    .filter(|auth_state| auth_state.requires_fresh_login()),
            }
        } else {
            None
        };
    let succeeded = model_result.is_ok();
    let retry_after_ms = model_result.as_ref().err().and_then(|failure| failure.2);
    if let Some(delay) = retry_after_ms {
        state.refresh.respect_retry_after(
            &fence.identity(),
            zenith_relay_core::scheduler::refresh::RefreshKind::Models,
            delay,
        );
    }
    state.store.apply_account_refresh(fence, |account| {
        let previous_models = account.effective_models().to_vec();
        let previous_health = account_candidate_health(
            account.auth_state,
            account.health,
            account.subscription.status,
            account.last_error_code.as_deref(),
        );
        apply_discovered_models(account, model_result);
        if let Some(auth_state) = reauth_state {
            account.auth_state = auth_state;
        }
        Ok(super::refresh::AccountRead {
            account: account.clone(),
            transitions: Vec::new(),
            succeeded,
            retry_after_ms,
            models_changed: account.effective_models() != previous_models,
            health_changed: previous_health
                != account_candidate_health(
                    account.auth_state,
                    account.health,
                    account.subscription.status,
                    account.last_error_code.as_deref(),
                ),
        })
    })
}

fn apply_discovered_models(account: &mut ServerAccountRecord, result: ModelReadResult) {
    match result {
        Ok(models) => {
            let models = zenith_relay_core::normalize_model_ids(models);
            if account.models.is_empty() && !models.is_empty() {
                account.models = models.clone();
            }
            account.discovered_models = Some(models);
            recover_model_discovery_state(
                &mut account.auth_state,
                &mut account.health,
                &mut account.last_error_code,
            );
        }
        Err((code, retryable, _)) => {
            // Cached model slugs remain routable, but the failed refresh must
            // remain visible to management clients as stale availability.
            apply_account_model_discovery_failure(
                &mut account.auth_state,
                &mut account.health,
                &mut account.last_error_code,
                &code,
                retryable,
            )
        }
    }
}

fn model_discovery_was_unauthorized(result: &ModelReadResult) -> bool {
    matches!(result, Err((code, _, _)) if code == error_codes::MODELS_UNAUTHORIZED)
}

async fn discover_account_models(
    state: &Arc<AppState>,
    account: &ServerAccountRecord,
    fence: &crate::store::AccountRefreshFence,
    rejected_tokens: &mut Option<zenith_relay_core::accounts::TokenSet>,
) -> ModelReadResult {
    *rejected_tokens = None;
    let prepared = super::refresh::request_authorization(state, fence)
        .await
        .map_err(|failure| {
            use super::refresh::AuthorizationFailure;
            let (code, retryable) = match failure {
                AuthorizationFailure::SecretLoad => ("models_secret_load", true),
                AuthorizationFailure::SecretMissing => ("models_secret_missing", false),
                AuthorizationFailure::SecretInvalid => ("models_secret_invalid", false),
                AuthorizationFailure::Prepare | AuthorizationFailure::Stale => {
                    ("models_authorization_prepare", true)
                }
            };
            (code.to_string(), retryable, None)
        })?;
    let (mut credential, mut authorization) = (prepared.credential, prepared.header);
    *rejected_tokens = prepared.oauth_tokens;
    let proxy = account_proxy_config(state, account, &credential).map_err(|_| {
        (
            error_codes::MODELS_PROXY_UNAVAILABLE.to_string(),
            false,
            None,
        )
    })?;
    let client = CodexModelsClient::new_with_proxy_and_timeout_and_user_agent(
        proxy.as_ref(),
        Duration::from_secs(20),
        "Zenith Relay Server",
    )
    .map_err(|_| (error_codes::MODELS_CLIENT_INIT.to_string(), false, None))?
    .with_http_scope(super::refresh::account_http_scope(state, fence));
    let client_version = configured_codex_client_version();
    let mut result = client
        .discover_authorized(
            authorization,
            &credential.chatgpt_account_id,
            &client_version,
        )
        .await;
    if rejected_tokens.is_none()
        && credential.is_agent_identity()
        && matches!(
            result.as_ref(),
            Err(ModelDiscoveryFailure {
                code: ModelDiscoveryFailureCode::AgentTaskInvalid,
                ..
            })
        )
    {
        let expected_task_id = credential.agent_task_id.clone().unwrap_or_default();
        (credential, authorization, *rejected_tokens) = prepare_server_account_authorization(
            state,
            account,
            credential,
            Some(&expected_task_id),
        )
        .await
        .map_err(|_| ("models_authorization_prepare".to_string(), true, None))?;
        result = client
            .discover_authorized(
                authorization,
                &credential.chatgpt_account_id,
                &client_version,
            )
            .await;
    }
    result.map_err(model_discovery_error)
}

fn model_discovery_error(error: ModelDiscoveryFailure) -> ModelReadFailure {
    let code = match error.code {
        // The server retries agent task registration once. A second failed
        // attempt used to be handled as its 401 response category.
        ModelDiscoveryFailureCode::AgentTaskInvalid => error_codes::MODELS_UNAUTHORIZED,
        // The server categorizes client construction errors separately from
        // a malformed endpoint response.
        ModelDiscoveryFailureCode::InvalidEndpoint => error_codes::MODELS_CLIENT_INIT,
        code => code.management_code(),
    };
    (code.to_string(), error.retryable, error.retry_after_ms)
}

#[cfg(test)]
mod tests;
