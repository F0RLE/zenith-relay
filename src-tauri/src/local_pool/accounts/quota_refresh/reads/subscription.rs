use super::super::*;

pub(in crate::local_pool::accounts) async fn request_subscription_metadata(
    prepared: &PreparedAccountAuthorization,
    request_timeout: Duration,
    now_ms: u64,
    http_scope: &ManagementHttpScope,
) -> std::result::Result<Option<CodexSubscriptionMetadata>, QuotaRefreshFailure> {
    let Some(authorization) = prepared.subscription_authorization.clone() else {
        return Ok(None);
    };
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(request_timeout);
    let client = match prepared.proxy.as_ref() {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| QuotaRefreshFailure::new(error_codes::SUBSCRIPTION_CONFIGURATION, false))?;
    CodexSubscriptionClient::new(client)?
        .with_http_scope(http_scope.clone())
        .fetch_authorized(authorization, &prepared.provider_account_id, now_ms)
        .await
        .map(Some)
}

pub(super) fn record_subscription_failure(
    account_id: &str,
    prepared: &PreparedAccountAuthorization,
    failure: &QuotaRefreshFailure,
) {
    // Optional metadata failure is diagnostic, not an account/quota rejection.
    crate::diagnostics::record_error(
        "account-subscription-probe",
        Some(&failure.code),
        "account subscription check failed",
        &[
            ("account", crate::diagnostics::hash_identifier(account_id)),
            (
                "request_route",
                if prepared.proxy.is_some() {
                    "configured_proxy"
                } else {
                    "client_default"
                }
                .into(),
            ),
            (
                "http_status",
                failure
                    .http_status()
                    .map_or_else(|| "none".into(), |status| status.to_string()),
            ),
            ("retryable", failure.retryable.to_string()),
        ],
    );
}

pub(super) fn apply_subscription_read(
    quota: &mut QuotaRefreshOutcome,
    subscription: &mut Subscription,
    metadata: CodexSubscriptionMetadata,
    observed_at_ms: u64,
) {
    apply_subscription_metadata(subscription, metadata, observed_at_ms);
    match quota {
        QuotaRefreshOutcome::Updated(result) => {
            result.quota.subscription = Some(zenith_relay_core::quota::SubscriptionInput {
                plan_type: subscription.plan_type.clone(),
                active_until_ms: subscription.active_until_ms,
                forbidden: false,
                observed_at_ms,
            });
        }
        QuotaRefreshOutcome::Failed {
            subscription: previous,
            ..
        } => {
            *previous = subscription.clone();
        }
    }
}

pub(in crate::local_pool::accounts) fn apply_subscription_metadata(
    subscription: &mut Subscription,
    metadata: CodexSubscriptionMetadata,
    observed_at_ms: u64,
) {
    let mut plan_type = subscription.plan_type.clone();
    let mut active_until_ms = subscription.active_until_ms;
    merge_subscription_metadata_at(
        &mut plan_type,
        &mut active_until_ms,
        metadata,
        Some(observed_at_ms),
    );
    *subscription = Subscription::normalize(zenith_relay_core::quota::SubscriptionInput {
        plan_type,
        active_until_ms,
        forbidden: false,
        observed_at_ms,
    });
}

pub(super) async fn request_account_quota_metadata(
    prepared: &PreparedAccountAuthorization,
    request_timeout: Duration,
    now_ms: u64,
    subscription: &Subscription,
    http_scope: &ManagementHttpScope,
) -> LocalResult<std::result::Result<QuotaRefreshOutcome, tokio::time::error::Elapsed>> {
    let quota =
        CodexQuotaClient::new_with_proxy_and_timeout(prepared.proxy.as_ref(), request_timeout)
            .map_err(|failure| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    format!("failed to initialize quota client: {}", failure.code),
                )
            })?
            .with_http_scope(http_scope.clone());
    Ok(tokio::time::timeout(
        request_timeout.saturating_add(QUOTA_COMMAND_TIMEOUT_OVERHEAD),
        quota.refresh_quota_with_subscription_authorization(
            prepared.authorization.clone(),
            None,
            &prepared.provider_account_id,
            now_ms,
            subscription,
            false,
        ),
    )
    .await)
}

pub(super) fn respect_quota_retry_after(
    state: &DesktopState,
    scope: &AccountRefreshScope,
    refresh_result: &std::result::Result<QuotaRefreshOutcome, tokio::time::error::Elapsed>,
) {
    if let Ok(QuotaRefreshOutcome::Failed { failure, .. }) = refresh_result
        && let Some(delay) = failure.retry_after_ms()
    {
        state
            .refresh
            .respect_retry_after(&scope.fence.identity(), RefreshKind::Quota, delay);
    }
}

#[cfg(test)]
mod tests;
