use super::super::{
    ResponseBodyError, bearer_authorization, collect_response_body, valid_access_token,
};
use super::{CodexSubscriptionMetadata, CodexSubscriptionRead};
use crate::error_codes;
use crate::quota::{QuotaRefreshFailure, QuotaTransportFailure};
use crate::scheduler::refresh::http::{
    HttpAdmissionError, HttpClass, HttpSendError, ManagementHttpScope,
};
use crate::sources::{is_http_endpoint, is_loopback_url, url_has_userinfo};
use chrono::Local;
use reqwest::{
    Client, StatusCode,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, REFERER, USER_AGENT},
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
            if !is_http_endpoint(endpoint)
                || url_has_userinfo(endpoint)
                || (endpoint.scheme() == "http" && !is_loopback_url(endpoint))
            {
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

    /// Header-only observations without account credentials or generation.
    pub(super) async fn probe_anonymous(&self) -> Vec<super::SubscriptionEndpointProbe> {
        super::probe::probe(
            &self.http,
            &self.scope,
            &self.accounts_check_endpoint,
            &self.subscriptions_endpoint,
        )
        .await
    }

    pub(super) async fn probe_authorization(
        &self,
        authorization: HeaderValue,
        preferred_account_id: &str,
    ) -> Result<Vec<super::SubscriptionAuthorizationProbe>, QuotaRefreshFailure> {
        let metadata = self
            .fetch_account_check_once(authorization.clone(), preferred_account_id)
            .await?;
        super::probe::compare_authorization(
            &self.http,
            &self.scope,
            &self.subscriptions_endpoint,
            authorization,
            metadata
                .account_id
                .as_deref()
                .unwrap_or(preferred_account_id),
        )
        .await
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
        let read = self
            .read_authorized(authorization, preferred_account_id, now_ms)
            .await?;
        match read.fallback_failure {
            Some(failure) => Err(failure),
            None => Ok(read.metadata),
        }
    }

    /// Retains authenticated account metadata without hiding an optional lookup failure.
    pub async fn read_authorized(
        &self,
        authorization: HeaderValue,
        preferred_account_id: &str,
        now_ms: u64,
    ) -> Result<CodexSubscriptionRead, QuotaRefreshFailure> {
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
    ) -> Result<CodexSubscriptionRead, QuotaRefreshFailure> {
        let mut metadata = self
            .fetch_account_check_once(authorization.clone(), preferred_account_id)
            .await?;
        if metadata
            .active_until_ms
            .is_some_and(|active_until_ms| active_until_ms > now_ms)
        {
            return Ok(CodexSubscriptionRead {
                metadata,
                fallback_failure: None,
            });
        }

        let account_id = metadata
            .account_id
            .as_deref()
            .unwrap_or(preferred_account_id);
        let fallback = self
            .fetch_subscriptions_once(authorization.clone(), account_id)
            .await;
        let fallback = match fallback {
            Err(failure) if failure.retryable && failure.retry_after_ms().is_none() => {
                self.fetch_subscriptions_once(authorization, account_id)
                    .await
            }
            result => result,
        };
        let fallback_failure = match fallback {
            Ok(fallback) => {
                metadata.plan_type = fallback.plan_type.or(metadata.plan_type);
                metadata.active_until_ms = fallback.active_until_ms.or(metadata.active_until_ms);
                None
            }
            Err(failure) => Some(failure),
        };
        Ok(CodexSubscriptionRead {
            metadata,
            fallback_failure,
        })
    }

    async fn fetch_account_check_once(
        &self,
        authorization: HeaderValue,
        preferred_account_id: &str,
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
            .map_err(|error| send_failure(error, "accounts_check"))?;
        let subscription_check_payload = response_json(response, "accounts_check").await?;
        drop(permit);
        super::parse::parse_accounts_check(&subscription_check_payload, preferred_account_id)
            .map_err(|failure| failure.with_endpoint_stage("accounts_check"))
    }

    async fn fetch_subscriptions_once(
        &self,
        authorization: HeaderValue,
        account_id: &str,
    ) -> Result<CodexSubscriptionMetadata, QuotaRefreshFailure> {
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
            .map_err(|error| send_failure(error, "subscriptions"))?;
        let subscription_list_payload = response_json(response, "subscriptions").await?;
        drop(permit);
        super::parse::parse_subscriptions(&subscription_list_payload, account_id)
            .map_err(|failure| failure.with_endpoint_stage("subscriptions"))
    }
}

const MAX_RESPONSE_BYTES: usize = 256 * 1024;

pub(super) fn send_failure(
    error: HttpSendError,
    endpoint_stage: &'static str,
) -> QuotaRefreshFailure {
    let (code, retryable, reason) = match error {
        HttpSendError::Stale => (
            error_codes::SUBSCRIPTION_TRANSPORT,
            false,
            QuotaTransportFailure::StaleScope,
        ),
        HttpSendError::Admission(HttpAdmissionError::InvalidOrigin) => (
            error_codes::SUBSCRIPTION_CONFIGURATION,
            false,
            QuotaTransportFailure::InvalidOrigin,
        ),
        HttpSendError::Admission(HttpAdmissionError::Full) => (
            error_codes::SUBSCRIPTION_TRANSPORT,
            true,
            QuotaTransportFailure::AdmissionFull,
        ),
        HttpSendError::Admission(HttpAdmissionError::TimedOut) => (
            error_codes::SUBSCRIPTION_TRANSPORT,
            true,
            QuotaTransportFailure::AdmissionTimeout,
        ),
        HttpSendError::Transport { timeout: true } => (
            error_codes::SUBSCRIPTION_TRANSPORT,
            true,
            QuotaTransportFailure::Timeout,
        ),
        HttpSendError::Transport { timeout: false } => (
            error_codes::SUBSCRIPTION_TRANSPORT,
            true,
            QuotaTransportFailure::Network,
        ),
    };
    super::failure(code, retryable)
        .with_endpoint_stage(endpoint_stage)
        .with_transport_failure(reason)
}

// Observed ChatGPT Web compatibility headers do not establish Codex OAuth support.
pub(super) const CHATGPT_WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36";

fn authorization_header(access_token: &str) -> Result<HeaderValue, QuotaRefreshFailure> {
    if !valid_access_token(access_token) {
        return Err(super::failure(
            error_codes::SUBSCRIPTION_ACCESS_TOKEN_INVALID,
            false,
        ));
    }
    bearer_authorization(access_token)
        .map_err(|_| super::failure(error_codes::SUBSCRIPTION_ACCESS_TOKEN_INVALID, false))
}

fn validate_account_id(account_id: &str) -> Result<&str, QuotaRefreshFailure> {
    let account_id = account_id.trim();
    if account_id.is_empty()
        || account_id.len() > super::MAX_ACCOUNT_ID_BYTES
        || account_id.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(super::failure(
            error_codes::SUBSCRIPTION_ACCOUNT_ID_INVALID,
            false,
        ))
    } else {
        Ok(account_id)
    }
}

pub(super) fn subscription_headers(
    mut authorization: HeaderValue,
    target_path: &str,
) -> Result<HeaderMap, QuotaRefreshFailure> {
    authorization.set_sensitive(true);
    let mut headers = subscription_public_headers(target_path)?;
    headers.insert(AUTHORIZATION, authorization);
    Ok(headers)
}

pub(super) fn subscription_public_headers(
    target_path: &str,
) -> Result<HeaderMap, QuotaRefreshFailure> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(REFERER, HeaderValue::from_static("https://chatgpt.com/"));
    headers.insert(USER_AGENT, HeaderValue::from_static(CHATGPT_WEB_USER_AGENT));
    let target = HeaderValue::from_str(target_path)
        .map_err(|_| super::failure(error_codes::SUBSCRIPTION_CONFIGURATION, false))?;
    headers.insert("x-openai-target-path", target.clone());
    headers.insert("x-openai-target-route", target);
    Ok(headers)
}

pub(super) fn response_format(headers: &HeaderMap) -> &'static str {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match content_type.as_str() {
        "application/json" => "json",
        "text/html" | "application/xhtml+xml" => "html",
        "text/plain" => "text",
        "" => "unknown",
        value if value.starts_with("application/") && value.ends_with("+json") => "json",
        _ => "other",
    }
}

async fn response_json(
    response: reqwest::Response,
    endpoint_stage: &'static str,
) -> Result<Value, QuotaRefreshFailure> {
    let status = response.status();
    // Only fixed categories leave this boundary, never header/body contents.
    let response_format = response_format(response.headers());
    // HTML alone does not establish who rejected the request. Retain only
    // Cloudflare's documented challenge signal, never arbitrary header values.
    let response_mitigation = response
        .headers()
        .get("cf-mitigated")
        .is_some_and(|value| value == "challenge")
        .then_some("cloudflare_challenge");
    let retry_after_ms =
        crate::transport::retry_after_ms(response.headers(), std::time::SystemTime::now());
    let cf_ray = response
        .headers()
        .get("cf-ray")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 64)
        .map(str::to_owned);
    let with_context = |failure: QuotaRefreshFailure| {
        failure
            .with_http_status(status.as_u16())
            .with_retry_after(retry_after_ms)
            .with_endpoint_stage(endpoint_stage)
            .with_response_format(response_format)
            .with_response_mitigation(response_mitigation)
            .with_cf_ray(cf_ray.as_deref())
    };
    // Error bodies are unused. Their truncation or timeout must not turn a
    // definitive rejection into a retryable transport failure.
    if !status.is_success() {
        return Err(with_context(http_failure(status)));
    }
    let subscription_response_body = collect_response_body(response, MAX_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            ResponseBodyError::Timeout => super::failure(error_codes::SUBSCRIPTION_TRANSPORT, true)
                .with_transport_failure(QuotaTransportFailure::BodyTimeout),
            ResponseBodyError::Transport => {
                super::failure(error_codes::SUBSCRIPTION_TRANSPORT, true)
                    .with_transport_failure(QuotaTransportFailure::BodyTransport)
            }
            ResponseBodyError::TooLarge => {
                super::failure(error_codes::SUBSCRIPTION_RESPONSE_TOO_LARGE, false)
            }
        })
        .map_err(with_context)?;
    serde_json::from_slice(&subscription_response_body).map_err(|_| {
        with_context(super::failure(
            error_codes::SUBSCRIPTION_INVALID_RESPONSE,
            false,
        ))
    })
}

fn http_failure(status: StatusCode) -> QuotaRefreshFailure {
    match status.as_u16() {
        401 => super::failure(error_codes::SUBSCRIPTION_UNAUTHORIZED, false),
        403 => super::failure(error_codes::SUBSCRIPTION_FORBIDDEN, false),
        429 => super::failure(error_codes::SUBSCRIPTION_RATE_LIMITED, true),
        _ if status.is_server_error() => super::failure(error_codes::SUBSCRIPTION_UPSTREAM, true),
        _ => super::failure(error_codes::SUBSCRIPTION_HTTP_STATUS, false),
    }
    .with_http_status(status.as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_authorization_redacts_debug_without_changing_wire_bytes() {
        let token = "synthetic-subscription-secret";
        let authorization = authorization_header(token).unwrap();
        assert_eq!(authorization.to_str().unwrap(), format!("Bearer {token}"));
        assert!(authorization.is_sensitive());
        assert!(
            !format!("{authorization:?}").contains(token),
            "provider tokens must not appear in authorization Debug output"
        );
    }

    #[test]
    fn subscription_requests_redact_even_unmarked_external_authorization() {
        let token = "synthetic-external-subscription-secret";
        let authorization = HeaderValue::from_str(&format!("Bearer {token}")).unwrap();
        assert!(!authorization.is_sensitive());
        let headers = subscription_headers(authorization, "/backend-api/subscriptions").unwrap();
        let request = Client::new()
            .get(CODEX_SUBSCRIPTIONS_ENDPOINT)
            .headers(headers)
            .build()
            .unwrap();
        let authorization = &request.headers()[AUTHORIZATION];
        assert_eq!(authorization.to_str().unwrap(), format!("Bearer {token}"));
        assert!(authorization.is_sensitive());
        assert!(!format!("{:?}", request.headers()).contains(token));
        assert_eq!(
            request.headers()["x-openai-target-path"],
            "/backend-api/subscriptions"
        );
        assert_eq!(
            request.headers()["x-openai-target-route"],
            "/backend-api/subscriptions"
        );
    }
}
