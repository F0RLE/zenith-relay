use super::*;

pub(crate) async fn refresh_account_quota_once(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<AccountQuotaRefreshResponse> {
    match refresh::request(state, account_id, RefreshKind::Quota).await? {
        RefreshRead::Quota(response) => Ok(*response),
        _ => Err(LocalPoolError::invalid_state(
            "unexpected quota refresh result",
        )),
    }
}

pub(in crate::local_pool) async fn read_account_quota_once(
    state: &DesktopState,
    scope: &AccountRefreshScope,
    force_subscription_refresh: bool,
) -> LocalResult<AccountQuotaRefreshResponse> {
    let quota_lock = state.quota_account_lock(&scope.fence.account_id)?;
    let _quota_guard = quota_lock.lock().await;
    scope.validate(state)?;
    let quota_result = read_account_quota(state, scope, force_subscription_refresh).await;
    if let Err(error) = &quota_result {
        record_read_error(state, scope, RefreshReadKind::Quota, error).await;
    }
    quota_result
}

async fn read_account_quota(
    state: &DesktopState,
    scope: &AccountRefreshScope,
    force_subscription_refresh: bool,
) -> LocalResult<AccountQuotaRefreshResponse> {
    let account_id = &scope.fence.account_id;
    let mut prepared = refresh::request_authorization(state, &scope.fence).await?;
    scope.validate(state)?;
    if prepared.oauth_client_kind == zenith_relay_core::providers::chatgpt::OAuthClientKind::Siwc {
        return Ok(AccountQuotaRefreshResponse {
            account: scope.initial_account.clone(),
            quota: AccountQuotaOutcome::Skipped,
            exhaustion_transitions: Vec::new(),
        });
    }
    let http_scope = scope.http_scope(state);
    let now_ms = current_time_ms();
    let request_timeout =
        Duration::from_secs(state.store()?.gateway().quota_request_timeout_seconds);
    let account_before_refresh = &scope.initial_account;
    let mut subscription = account_before_refresh.account.subscription.clone();
    // Import/login owns token hints. A later poll must not resurrect a date
    // cleared by the provider, or promote an old JWT claim to fresh evidence.
    scope.validate(state)?;
    let mut refreshed = subscription::request_account_quota_metadata(
        &prepared,
        request_timeout,
        now_ms,
        &subscription,
        &http_scope,
    )
    .await?;
    subscription::respect_quota_retry_after(state, scope, &refreshed);
    scope.validate(state)?;
    if let Some(rejected_tokens) = prepared
        .tokens
        .as_ref()
        .filter(|_| quota_refresh_was_unauthorized(&refreshed))
    {
        match recover_account_authorization(
            state,
            account_id,
            Some(rejected_tokens.generation()),
            current_time_ms(),
        )
        .await
        {
            Ok(recovered) => {
                prepared = PreparedAccountAuthorization::from_tokens(recovered)?;
                scope.validate(state)?;
                refreshed = subscription::request_account_quota_metadata(
                    &prepared,
                    request_timeout,
                    current_time_ms(),
                    &subscription,
                    &http_scope,
                )
                .await?;
                subscription::respect_quota_retry_after(state, scope, &refreshed);
            }
            Err(_) if account_auth_is_access_only(state, account_id)? => {
                mark_access_only_reauthentication(state, account_id, rejected_tokens).await?;
            }
            Err(_) if !account_requires_reauthentication(state, account_id)? => {
                refreshed = Ok(QuotaRefreshOutcome::Failed {
                    failure: QuotaRefreshFailure::new(error_codes::QUOTA_TOKEN_REFRESH, true),
                    subscription: subscription.clone(),
                });
            }
            Err(_) => {}
        }
    } else if let Some(task_id) = prepared.agent_task_id.as_deref() {
        let invalid_task = quota_refresh_has_invalid_agent_task(&refreshed);
        if invalid_task {
            let stored = CredentialStore::from_backend(NativeSecretBackend)
                .require(account_id)
                .map_err(credential_local_error)?;
            prepared = match ensure_local_agent_identity_task(
                state,
                account_id,
                stored.clone(),
                Some(task_id),
            )
            .await
            {
                Ok(_) => prepare_account_request_authorization(state, account_id).await?,
                Err(_) if stored.has_oauth() => PreparedAccountAuthorization::from_tokens(
                    prepare_account_credentials(state, account_id).await?,
                )?,
                Err(error) => return Err(error),
            };
            scope.validate(state)?;
            refreshed = subscription::request_account_quota_metadata(
                &prepared,
                request_timeout,
                current_time_ms(),
                &subscription,
                &http_scope,
            )
            .await?;
            subscription::respect_quota_retry_after(state, scope, &refreshed);
        }
    }

    let mut quota = refreshed.unwrap_or_else(|_| QuotaRefreshOutcome::Failed {
        failure: QuotaRefreshFailure::new(error_codes::QUOTA_TIMEOUT, true),
        subscription: subscription.clone(),
    });
    if let QuotaRefreshOutcome::Updated(result) = &quota
        && let Some(input) = &result.quota.subscription
    {
        subscription = Subscription::normalize(input.clone());
    }
    // The independent read has one owner and runs after authorization recovery.
    // Quota-only replies must neither duplicate it nor postpone its next poll.
    let observed_at_ms = current_time_ms();
    if force_subscription_refresh
        || subscription_refresh_due(
            subscription.active_until_ms,
            subscription.updated_at_ms,
            observed_at_ms,
        )
    {
        let mut subscription_guard = state.subscription_refresh_guard().await;
        scope.validate(state)?;
        let identity = scope.fence.identity();
        // Manual refresh also respects the metadata endpoint's floor, without
        // delaying independent quota reads or auth recovery for this account.
        if subscription_guard.can_refresh(&identity, state.refresh.now_ms()) {
            let observed_at_ms = current_time_ms();
            let metadata = subscription::request_subscription_metadata(
                &prepared,
                request_timeout,
                observed_at_ms,
                &http_scope,
            )
            .await;
            scope.validate(state)?;
            match metadata {
                Ok(Some(metadata)) => subscription::apply_subscription_read(
                    &mut quota,
                    &mut subscription,
                    metadata,
                    observed_at_ms,
                ),
                Ok(None) => {}
                Err(failure) => {
                    subscription::record_subscription_failure(account_id, &prepared, &failure);
                    subscription_guard.record_failure(&identity, state.refresh.now_ms(), &failure);
                }
            }
        }
    }
    let _mutation = state.setup_guard().await;
    let applied = {
        let mut store = state.store()?;
        apply_quota_read(&mut store, scope, quota, subscription, None)?
    };
    if zenith_relay_core::quota::subscription_plan_changed(
        applied
            .previous_account
            .account
            .subscription
            .plan_type
            .as_deref(),
        applied.account.account.subscription.plan_type.as_deref(),
    ) {
        state
            .refresh
            .mark_dirty(&scope.fence.identity(), RefreshKind::Models);
    }
    sync_refreshed_account_or_rollback(
        state,
        applied.previous_account,
        applied.account.clone(),
        applied.refresh_result.models_changed,
    )
    .await?;
    Ok(AccountQuotaRefreshResponse {
        account: applied.account,
        quota: applied.refresh_result.outcome,
        exhaustion_transitions: applied.refresh_result.exhaustion_transitions,
    })
}

mod authorization;
/// Request the independent model-kind job (not an inline quota subrequest).
mod models;
mod subscription;

pub(in crate::local_pool) use models::read_account_models_once;
pub(crate) use models::refresh_account_models_once;
#[cfg(test)]
pub(in crate::local_pool::accounts) use subscription::apply_subscription_metadata;

pub(in crate::local_pool) use authorization::prepare_account_request_authorization;
pub(in crate::local_pool::accounts) use authorization::{
    account_auth_is_access_only, account_requires_reauthentication,
    ensure_local_agent_identity_task, mark_access_only_reauthentication,
    model_discovery_has_invalid_agent_task, model_discovery_was_unauthorized,
    quota_refresh_has_invalid_agent_task, quota_refresh_was_unauthorized,
    recover_account_authorization,
};
