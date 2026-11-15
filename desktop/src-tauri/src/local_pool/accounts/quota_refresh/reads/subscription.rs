use super::super::*;

pub(in crate::local_pool::accounts) async fn request_subscription_metadata(
    prepared: &PreparedAccountAuthorization,
    request_timeout: Duration,
    now_ms: u64,
    http_scope: &ManagementHttpScope,
) -> std::result::Result<Option<CodexSubscriptionRead>, QuotaRefreshFailure> {
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
        .read_authorized(authorization, &prepared.provider_account_id, now_ms)
        .await
        .map(Some)
}

pub(super) fn record_subscription_failure(
    account_id: &str,
    prepared: &PreparedAccountAuthorization,
    failure: &QuotaRefreshFailure,
    elapsed: Duration,
) {
    if optional_expiry_missing(failure) && !crate::diagnostics::is_debug_enabled() {
        return;
    }
    let details = &[
        ("code", failure.code.clone()),
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
        (
            "retry_after_ms",
            failure
                .retry_after_ms()
                .map_or_else(|| "unreported".into(), |delay| delay.to_string()),
        ),
        ("elapsed_ms", elapsed.as_millis().to_string()),
        (
            "oauth_client_kind",
            format!("{:?}", prepared.oauth_client_kind),
        ),
        ("cf_ray", failure.cf_ray().unwrap_or("unreported").into()),
        (
            "endpoint_stage",
            failure.endpoint_stage().unwrap_or("unknown").into(),
        ),
        (
            "response_format",
            failure.response_format().unwrap_or("unknown").into(),
        ),
        (
            "response_mitigation",
            failure.response_mitigation().unwrap_or("unreported").into(),
        ),
        (
            "transport_failure",
            failure.transport_failure().unwrap_or("unreported").into(),
        ),
    ];
    if optional_expiry_missing(failure) {
        crate::diagnostics::record_operation(
            "account-subscription-probe",
            "optional subscription expiry is unavailable",
            details,
        );
    } else {
        crate::diagnostics::record_error(
            "account-subscription-probe",
            Some(&failure.code),
            "account subscription check failed",
            details,
        );
    }
}

fn optional_expiry_missing(failure: &QuotaRefreshFailure) -> bool {
    failure.code == error_codes::SUBSCRIPTION_HTTP_STATUS
        && failure.endpoint_stage() == Some("subscriptions")
        && failure.http_status() == Some(404)
        && failure.response_format() == Some("json")
        && failure.response_mitigation().is_none()
}

#[cfg(test)]
pub(in crate::local_pool::accounts) use zenith_relay_core::providers::chatgpt::connector::monitoring::{
    apply_partial_subscription_read, apply_subscription_metadata, apply_subscription_read,
};

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

#[cfg(test)]
mod tests;
