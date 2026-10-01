use super::super::{collect_response_body, valid_access_token, ResponseBodyError};
use super::CodexSubscriptionMetadata;
use crate::error_codes;
use crate::quota::QuotaRefreshFailure;
use crate::scheduler::refresh::http::{HttpClass, ManagementHttpScope};
use crate::{is_http_endpoint, url_has_userinfo};
use chrono::Local;
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, REFERER, USER_AGENT},
    Client, StatusCode,
};
use serde_json::Value;
use url::Url;

pub const CODEX_ACCOUNTS_CHECK_ENDPOINT: &str =
    "https://chatgpt.com/backend-api/accounts/check/v4-2023-04-27";
pub const CODEX_SUBSCRIPTIONS_ENDPOINT: &str = "https://chatgpt.com/backend-api/subscriptions";
#[derive(Clone)]
pub struct CodexSubscriptionClient {
    http: Client,
    accounts_check_endpoint: Url,
    subscriptions_endpoint: Url,
    scope: ManagementHttpScope,
}

impl CodexSubscriptionClient {
    pub fn new(http: Client) -> Result<Self, QuotaRefreshFailure> {
        Self::with_endpoints(
            http,
            Url::parse(CODEX_ACCOUNTS_CHECK_ENDPOINT)
                .map_err(|_| super::failure(error_codes::SUBSCRIPTION_CONFIGURATION, false))?,
            Url::parse(CODEX_SUBSCRIPTIONS_ENDPOINT)
                .map_err(|_| super::failure(error_codes::SUBSCRIPTION_CONFIGURATION, false))?,
        )
    }

    pub fn with_endpoints(
        http: Client,
        accounts_check_endpoint: Url,
        subscriptions_endpoint: Url,
    ) -> Result<Self, QuotaRefreshFailure> {
        for endpoint in [&accounts_check_endpoint, &subscriptions_endpoint] {
            if !is_http_endpoint(endpoint) || url_has_userinfo(endpoint) {
                return Err(super::failure(
                    error_codes::SUBSCRIPTION_CONFIGURATION,
                    false,
                ));
            }
        }
        Ok(Self {
            http,
            accounts_check_endpoint,
            subscriptions_endpoint,
            scope: ManagementHttpScope::default(),
        })
    }

    pub fn with_http_scope(mut self, scope: ManagementHttpScope) -> Self {
        self.scope = scope;
        self
    }

    pub async fn fetch(
        &self,
        access_token: &str,
        preferred_account_id: &str,
        now_ms: u64,
    ) -> Result<CodexSubscriptionMetadata, QuotaRefreshFailure> {
        let authorization = authorization_header(access_token)?;
        self.fetch_authorized(authorization, preferred_account_id, now_ms)
            .await
    }

    pub async fn fetch_authorized(
        &self,
        authorization: HeaderValue,
        preferred_account_id: &str,
        now_ms: u64,
    ) -> Result<CodexSubscriptionMetadata, QuotaRefreshFailure> {
        let first = self
            .fetch_authorized_once(authorization.clone(), preferred_account_id, now_ms)
            .await;
        match first {
            // A provider floor belongs to the first attempt. Never make the
            // built-in retry before a Retry-After has elapsed.
            Err(error) if error.retryable && error.retry_after_ms().is_none() => {
                self.fetch_authorized_once(authorization, preferred_account_id, now_ms)
                    .await
            }
            result => result,
        }
    }

    async fn fetch_authorized_once(
        &self,
        authorization: HeaderValue,
        preferred_account_id: &str,
        now_ms: u64,
    ) -> Result<CodexSubscriptionMetadata, QuotaRefreshFailure> {
        let preferred_account_id = validate_account_id(preferred_account_id)?;
        let (response, permit) = self
            .scope
            .send(
                &self.http,
                self.http
                    .get(self.accounts_check_endpoint.clone())
                    .query(&[(
                        "timezone_offset_min",
                        -(Local::now().offset().local_minus_utc() / 60),
                    )])
                    .headers(subscription_headers(
                        authorization.clone(),
                        "/backend-api/accounts/check/v4-2023-04-27",
                    )?),
                HttpClass::Ordinary,
            )
            .await
            .map_err(|_| super::failure(error_codes::SUBSCRIPTION_TRANSPORT, true))?;
        let payload = response_json(response).await?;
        drop(permit);
        let mut metadata = super::parse::parse_accounts_check(&payload, preferred_account_id)?;
        if metadata
            .active_until_ms
            .is_some_and(|active_until_ms| active_until_ms > now_ms)
        {
            return Ok(metadata);
        }

        let account_id = metadata
            .account_id
            .as_deref()
            .unwrap_or(preferred_account_id);
        let (response, permit) = self
            .scope
            .send(
                &self.http,
                self.http
                    .get(self.subscriptions_endpoint.clone())
                    .query(&[("account_id", account_id)])
                    .headers(subscription_headers(
                        authorization,
                        "/backend-api/subscriptions",
                    )?),
                HttpClass::Ordinary,
            )
            .await
            .map_err(|_| super::failure(error_codes::SUBSCRIPTION_TRANSPORT, true))?;
        let payload = response_json(response).await?;
        drop(permit);
        let fallback = super::parse::parse_subscriptions(&payload, account_id);
        metadata.account_id = fallback.account_id.or(metadata.account_id);
        metadata.plan_type = fallback.plan_type.or(metadata.plan_type);
        metadata.active_until_ms = fallback.active_until_ms.or(metadata.active_until_ms);
        Ok(metadata)
    }
}

const MAX_RESPONSE_BYTES: usize = 256 * 1024;
// These ChatGPT Web endpoints require browser-shaped headers, not the Codex API identity envelope.
pub(super) const CHATGPT_WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36";

fn authorization_header(access_token: &str) -> Result<HeaderValue, QuotaRefreshFailure> {
    if !valid_access_token(access_token) {
        return Err(super::failure(
            error_codes::SUBSCRIPTION_ACCESS_TOKEN_INVALID,
            false,
        ));
    }
    HeaderValue::from_str(&format!("Bearer {access_token}"))
        .map_err(|_| super::failure(error_codes::SUBSCRIPTION_ACCESS_TOKEN_INVALID, false))
}

fn validate_account_id(value: &str) -> Result<&str, QuotaRefreshFailure> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > super::MAX_ACCOUNT_ID_BYTES
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(super::failure(
            error_codes::SUBSCRIPTION_ACCOUNT_ID_INVALID,
            false,
        ))
    } else {
        Ok(value)
    }
}

fn subscription_headers(
    authorization: HeaderValue,
    target_path: &str,
) -> Result<HeaderMap, QuotaRefreshFailure> {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, authorization);
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(REFERER, HeaderValue::from_static("https://chatgpt.com/"));
    headers.insert(USER_AGENT, HeaderValue::from_static(CHATGPT_WEB_USER_AGENT));
    let target = HeaderValue::from_str(target_path)
        .map_err(|_| super::failure(error_codes::SUBSCRIPTION_CONFIGURATION, false))?;
    headers.insert("x-openai-target-path", target.clone());
    headers.insert("x-openai-target-route", target);
    Ok(headers)
}

async fn response_json(response: reqwest::Response) -> Result<Value, QuotaRefreshFailure> {
    let status = response.status();
    let retry_after_ms =
        crate::transport::retry_after_ms(response.headers(), std::time::SystemTime::now());
    let body = collect_response_body(response, MAX_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            ResponseBodyError::Transport => {
                super::failure(error_codes::SUBSCRIPTION_TRANSPORT, true)
            }
            ResponseBodyError::TooLarge => {
                super::failure(error_codes::SUBSCRIPTION_RESPONSE_TOO_LARGE, false)
            }
        })
        .map_err(|failure| failure.with_retry_after(retry_after_ms))?;
    if !status.is_success() {
        return Err(http_failure(status).with_retry_after(retry_after_ms));
    }
    serde_json::from_slice(&body)
        .map_err(|_| super::failure(error_codes::SUBSCRIPTION_INVALID_RESPONSE, false))
}

fn http_failure(status: StatusCode) -> QuotaRefreshFailure {
    match status.as_u16() {
        401 => super::failure(error_codes::SUBSCRIPTION_UNAUTHORIZED, false),
        403 => super::failure(error_codes::SUBSCRIPTION_FORBIDDEN, false),
        429 => super::failure(error_codes::SUBSCRIPTION_RATE_LIMITED, true),
        _ if status.is_server_error() => super::failure(error_codes::SUBSCRIPTION_UPSTREAM, true),
        _ => super::failure(error_codes::SUBSCRIPTION_HTTP_STATUS, false),
    }
}
