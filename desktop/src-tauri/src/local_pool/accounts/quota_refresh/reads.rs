use super::*;
use zenith_relay_core::providers::chatgpt::{
    OAuthClientKind,
    connector::monitoring::{self, ModelMonitorHost, MonitorHost, QuotaMonitorHost},
};

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
    let result =
        monitoring::refresh_quota(&DesktopMonitor { state, scope }, force_subscription_refresh)
            .await;
    if let Err(error) = &result {
        record_read_error(state, scope, RefreshReadKind::Quota, error).await;
    }
    result
}

struct DesktopMonitor<'a> {
    state: &'a DesktopState,
    scope: &'a AccountRefreshScope,
}

impl MonitorHost for DesktopMonitor<'_> {
    type Authorization = PreparedAccountAuthorization;
    type Error = LocalPoolError;

    fn validate(&self) -> LocalResult<()> {
        self.scope.validate(self.state)
    }
    fn now_ms(&self) -> u64 {
        current_time_ms()
    }
    fn client_kind(auth: &Self::Authorization) -> OAuthClientKind {
        auth.oauth_client_kind
    }
    fn has_tokens(auth: &Self::Authorization) -> bool {
        auth.tokens.is_some()
    }
    fn has_agent_task(auth: &Self::Authorization) -> bool {
        auth.agent_task_id.is_some()
    }

    async fn prepare(&self) -> LocalResult<Self::Authorization> {
        refresh::request_authorization(self.state, &self.scope.fence).await
    }

    async fn recover(&self, rejected: &Self::Authorization) -> LocalResult<Self::Authorization> {
        PreparedAccountAuthorization::from_tokens(
            recover_account_authorization(
                self.state,
                &self.scope.fence.account_id,
                rejected,
                current_time_ms(),
            )
            .await?,
        )
    }

    async fn restore_task(
        &self,
        rejected: &Self::Authorization,
    ) -> LocalResult<Self::Authorization> {
        let account_id = &self.scope.fence.account_id;
        let stored = CredentialStore::from_backend(NativeSecretBackend)
            .require(account_id)
            .map_err(credential_local_error)?;
        match ensure_local_agent_identity_task(
            self.state,
            account_id,
            stored.clone(),
            rejected.agent_task_id.as_deref(),
        )
        .await
        {
            Ok(_) => prepare_account_request_authorization(self.state, account_id).await,
            Err(_) if stored.has_oauth() => PreparedAccountAuthorization::from_tokens(
                prepare_account_credentials(self.state, account_id).await?,
            ),
            Err(error) => Err(error),
        }
    }

    fn auth_state(&self) -> LocalResult<AccountAuthState> {
        self.state
            .store()?
            .account(&self.scope.fence.account_id)
            .map(|account| account.account.auth_state)
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account not found"))
    }

    async fn mark_access_only_rejected(&self, rejected: &Self::Authorization) -> LocalResult<()> {
        if let Some(tokens) = &rejected.tokens {
            mark_access_only_reauthentication(self.state, &self.scope.fence.account_id, tokens)
                .await?;
        }
        Ok(())
    }

    fn respect_retry_after(&self, kind: RefreshKind, delay_ms: u64) {
        self.state
            .refresh
            .respect_retry_after(&self.scope.fence.identity(), kind, delay_ms);
    }
}

impl QuotaMonitorHost for DesktopMonitor<'_> {
    type Output = AccountQuotaRefreshResponse;

    fn initial_subscription(&self) -> Subscription {
        self.scope.initial_account.account.subscription.clone()
    }

    fn skipped(&self) -> Self::Output {
        AccountQuotaRefreshResponse {
            account: self.scope.initial_account.clone(),
            quota: AccountQuotaOutcome::Skipped,
            exhaustion_transitions: Vec::new(),
        }
    }

    async fn read_quota(
        &self,
        auth: &Self::Authorization,
        previous: &Subscription,
    ) -> LocalResult<QuotaRefreshOutcome> {
        let timeout =
            Duration::from_secs(self.state.store()?.gateway().quota_request_timeout_seconds);
        Ok(subscription::request_account_quota_metadata(
            auth,
            timeout,
            current_time_ms(),
            previous,
            &self.scope.http_scope(self.state),
        )
        .await?
        .unwrap_or_else(|_| QuotaRefreshOutcome::Failed {
            failure: QuotaRefreshFailure::new(error_codes::QUOTA_TIMEOUT, true),
            subscription: previous.clone(),
        }))
    }

    async fn can_read_subscription(&self) -> LocalResult<bool> {
        let mut guard = self.state.subscription_refresh_guard().await;
        self.validate()?;
        Ok(guard.can_refresh(&self.scope.fence.identity(), self.state.refresh.now_ms()))
    }

    async fn read_subscription(
        &self,
        auth: &Self::Authorization,
        observed_at_ms: u64,
    ) -> Result<Option<CodexSubscriptionRead>, QuotaRefreshFailure> {
        let timeout = self
            .state
            .store()
            .map_err(|_| QuotaRefreshFailure::new(error_codes::SUBSCRIPTION_CONFIGURATION, false))?
            .gateway()
            .quota_request_timeout_seconds;
        subscription::request_subscription_metadata(
            auth,
            Duration::from_secs(timeout),
            observed_at_ms,
            &self.scope.http_scope(self.state),
        )
        .await
    }

    async fn subscription_failed(
        &self,
        auth: &Self::Authorization,
        failure: &QuotaRefreshFailure,
        elapsed: Duration,
    ) -> LocalResult<()> {
        subscription::record_subscription_failure(
            &self.scope.fence.account_id,
            auth,
            failure,
            elapsed,
        );
        let mut guard = self.state.subscription_refresh_guard().await;
        self.validate()?;
        guard.record_failure(
            &self.scope.fence.identity(),
            self.state.refresh.now_ms(),
            failure,
        );
        Ok(())
    }

    async fn subscription_succeeded(&self) -> LocalResult<()> {
        let mut guard = self.state.subscription_refresh_guard().await;
        self.validate()?;
        guard.record_success(&self.scope.fence.identity(), self.state.refresh.now_ms());
        Ok(())
    }

    async fn commit_quota(
        &self,
        quota: QuotaRefreshOutcome,
        subscription: SubscriptionObservation,
    ) -> LocalResult<Self::Output> {
        let _mutation = self.state.setup_guard().await;
        let applied = {
            let mut store = self.state.store()?;
            apply_quota_read(&mut store, self.scope, quota, subscription, None)?
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
            self.state
                .refresh
                .mark_dirty(&self.scope.fence.identity(), RefreshKind::Models);
        }
        sync_refreshed_account_or_rollback(
            self.state,
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
}

impl ModelMonitorHost for DesktopMonitor<'_> {
    async fn read_models(
        &self,
        auth: &Self::Authorization,
    ) -> Result<Vec<String>, ModelDiscoveryFailure> {
        models::discover_account_models(
            self.state,
            &self.scope.fence.account_id,
            auth,
            &self.scope.http_scope(self.state),
        )
        .await
    }

    async fn commit_models(
        &self,
        client: OAuthClientKind,
        models: Result<Vec<String>, ModelDiscoveryFailure>,
    ) -> LocalResult<()> {
        let _mutation = self.state.setup_guard().await;
        let applied = {
            let mut store = self.state.store()?;
            apply_models_read(&mut store, self.scope, client, models)?
        };
        sync_refreshed_account_or_rollback(
            self.state,
            applied.previous_account,
            applied.account,
            applied.refresh_result,
        )
        .await
    }
}

mod authorization;
mod models;
mod subscription;

pub(in crate::local_pool) use authorization::prepare_account_request_authorization;
pub(in crate::local_pool::accounts) use authorization::{
    ensure_local_agent_identity_task, mark_access_only_reauthentication,
    recover_account_authorization,
};
#[cfg(test)]
pub(in crate::local_pool::accounts) use authorization::{
    model_discovery_was_unauthorized, quota_refresh_was_unauthorized,
};
pub(in crate::local_pool) use models::read_account_models_once;
pub(crate) use models::refresh_account_models_once;
#[cfg(test)]
pub(in crate::local_pool::accounts) use subscription::apply_subscription_metadata;
