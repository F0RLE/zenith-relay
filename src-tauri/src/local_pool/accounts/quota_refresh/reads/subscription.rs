use super::super::*;

pub(in crate::local_pool::accounts) async fn request_subscription_metadata(
    prepared: &PreparedAccountAuthorization,
    request_timeout: Duration,
    now_ms: u64,
    http_scope: &ManagementHttpScope,
) -> Option<CodexSubscriptionMetadata> {
    let authorization = prepared.subscription_authorization.clone()?;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(request_timeout);
    let client = match prepared.proxy.as_ref() {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .ok()?;
    CodexSubscriptionClient::new(client)
        .ok()?
        .with_http_scope(http_scope.clone())
        .fetch_authorized(authorization, &prepared.provider_account_id, now_ms)
        .await
        .ok()
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
    refresh_subscription: bool,
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
            prepared.subscription_authorization.clone(),
            &prepared.provider_account_id,
            now_ms,
            subscription,
            refresh_subscription,
        ),
    )
    .await)
}

pub(super) fn respect_quota_retry_after(
    state: &DesktopState,
    scope: &AccountRefreshScope,
    refresh_result: &std::result::Result<QuotaRefreshOutcome, tokio::time::error::Elapsed>,
) {
    if let Ok(QuotaRefreshOutcome::Failed { failure, .. }) = refresh_result {
        if let Some(delay) = failure.retry_after_ms() {
            state
                .refresh
                .respect_retry_after(&scope.fence.identity(), RefreshKind::Quota, delay);
        }
    }
}
