use crate::{
    app::{account_proxy_config, prepare_server_account_authorization},
    state::{now_ms, AppState, ServerAccountRecord},
};
use std::{sync::Arc, time::Duration};
use zenith_relay_core::{
    accounts::{reduce_account_quota, AccountAuthState, AccountQuotaOutcome},
    error_codes,
    providers::chatgpt::{
        bearer_authorization, is_agent_identity_task_invalid_failure, subscription_refresh_due,
        CodexQuotaClient,
    },
    quota::{QuotaRefreshFailure, QuotaRefreshResult},
    scheduler::account_candidate_health,
};

pub(super) async fn read_one(
    state: &Arc<AppState>,
    fence: &crate::store::AccountRefreshFence,
    force_subscription_refresh: bool,
) -> Result<super::refresh::AccountRead, String> {
    let account_id = &fence.account_id;
    let (checked, current) = state.store.account_refresh_scope(account_id)?;
    if &current != fence {
        return Err("account changed during refresh".into());
    }
    let observed_at_ms = now_ms();
    let result = refresh_data(state, &checked, fence, force_subscription_refresh).await;
    let retry_after_ms = result
        .as_ref()
        .err()
        .and_then(QuotaRefreshFailure::retry_after_ms);
    if let Some(delay) = retry_after_ms {
        state.refresh.respect_retry_after(
            &fence.identity(),
            zenith_relay_core::scheduler::refresh::RefreshKind::Quota,
            delay,
        );
    }
    let access_only_rejected = result.as_ref().err().is_some_and(|failure| {
        failure.http_status() == Some(401) || failure.code == error_codes::QUOTA_TOKEN_PREPARE
    });
    let auth_state = state.token_authority.auth_state(account_id).await;
    state.store.apply_account_refresh(fence, |account| {
        let previous_health = account_candidate_health(
            account.auth_state,
            account.health,
            account.subscription.status,
            account.last_error_code.as_deref(),
        );
        // A newer passive quota observation wins over this delayed read/failure.
        if account
            .quota
            .updated_at_ms
            .is_some_and(|at| at > observed_at_ms)
        {
            return Ok(super::refresh::AccountRead {
                account: account.clone(),
                transitions: Vec::new(),
                succeeded: true,
                retry_after_ms: None,
                models_changed: false,
                health_changed: false,
            });
        }
        let update = reduce_account_quota(
            &account.quota,
            &account.subscription,
            account.health,
            account.last_error_code.as_deref(),
            result,
            observed_at_ms,
        )
        .map_err(|error| error.to_string())?;
        let succeeded = matches!(update.outcome, AccountQuotaOutcome::Updated { .. });
        let transitions = match &update.outcome {
            AccountQuotaOutcome::Updated { transitions } => transitions.clone(),
            AccountQuotaOutcome::Failed { .. } => Vec::new(),
        };
        account.quota = update.quota;
        account.subscription = update.subscription;
        account.health = update.health;
        account.last_error_code = update.last_error_code;
        // Credential authority already persists its state. Never replace a
        // concurrently changed login/auth state with a pre-HTTP snapshot.
        if account.auth_state == checked.auth_state {
            if let Some(auth_state) = auth_state {
                account.auth_state = auth_state;
            }
            if access_only_rejected && account.auth_state == AccountAuthState::DegradedAccessOnly {
                account.auth_state = AccountAuthState::RequiresReauth(
                    zenith_relay_core::accounts::ReauthReason::AccessTokenExpired,
                );
            }
        }
        Ok(super::refresh::AccountRead {
            account: account.clone(),
            transitions,
            succeeded,
            retry_after_ms,
            models_changed: false,
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

async fn refresh_data(
    state: &Arc<AppState>,
    account: &ServerAccountRecord,
    fence: &crate::store::AccountRefreshFence,
    force_subscription_refresh: bool,
) -> Result<QuotaRefreshResult, QuotaRefreshFailure> {
    let prepared = super::refresh::request_authorization(state, fence)
        .await
        .map_err(|failure| {
            use super::refresh::AuthorizationFailure;
            let (code, retryable) = match failure {
                AuthorizationFailure::SecretLoad => (error_codes::QUOTA_SECRET_LOAD, true),
                AuthorizationFailure::SecretMissing => (error_codes::QUOTA_SECRET_MISSING, false),
                AuthorizationFailure::SecretInvalid => (error_codes::QUOTA_SECRET_INVALID, false),
                AuthorizationFailure::Prepare | AuthorizationFailure::Stale => {
                    (error_codes::QUOTA_AUTHORIZATION_PREPARE, true)
                }
            };
            QuotaRefreshFailure::new(code, retryable)
        })?;
    let (mut credential, mut authorization, oauth_tokens) =
        (prepared.credential, prepared.header, prepared.oauth_tokens);
    let proxy = account_proxy_config(state, account, &credential)
        .map_err(|_| QuotaRefreshFailure::new(error_codes::QUOTA_PROXY_UNAVAILABLE, false))?;
    let request_timeout_seconds = state
        .store
        .quota_request_timeout_seconds()
        .map_err(|_| QuotaRefreshFailure::new(error_codes::QUOTA_POLICY_INVALID, false))?;
    let observed_at_ms = now_ms();
    let refresh_subscription = force_subscription_refresh
        || subscription_refresh_due(
            account.subscription.active_until_ms,
            account.subscription.updated_at_ms,
            observed_at_ms,
        );
    let client = CodexQuotaClient::new_with_proxy_and_timeout(
        proxy.as_ref(),
        Duration::from_secs(request_timeout_seconds),
    )?
    .with_http_scope(super::refresh::account_http_scope(state, fence));
    let first = client
        .refresh_data_with_subscription_authorized(
            authorization.clone(),
            &credential.chatgpt_account_id,
            observed_at_ms,
            &account.subscription,
            refresh_subscription,
        )
        .await;
    let failure = match first {
        Ok(data) => return Ok(data),
        Err(failure)
            if oauth_tokens.is_none()
                && credential.is_agent_identity()
                && is_agent_identity_task_invalid_failure(&failure) =>
        {
            let expected_task_id = credential.agent_task_id.clone().unwrap_or_default();
            (credential, authorization, _) = prepare_server_account_authorization(
                state,
                account,
                credential,
                Some(&expected_task_id),
            )
            .await
            .map_err(|_| {
                QuotaRefreshFailure::new(error_codes::QUOTA_AUTHORIZATION_PREPARE, true)
            })?;
            return client
                .refresh_data_with_subscription_authorized(
                    authorization,
                    &credential.chatgpt_account_id,
                    now_ms(),
                    &account.subscription,
                    refresh_subscription,
                )
                .await;
        }
        Err(failure) if oauth_tokens.is_none() && credential.is_agent_identity() => {
            return Err(failure);
        }
        Err(failure) if failure.http_status() == Some(401) => failure,
        Err(failure) => return Err(failure),
    };
    let Some(rejected_tokens) = oauth_tokens else {
        return Err(failure);
    };
    let Ok(tokens) = state
        .recover_account_tokens_after_unauthorized(account, &rejected_tokens)
        .await
    else {
        return Err(failure);
    };
    authorization = bearer_authorization(tokens.access_token())
        .map_err(|_| QuotaRefreshFailure::new(error_codes::QUOTA_TOKEN_PREPARE, true))?;
    client
        .refresh_data_with_subscription_authorized(
            authorization,
            &credential.chatgpt_account_id,
            observed_at_ms,
            &account.subscription,
            refresh_subscription,
        )
        .await
}
