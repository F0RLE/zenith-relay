//! Fenced account observation workflows. Hosts supply storage, authorization,
//! scheduling and diagnostics; this service owns retries and completion order.
use super::observations::SubscriptionObservation;
use crate::{
    accounts::AccountAuthState,
    error_codes,
    providers::chatgpt::{
        CodexSubscriptionMetadata, CodexSubscriptionRead, ModelDiscoveryFailure,
        ModelDiscoveryFailureCode, OAuthClientKind, QuotaRefreshOutcome,
        is_agent_identity_task_invalid_failure, merge_subscription_metadata_at,
        subscription_refresh_due,
    },
    quota::{QuotaRefreshFailure, Subscription, SubscriptionInput, SubscriptionStatus},
    scheduler::refresh::RefreshKind,
};
use std::{
    future::Future,
    time::{Duration, Instant},
};

pub trait MonitorHost: Sync {
    type Authorization: Send + Sync;
    type Error: Send;

    fn validate(&self) -> Result<(), Self::Error>;
    fn now_ms(&self) -> u64;
    fn client_kind(auth: &Self::Authorization) -> OAuthClientKind;
    fn has_tokens(auth: &Self::Authorization) -> bool;
    fn has_agent_task(auth: &Self::Authorization) -> bool;
    fn prepare(&self) -> impl Future<Output = Result<Self::Authorization, Self::Error>> + Send;
    fn recover(
        &self,
        rejected: &Self::Authorization,
    ) -> impl Future<Output = Result<Self::Authorization, Self::Error>> + Send;
    fn restore_task(
        &self,
        rejected: &Self::Authorization,
    ) -> impl Future<Output = Result<Self::Authorization, Self::Error>> + Send;
    fn auth_state(&self) -> Result<AccountAuthState, Self::Error>;
    fn mark_access_only_rejected(
        &self,
        rejected: &Self::Authorization,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn respect_retry_after(&self, kind: RefreshKind, delay_ms: u64);
}

pub trait QuotaMonitorHost: MonitorHost {
    type Output: Send;

    fn initial_subscription(&self) -> Subscription;
    fn skipped(&self) -> Self::Output;
    fn read_quota(
        &self,
        auth: &Self::Authorization,
        subscription: &Subscription,
    ) -> impl Future<Output = Result<QuotaRefreshOutcome, Self::Error>> + Send;
    fn can_read_subscription(&self) -> impl Future<Output = Result<bool, Self::Error>> + Send;
    fn read_subscription(
        &self,
        auth: &Self::Authorization,
        observed_at_ms: u64,
    ) -> impl Future<Output = Result<Option<CodexSubscriptionRead>, QuotaRefreshFailure>> + Send;
    fn subscription_failed(
        &self,
        auth: &Self::Authorization,
        failure: &QuotaRefreshFailure,
        elapsed: Duration,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn subscription_succeeded(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;
    /// Validate and merge under the host mutation lock, persist, then publish.
    /// On publication failure conditionally restore only this operation's write.
    fn commit_quota(
        &self,
        quota: QuotaRefreshOutcome,
        subscription: SubscriptionObservation,
    ) -> impl Future<Output = Result<Self::Output, Self::Error>> + Send;
}

async fn read_quota<H: QuotaMonitorHost>(
    host: &H,
    auth: &H::Authorization,
    subscription: &Subscription,
) -> Result<QuotaRefreshOutcome, H::Error> {
    host.validate()?;
    let outcome = host.read_quota(auth, subscription).await?;
    host.validate()?;
    if let QuotaRefreshOutcome::Failed { failure, .. } = &outcome
        && let Some(delay) = failure.retry_after_ms()
    {
        host.respect_retry_after(RefreshKind::Quota, delay);
    }
    Ok(outcome)
}

pub async fn refresh_quota<H: QuotaMonitorHost>(
    host: &H,
    force_subscription: bool,
) -> Result<H::Output, H::Error> {
    host.validate()?;
    let mut auth = host.prepare().await?;
    host.validate()?;
    if H::client_kind(&auth) == OAuthClientKind::Siwc {
        return Ok(host.skipped());
    }
    let mut subscription = host.initial_subscription();
    let mut quota = read_quota(host, &auth, &subscription).await?;
    if H::has_tokens(&auth)
        && matches!(&quota, QuotaRefreshOutcome::Failed { failure, .. } if failure.http_status() == Some(401))
    {
        match host.recover(&auth).await {
            Ok(recovered) => {
                auth = recovered;
                quota = read_quota(host, &auth, &subscription).await?;
            }
            Err(_) => {
                host.validate()?;
                let state = host.auth_state()?;
                if state == AccountAuthState::DegradedAccessOnly {
                    host.mark_access_only_rejected(&auth).await?;
                } else if !state.requires_fresh_login() {
                    quota = QuotaRefreshOutcome::Failed {
                        failure: QuotaRefreshFailure::new(error_codes::QUOTA_TOKEN_REFRESH, true),
                        subscription: subscription.clone(),
                    };
                }
            }
        }
    } else if H::has_agent_task(&auth)
        && matches!(&quota, QuotaRefreshOutcome::Failed { failure, .. } if is_agent_identity_task_invalid_failure(failure))
    {
        host.validate()?;
        auth = host.restore_task(&auth).await?;
        quota = read_quota(host, &auth, &subscription).await?;
    }
    host.validate()?;
    if let QuotaRefreshOutcome::Updated(result) = &quota
        && let Some(input) = &result.quota.subscription
    {
        subscription = Subscription::normalize(input.clone());
    }
    let mut successful_subscription = false;
    let mut partial_subscription = false;
    if (force_subscription
        || subscription_refresh_due(
            subscription.active_until_ms,
            subscription.updated_at_ms,
            host.now_ms(),
        ))
        && host.can_read_subscription().await?
    {
        host.validate()?;
        let observed_at_ms = host.now_ms();
        let started = Instant::now();
        let metadata = host.read_subscription(&auth, observed_at_ms).await;
        host.validate()?;
        match metadata {
            Ok(Some(read)) => {
                if let Some(failure) = read.fallback_failure {
                    partial_subscription = true;
                    apply_partial_subscription_read(
                        &mut quota,
                        &mut subscription,
                        read.metadata,
                        observed_at_ms,
                    );
                    host.subscription_failed(&auth, &failure, started.elapsed())
                        .await?;
                } else {
                    apply_subscription_read(
                        &mut quota,
                        &mut subscription,
                        read.metadata,
                        observed_at_ms,
                    );
                    successful_subscription = true;
                }
            }
            Ok(None) => {}
            Err(failure) => {
                host.subscription_failed(&auth, &failure, started.elapsed())
                    .await?
            }
        }
    }
    host.validate()?;
    let observation = if partial_subscription {
        SubscriptionObservation::Partial(subscription)
    } else {
        SubscriptionObservation::Timestamped(subscription)
    };
    let output = host.commit_quota(quota, observation).await?;
    if successful_subscription {
        host.validate()?;
        host.subscription_succeeded().await?;
    }
    Ok(output)
}

pub trait ModelMonitorHost: MonitorHost {
    fn read_models(
        &self,
        auth: &Self::Authorization,
    ) -> impl Future<Output = Result<Vec<String>, ModelDiscoveryFailure>> + Send;
    fn commit_models(
        &self,
        client: OAuthClientKind,
        models: Result<Vec<String>, ModelDiscoveryFailure>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

async fn read_models<H: ModelMonitorHost>(
    host: &H,
    auth: &H::Authorization,
) -> Result<Result<Vec<String>, ModelDiscoveryFailure>, H::Error> {
    host.validate()?;
    let result = host.read_models(auth).await;
    host.validate()?;
    if let Some(delay) = result
        .as_ref()
        .err()
        .and_then(|failure| failure.retry_after_ms)
    {
        host.respect_retry_after(RefreshKind::Models, delay);
    }
    Ok(result)
}

pub async fn refresh_models<H: ModelMonitorHost>(host: &H) -> Result<bool, H::Error> {
    host.validate()?;
    let mut auth = host.prepare().await?;
    let mut models = read_models(host, &auth).await?;
    if H::has_tokens(&auth)
        && matches!(&models, Err(failure) if failure.code == ModelDiscoveryFailureCode::Unauthorized)
    {
        match host.recover(&auth).await {
            Ok(recovered) => {
                auth = recovered;
                models = read_models(host, &auth).await?;
            }
            Err(_) => {
                host.validate()?;
                if host.auth_state()? == AccountAuthState::DegradedAccessOnly {
                    host.mark_access_only_rejected(&auth).await?;
                }
            }
        }
    }
    if H::has_agent_task(&auth)
        && matches!(&models, Err(failure) if failure.code == ModelDiscoveryFailureCode::AgentTaskInvalid)
    {
        host.validate()?;
        auth = host.restore_task(&auth).await?;
        models = read_models(host, &auth).await?;
    }
    host.validate()?;
    let succeeded = models.is_ok();
    host.commit_models(H::client_kind(&auth), models).await?;
    Ok(succeeded)
}

pub fn apply_subscription_read(
    quota: &mut QuotaRefreshOutcome,
    subscription: &mut Subscription,
    metadata: CodexSubscriptionMetadata,
    observed_at_ms: u64,
) {
    apply_subscription_metadata(subscription, metadata, observed_at_ms);
    match quota {
        QuotaRefreshOutcome::Updated(result) => {
            result.quota.subscription = Some(SubscriptionInput {
                plan_type: subscription.plan_type.clone(),
                active_until_ms: subscription.active_until_ms,
                forbidden: false,
                observed_at_ms,
            });
        }
        QuotaRefreshOutcome::Failed {
            subscription: previous,
            ..
        } => *previous = subscription.clone(),
    }
}

pub fn apply_subscription_metadata(
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
    *subscription = Subscription::normalize(SubscriptionInput {
        plan_type,
        active_until_ms,
        forbidden: false,
        observed_at_ms,
    });
}

pub fn apply_partial_subscription_read(
    quota: &mut QuotaRefreshOutcome,
    subscription: &mut Subscription,
    metadata: CodexSubscriptionMetadata,
    observed_at_ms: u64,
) {
    let plan_changed = crate::quota::subscription_plan_changed(
        subscription.plan_type.as_deref(),
        metadata.plan_type.as_deref(),
    );
    let previous_updated_at_ms = subscription.updated_at_ms;
    let mut plan_type = subscription.plan_type.clone();
    let mut active_until_ms = subscription.active_until_ms;
    merge_subscription_metadata_at(&mut plan_type, &mut active_until_ms, metadata, None);
    let mut partial = Subscription::normalize(SubscriptionInput {
        plan_type,
        active_until_ms,
        forbidden: subscription.status == SubscriptionStatus::Forbidden,
        observed_at_ms,
    });
    partial.updated_at_ms = if plan_changed {
        None
    } else {
        previous_updated_at_ms
    };
    *subscription = partial;
    if let QuotaRefreshOutcome::Failed {
        subscription: previous,
        ..
    } = quota
    {
        *previous = subscription.clone();
    }
}

#[cfg(test)]
mod tests;
