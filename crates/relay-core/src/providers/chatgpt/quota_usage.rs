use super::{
    agent_identity::is_agent_identity_task_invalid_response,
    bearer_authorization as shared_bearer_authorization, collect_response_body,
    quota_subscription::{merge_subscription_metadata_at, CodexSubscriptionClient},
    valid_access_token, ResponseBodyError,
};
use crate::error_codes;
use crate::quota::{
    QuotaAdapter, QuotaAdapterCapabilities, QuotaAdapterContext, QuotaRefreshFailure,
    QuotaRefreshResult, QuotaWindowKind, Subscription, SubscriptionInput,
};
use crate::scheduler::refresh::http::{HttpClass, ManagementHttpScope};
use crate::{providers::chatgpt::CodexIdentityEnvelope, ProxyConfig};
use futures_util::future::BoxFuture;
use reqwest::{
    header::{HeaderValue, ACCEPT, AUTHORIZATION},
    redirect::Policy,
};
use std::{collections::BTreeSet, time::Duration};
use url::Url;

pub const CODEX_QUOTA_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";
#[cfg(test)]
const ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";
const MAX_ACCOUNT_ID_BYTES: usize = 512;
const MAX_QUOTA_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Clone)]
pub struct CodexQuotaClient {
    http: reqwest::Client,
    usage_endpoint: Url,
    subscription: CodexSubscriptionClient,
    scope: ManagementHttpScope,
}

impl CodexQuotaClient {
    pub fn new() -> Result<Self, QuotaRefreshFailure> {
        Self::new_with_proxy(None)
    }

    pub fn new_with_proxy(proxy: Option<&ProxyConfig>) -> Result<Self, QuotaRefreshFailure> {
        Self::new_with_proxy_and_timeout(proxy, Duration::from_secs(20))
    }

    pub fn new_with_proxy_and_timeout(
        proxy: Option<&ProxyConfig>,
        request_timeout: Duration,
    ) -> Result<Self, QuotaRefreshFailure> {
        let usage_endpoint = Url::parse(CODEX_QUOTA_ENDPOINT)
            .map_err(|_| QuotaRefreshFailure::new(error_codes::INVALID_CONFIGURATION, false))?;
        Self::with_endpoint_proxy_and_timeout(usage_endpoint, proxy, request_timeout)
    }

    #[cfg(test)]
    pub(super) fn with_endpoint(usage_endpoint: Url) -> Result<Self, QuotaRefreshFailure> {
        Self::with_endpoint_proxy_and_timeout(usage_endpoint, None, Duration::from_secs(20))
    }

    fn with_endpoint_proxy_and_timeout(
        usage_endpoint: Url,
        proxy: Option<&ProxyConfig>,
        request_timeout: Duration,
    ) -> Result<Self, QuotaRefreshFailure> {
        let builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(request_timeout)
            .user_agent("Zenith Relay");
        let http = match proxy {
            Some(proxy) => proxy.apply(builder),
            None => builder,
        }
        .build()
        .map_err(|_| QuotaRefreshFailure::new(error_codes::INVALID_CONFIGURATION, false))?;
        let subscription = CodexSubscriptionClient::new(http.clone())?;
        Ok(Self {
            http,
            usage_endpoint,
            subscription,
            scope: ManagementHttpScope::default(),
        })
    }

    pub fn with_http_scope(mut self, scope: ManagementHttpScope) -> Self {
        self.subscription = self.subscription.with_http_scope(scope.clone());
        self.scope = scope;
        self
    }

    pub fn capabilities(&self) -> QuotaAdapterCapabilities {
        let windows = BTreeSet::from([QuotaWindowKind::Primary, QuotaWindowKind::Secondary]);
        QuotaAdapterCapabilities {
            supports_quota: true,
            supports_subscription: true,
            supported_windows: windows.clone(),
            wake_windows: windows,
        }
    }

    pub async fn refresh_quota(
        &self,
        access_token: &str,
        chatgpt_account_id: &str,
        now_ms: u64,
        previous_subscription: &Subscription,
        refresh_subscription: bool,
    ) -> QuotaRefreshOutcome {
        let authorization = match bearer_authorization(access_token) {
            Ok(authorization) => authorization,
            Err(failure) => {
                return QuotaRefreshOutcome::Failed {
                    failure,
                    subscription: previous_subscription.clone(),
                }
            }
        };
        self.refresh_quota_authorized(
            authorization,
            chatgpt_account_id,
            now_ms,
            previous_subscription,
            refresh_subscription,
        )
        .await
    }

    pub async fn refresh_quota_authorized(
        &self,
        authorization: HeaderValue,
        chatgpt_account_id: &str,
        now_ms: u64,
        previous_subscription: &Subscription,
        refresh_subscription: bool,
    ) -> QuotaRefreshOutcome {
        self.refresh_quota_with_subscription_authorization(
            authorization.clone(),
            Some(authorization),
            chatgpt_account_id,
            now_ms,
            previous_subscription,
            refresh_subscription,
        )
        .await
    }

    pub async fn refresh_quota_with_subscription_authorization(
        &self,
        authorization: HeaderValue,
        subscription_authorization: Option<HeaderValue>,
        chatgpt_account_id: &str,
        now_ms: u64,
        previous_subscription: &Subscription,
        refresh_subscription: bool,
    ) -> QuotaRefreshOutcome {
        match self
            .refresh_data_with_subscription_authorization(
                authorization,
                subscription_authorization,
                chatgpt_account_id,
                now_ms,
                previous_subscription,
                refresh_subscription,
            )
            .await
        {
            Ok(data) => QuotaRefreshOutcome::Updated(Box::new(data)),
            Err(failure) => QuotaRefreshOutcome::Failed {
                failure,
                subscription: previous_subscription.clone(),
            },
        }
    }

    pub async fn refresh_data(
        &self,
        access_token: &str,
        chatgpt_account_id: &str,
        now_ms: u64,
    ) -> Result<QuotaRefreshResult, QuotaRefreshFailure> {
        let authorization = bearer_authorization(access_token)?;
        self.refresh_data_authorized(authorization, chatgpt_account_id, now_ms)
            .await
    }

    pub async fn refresh_data_authorized(
        &self,
        authorization: HeaderValue,
        chatgpt_account_id: &str,
        now_ms: u64,
    ) -> Result<QuotaRefreshResult, QuotaRefreshFailure> {
        if chatgpt_account_id.is_empty() || chatgpt_account_id.len() > MAX_ACCOUNT_ID_BYTES {
            return Err(QuotaRefreshFailure::new(
                error_codes::INVALID_CHATGPT_ACCOUNT_ID,
                false,
            ));
        }
        let identity = CodexIdentityEnvelope::standard(chatgpt_account_id).map_err(|_| {
            QuotaRefreshFailure::new(error_codes::INVALID_CHATGPT_ACCOUNT_ID, false)
        })?;
        let (response, permit) = self
            .scope
            .send(
                &self.http,
                identity.apply(
                    self.http
                        .get(self.usage_endpoint.clone())
                        .header(AUTHORIZATION, authorization)
                        .header(ACCEPT, "application/json"),
                ),
                HttpClass::Ordinary,
            )
            .await
            .map_err(|_| QuotaRefreshFailure::new(error_codes::QUOTA_TRANSPORT, true))?;
        let status = response.status();
        let retry_after_ms =
            crate::transport::retry_after_ms(response.headers(), std::time::SystemTime::now());
        let body = collect_response_body(response, MAX_QUOTA_RESPONSE_BYTES)
            .await
            .map_err(|error| match error {
                ResponseBodyError::Transport => {
                    QuotaRefreshFailure::new(error_codes::QUOTA_TRANSPORT, true)
                }
                ResponseBodyError::TooLarge => {
                    QuotaRefreshFailure::new(error_codes::QUOTA_RESPONSE_TOO_LARGE, false)
                }
            })
            .map_err(|failure| failure.with_retry_after(retry_after_ms))?;
        drop(permit);
        if !status.is_success() {
            return Err(
                classify_quota_failure(status.as_u16(), &body).with_retry_after(retry_after_ms)
            );
        }
        parse_codex_usage(&body, now_ms)
    }

    pub async fn refresh_data_with_subscription(
        &self,
        access_token: &str,
        chatgpt_account_id: &str,
        now_ms: u64,
        previous_subscription: &Subscription,
        refresh_subscription: bool,
    ) -> Result<QuotaRefreshResult, QuotaRefreshFailure> {
        let authorization = bearer_authorization(access_token)?;
        self.refresh_data_with_subscription_authorized(
            authorization,
            chatgpt_account_id,
            now_ms,
            previous_subscription,
            refresh_subscription,
        )
        .await
    }

    pub async fn refresh_data_with_subscription_authorized(
        &self,
        authorization: HeaderValue,
        chatgpt_account_id: &str,
        now_ms: u64,
        previous_subscription: &Subscription,
        refresh_subscription: bool,
    ) -> Result<QuotaRefreshResult, QuotaRefreshFailure> {
        self.refresh_data_with_subscription_authorization(
            authorization.clone(),
            Some(authorization),
            chatgpt_account_id,
            now_ms,
            previous_subscription,
            refresh_subscription,
        )
        .await
    }

    pub async fn refresh_data_with_subscription_authorization(
        &self,
        authorization: HeaderValue,
        subscription_authorization: Option<HeaderValue>,
        chatgpt_account_id: &str,
        now_ms: u64,
        previous_subscription: &Subscription,
        refresh_subscription: bool,
    ) -> Result<QuotaRefreshResult, QuotaRefreshFailure> {
        let mut data = self
            .refresh_data_authorized(authorization, chatgpt_account_id, now_ms)
            .await?;
        data.quota
            .preserve_subscription_metadata(previous_subscription);
        if refresh_subscription {
            let metadata = match subscription_authorization {
                Some(authorization) => Some(
                    self.subscription
                        .fetch_authorized(authorization, chatgpt_account_id, now_ms)
                        .await,
                ),
                None => None,
            };
            let input = data
                .quota
                .subscription
                .get_or_insert_with(|| SubscriptionInput {
                    plan_type: previous_subscription.plan_type.clone(),
                    active_until_ms: previous_subscription.active_until_ms,
                    forbidden: false,
                    observed_at_ms: now_ms,
                });
            match metadata {
                Some(Ok(metadata)) => {
                    merge_subscription_metadata_at(
                        &mut input.plan_type,
                        &mut input.active_until_ms,
                        metadata,
                        Some(now_ms),
                    );
                    input.observed_at_ms = now_ms;
                }
                Some(Err(_)) | None => input.observed_at_ms = now_ms,
            }
        } else if let Some(input) = data.quota.subscription.as_mut() {
            if input.plan_type == previous_subscription.plan_type && input.active_until_ms.is_none()
            {
                input.observed_at_ms = previous_subscription.updated_at_ms.unwrap_or(now_ms);
            }
        }
        Ok(data)
    }
}

impl QuotaAdapter for CodexQuotaClient {
    fn capabilities(&self) -> QuotaAdapterCapabilities {
        CodexQuotaClient::capabilities(self)
    }

    fn refresh<'a>(
        &'a self,
        context: &'a QuotaAdapterContext,
        access_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<QuotaRefreshResult, QuotaRefreshFailure>> {
        Box::pin(async move {
            self.refresh_data(access_token, &context.provider_account_id, now_ms)
                .await
        })
    }
}

pub fn is_agent_identity_task_invalid_failure(failure: &QuotaRefreshFailure) -> bool {
    failure.http_status() == Some(401)
        && matches!(
            failure.code.as_str(),
            error_codes::INVALID_TASK_ID | "task_not_found" | "task_expired"
        )
}

fn classify_quota_failure(status: u16, body: &[u8]) -> QuotaRefreshFailure {
    if is_agent_identity_task_invalid_response(status, body) {
        return QuotaRefreshFailure::new(error_codes::INVALID_TASK_ID, false)
            .with_http_status(status);
    }
    crate::quota::classify_quota_http_failure(status, body)
}

fn bearer_authorization(access_token: &str) -> Result<HeaderValue, QuotaRefreshFailure> {
    if !valid_access_token(access_token) {
        return Err(QuotaRefreshFailure::new(
            error_codes::INVALID_ACCESS_TOKEN,
            false,
        ));
    }
    shared_bearer_authorization(access_token)
        .map_err(|_| QuotaRefreshFailure::new(error_codes::INVALID_ACCESS_TOKEN, false))
}

#[derive(Clone, Debug, PartialEq)]
pub enum QuotaRefreshOutcome {
    Updated(Box<QuotaRefreshResult>),
    Failed {
        failure: QuotaRefreshFailure,
        subscription: Subscription,
    },
}

mod parse;
pub use parse::parse_codex_usage;

#[cfg(test)]
mod tests;
