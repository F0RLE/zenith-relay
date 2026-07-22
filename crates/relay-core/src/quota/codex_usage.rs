use super::{
    merge_subscription_metadata, CodexSubscriptionClient, QuotaAdapterCapabilities,
    QuotaRefreshData, QuotaRefreshFailure, QuotaWindowInput, QuotaWindowKind, ResetTime,
    Subscription, SubscriptionInput, SupplementalQuotaWindowInput,
};
use crate::{accounts::CodexIdentityEnvelope, ProxyConfig};
use futures_util::StreamExt;
use reqwest::{
    header::{HeaderValue, ACCEPT, AUTHORIZATION},
    redirect::Policy,
};
use serde::Deserialize;
use std::{collections::BTreeSet, time::Duration};
use url::Url;

pub const CODEX_QUOTA_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";
#[cfg(test)]
const ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";
const MAX_ACCOUNT_ID_BYTES: usize = 512;
const MAX_ACCESS_TOKEN_BYTES: usize = 64 * 1024;
const MAX_QUOTA_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_ADDITIONAL_LIMITS: usize = 15;

#[derive(Clone)]
pub struct CodexQuotaClient {
    http: reqwest::Client,
    usage_endpoint: Url,
    subscription: CodexSubscriptionClient,
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
            .map_err(|_| QuotaRefreshFailure::new("invalid_configuration", false))?;
        Self::with_endpoint_proxy_and_timeout(usage_endpoint, proxy, request_timeout)
    }

    #[cfg(test)]
    fn with_endpoint(usage_endpoint: Url) -> Result<Self, QuotaRefreshFailure> {
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
        .map_err(|_| QuotaRefreshFailure::new("invalid_configuration", false))?;
        let subscription = CodexSubscriptionClient::new(http.clone())?;
        Ok(Self {
            http,
            usage_endpoint,
            subscription,
        })
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
        match self
            .refresh_data_with_subscription(
                access_token,
                chatgpt_account_id,
                now_ms,
                previous_subscription,
                refresh_subscription,
            )
            .await
        {
            Ok(data) => QuotaRefreshOutcome::Updated(data),
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
    ) -> Result<CodexQuotaRefreshData, QuotaRefreshFailure> {
        validate_access_token(access_token)?;
        if chatgpt_account_id.is_empty() || chatgpt_account_id.len() > MAX_ACCOUNT_ID_BYTES {
            return Err(QuotaRefreshFailure::new(
                "invalid_chatgpt_account_id",
                false,
            ));
        }
        let identity = CodexIdentityEnvelope::standard(chatgpt_account_id)
            .map_err(|_| QuotaRefreshFailure::new("invalid_chatgpt_account_id", false))?;
        let authorization = HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|_| QuotaRefreshFailure::new("invalid_access_token", false))?;
        let response = identity
            .apply(
                self.http
                    .get(self.usage_endpoint.clone())
                    .header(AUTHORIZATION, authorization)
                    .header(ACCEPT, "application/json"),
            )
            .send()
            .await
            .map_err(|_| QuotaRefreshFailure::new("quota_transport", true))?;
        let status = response.status();
        let body = collect_limited(response).await?;
        if !status.is_success() {
            return Err(super::classify_quota_http_failure(status.as_u16(), &body));
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
    ) -> Result<CodexQuotaRefreshData, QuotaRefreshFailure> {
        let mut data = self
            .refresh_data(access_token, chatgpt_account_id, now_ms)
            .await?;
        data.quota
            .preserve_subscription_metadata(previous_subscription);
        if refresh_subscription {
            let metadata = self
                .subscription
                .fetch(access_token, chatgpt_account_id, now_ms)
                .await;
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
                Ok(metadata) => {
                    merge_subscription_metadata(
                        &mut input.plan_type,
                        &mut input.active_until_ms,
                        metadata,
                    );
                    input.observed_at_ms = now_ms;
                }
                Err(_) => {
                    input.observed_at_ms = previous_subscription.updated_at_ms.unwrap_or_default();
                }
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

#[derive(Clone, Debug, PartialEq)]
pub enum QuotaRefreshOutcome {
    Updated(CodexQuotaRefreshData),
    Failed {
        failure: QuotaRefreshFailure,
        subscription: Subscription,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct CodexQuotaRefreshData {
    pub quota: QuotaRefreshData,
    pub allowed: Option<bool>,
    pub limit_reached: Option<bool>,
}

async fn collect_limited(response: reqwest::Response) -> Result<Vec<u8>, QuotaRefreshFailure> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| QuotaRefreshFailure::new("quota_transport", true))?;
        if body.len().saturating_add(chunk.len()) > MAX_QUOTA_RESPONSE_BYTES {
            return Err(QuotaRefreshFailure::new("quota_response_too_large", false));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn validate_access_token(access_token: &str) -> Result<(), QuotaRefreshFailure> {
    if access_token.is_empty()
        || access_token.len() > MAX_ACCESS_TOKEN_BYTES
        || access_token.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(QuotaRefreshFailure::new("invalid_access_token", false))
    } else {
        Ok(())
    }
}

#[derive(Deserialize)]
struct UsagePayload {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<RateLimitStatus>,
    #[serde(default)]
    code_review_rate_limit: Option<SupplementalRateLimitStatus>,
    #[serde(default)]
    additional_rate_limits: Option<Vec<AdditionalRateLimitStatus>>,
    #[serde(default)]
    rate_limit_reset_credits: Option<ResetCreditsSummary>,
    #[serde(default)]
    rate_limit_reached_type: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RateLimitStatus {
    #[serde(default)]
    allowed: Option<bool>,
    #[serde(default)]
    limit_reached: Option<bool>,
    #[serde(default)]
    primary_window: Option<RateLimitWindow>,
    #[serde(default)]
    secondary_window: Option<RateLimitWindow>,
}

#[derive(Clone, Deserialize)]
struct RateLimitWindow {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_after_seconds: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
}

#[derive(Deserialize)]
struct SupplementalRateLimitStatus {
    #[serde(default)]
    primary_window: Option<RateLimitWindow>,
    #[serde(default)]
    secondary_window: Option<RateLimitWindow>,
}

#[derive(Deserialize)]
struct AdditionalRateLimitStatus {
    #[serde(default)]
    limit_name: Option<String>,
    #[serde(default)]
    metered_feature: Option<String>,
    #[serde(default)]
    rate_limit: Option<SupplementalRateLimitStatus>,
}

#[derive(Deserialize)]
struct ResetCreditsSummary {
    available_count: i64,
}

pub fn parse_codex_usage(
    body: &[u8],
    observed_at_ms: u64,
) -> Result<CodexQuotaRefreshData, QuotaRefreshFailure> {
    let payload: UsagePayload = serde_json::from_slice(body)
        .map_err(|_| QuotaRefreshFailure::new("quota_invalid_response", false))?;
    let supplemental = collect_supplemental_windows(&payload, observed_at_ms);
    let explicit_limit_reached = payload.rate_limit_reached_type.is_some();
    let (primary, secondary, allowed, limit_reached) = match payload.rate_limit {
        Some(rate_limit) => (
            rate_limit
                .primary_window
                .map(|window| map_window(window, QuotaWindowKind::Primary, observed_at_ms))
                .transpose()?,
            rate_limit
                .secondary_window
                .map(|window| map_window(window, QuotaWindowKind::Secondary, observed_at_ms))
                .transpose()?,
            rate_limit.allowed,
            rate_limit.limit_reached,
        ),
        None => (None, None, None, None),
    };
    let limit_reached = explicit_limit_reached.then_some(true).or(limit_reached);
    let subscription = payload
        .plan_type
        .as_deref()
        .and_then(safe_label)
        .map(|plan_type| SubscriptionInput {
            plan_type: Some(plan_type),
            active_until_ms: None,
            forbidden: false,
            observed_at_ms,
        });
    Ok(CodexQuotaRefreshData {
        quota: QuotaRefreshData {
            primary,
            secondary,
            supplemental,
            limit_reached: limit_reached == Some(true),
            subscription,
            reset_credits_available: payload
                .rate_limit_reset_credits
                .and_then(|credits| u32::try_from(credits.available_count).ok()),
            observed_at_ms,
        },
        allowed,
        limit_reached,
    })
}

fn collect_supplemental_windows(
    payload: &UsagePayload,
    observed_at_ms: u64,
) -> Vec<SupplementalQuotaWindowInput> {
    let mut windows = Vec::new();
    if let Some(rate_limit) = payload.code_review_rate_limit.as_ref() {
        append_supplemental_windows(
            &mut windows,
            "code_review",
            "Code Review",
            rate_limit,
            observed_at_ms,
        );
    }
    for (index, entry) in payload
        .additional_rate_limits
        .as_deref()
        .unwrap_or_default()
        .iter()
        .take(MAX_ADDITIONAL_LIMITS)
        .enumerate()
    {
        if is_spark_limit(entry) {
            continue;
        }
        let Some(rate_limit) = entry.rate_limit.as_ref() else {
            continue;
        };
        let label = entry
            .limit_name
            .as_deref()
            .and_then(safe_display_label)
            .or_else(|| {
                entry
                    .metered_feature
                    .as_deref()
                    .and_then(safe_display_label)
            })
            .unwrap_or_else(|| "Additional quota".to_string());
        append_supplemental_windows(
            &mut windows,
            &format!("additional:{index}"),
            &label,
            rate_limit,
            observed_at_ms,
        );
    }
    windows
}

fn is_spark_limit(entry: &AdditionalRateLimitStatus) -> bool {
    entry
        .limit_name
        .as_deref()
        .into_iter()
        .chain(entry.metered_feature.as_deref())
        .any(|value| value.to_ascii_lowercase().contains("spark"))
}

fn append_supplemental_windows(
    output: &mut Vec<SupplementalQuotaWindowInput>,
    id_prefix: &str,
    label: &str,
    rate_limit: &SupplementalRateLimitStatus,
    observed_at_ms: u64,
) {
    for (kind, window) in [
        (QuotaWindowKind::Primary, rate_limit.primary_window.as_ref()),
        (
            QuotaWindowKind::Secondary,
            rate_limit.secondary_window.as_ref(),
        ),
    ] {
        let Some(window) = window else { continue };
        let kind_label = match kind {
            QuotaWindowKind::Primary => "primary",
            QuotaWindowKind::Secondary => "secondary",
        };
        let Ok(window) = map_window(window.clone(), kind, observed_at_ms) else {
            continue;
        };
        output.push(SupplementalQuotaWindowInput {
            id: format!("{id_prefix}:{kind_label}"),
            label: label.to_string(),
            window,
        });
    }
}

fn map_window(
    window: RateLimitWindow,
    kind: QuotaWindowKind,
    observed_at_ms: u64,
) -> Result<QuotaWindowInput, QuotaRefreshFailure> {
    let used_percent = window
        .used_percent
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
        .ok_or_else(|| QuotaRefreshFailure::new("quota_invalid_percentage", false))?;
    let reset = window
        .reset_at
        .filter(|value| *value > 0)
        .and_then(|value| u64::try_from(value).ok())
        .map(ResetTime::AbsoluteUnixSeconds)
        .or_else(|| {
            window
                .reset_after_seconds
                .filter(|value| *value >= 0)
                .and_then(|value| u64::try_from(value).ok())
                .map(ResetTime::RelativeSeconds)
        });
    Ok(QuotaWindowInput {
        kind,
        available_percent: Some(100.0 - used_percent),
        explicitly_full: None,
        reset,
        window_minutes: window
            .limit_window_seconds
            .filter(|seconds| *seconds > 0)
            .and_then(|seconds| u32::try_from((seconds.saturating_add(59)) / 60).ok()),
        provider_cycle_id: None,
        observed_at_ms,
    })
}

fn safe_label(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then(|| value.to_ascii_lowercase())
}

fn safe_display_label(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control))
        .then(|| value.split_whitespace().collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::{QuotaSnapshot, SubscriptionStatus};
    use axum::{
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::get,
        Json, Router,
    };
    use serde_json::json;

    #[test]
    fn usage_payload_has_one_normalized_shape() {
        let data = parse_codex_usage(
            br#"{
                "plan_type":"plus",
                "rate_limit":{
                    "allowed":true,
                    "limit_reached":false,
                    "primary_window":{"used_percent":25,"limit_window_seconds":18000,"reset_after_seconds":60},
                    "secondary_window":{"used_percent":0,"limit_window_seconds":604800,"reset_at":1700000300}
                },
                "code_review_rate_limit":{"primary_window":{"used_percent":40,"limit_window_seconds":18000}},
                "additional_rate_limits":[{
                    "metered_feature":" GPT-5   Priority ",
                    "rate_limit":{"secondary_window":{"used_percent":10,"limit_window_seconds":604800}}
                },{
                    "limit_name":"GPT-5.3 Codex Spark",
                    "rate_limit":{"primary_window":{"used_percent":50}}
                }],
                "rate_limit_reset_credits":{"available_count":2}
            }"#,
            1_000,
        )
        .unwrap();
        assert_eq!(data.allowed, Some(true));
        assert_eq!(data.limit_reached, Some(false));
        let (quota, subscription) = data.quota.normalize(&QuotaSnapshot::default()).unwrap();
        assert_eq!(quota.primary.unwrap().available_basis_points, Some(7_500));
        assert_eq!(
            quota.secondary.unwrap().available_basis_points,
            Some(10_000)
        );
        assert_eq!(quota.supplemental.len(), 2);
        assert_eq!(quota.supplemental[1].label, "GPT-5 Priority");
        assert_eq!(quota.reset_credits_available, Some(2));
        assert_eq!(subscription.unwrap().plan_type.as_deref(), Some("plus"));
    }

    #[test]
    fn explicit_limit_and_invalid_primary_are_unambiguous() {
        let limited = parse_codex_usage(
            br#"{
                "rate_limit":{"limit_reached":false,"primary_window":{"used_percent":98}},
                "rate_limit_reached_type":{"type":"rate_limit_reached"}
            }"#,
            1_000,
        )
        .unwrap();
        assert_eq!(limited.limit_reached, Some(true));
        assert!(limited.quota.limit_reached);

        assert_eq!(
            parse_codex_usage(
                br#"{"rate_limit":{"primary_window":{"used_percent":101}}}"#,
                1_000,
            )
            .unwrap_err()
            .code,
            "quota_invalid_percentage"
        );
    }

    #[tokio::test]
    async fn shared_client_sends_safe_headers_and_refreshes_subscription() {
        let router = Router::new()
            .route("/backend-api/wham/usage", get(successful_usage))
            .route(
                "/backend-api/accounts/check/v4-2023-04-27",
                get(subscription_account_check),
            )
            .route("/backend-api/subscriptions", get(subscription_status));
        let (usage_endpoint, server) = spawn(router).await;
        let http = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .unwrap();
        let client = CodexQuotaClient {
            subscription: CodexSubscriptionClient::with_endpoints(
                http.clone(),
                usage_endpoint
                    .join("/backend-api/accounts/check/v4-2023-04-27")
                    .unwrap(),
                usage_endpoint.join("/backend-api/subscriptions").unwrap(),
            )
            .unwrap(),
            http,
            usage_endpoint,
        };

        let QuotaRefreshOutcome::Updated(data) = client
            .refresh_quota(
                "access-secret",
                "account-123",
                1_700_000_000_000,
                &Subscription::default(),
                true,
            )
            .await
        else {
            panic!("expected updated quota");
        };
        let (quota, subscription) = data.quota.normalize(&QuotaSnapshot::default()).unwrap();
        assert_eq!(quota.primary.unwrap().available_basis_points, Some(7_500));
        let subscription = subscription.unwrap();
        assert_eq!(subscription.plan_type.as_deref(), Some("business"));
        assert_eq!(subscription.active_until_ms, Some(1_791_590_400_000));
        server.abort();
    }

    #[tokio::test]
    async fn quota_refresh_keeps_a_known_subscription_expiry() {
        let router = Router::new()
            .route("/backend-api/wham/usage", get(successful_usage))
            .route(
                "/backend-api/accounts/check/v4-2023-04-27",
                get(subscription_account_check),
            )
            .route(
                "/backend-api/subscriptions",
                get(subscription_status_without_expiry),
            );
        let (usage_endpoint, server) = spawn(router).await;
        let http = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .unwrap();
        let client = CodexQuotaClient {
            subscription: CodexSubscriptionClient::with_endpoints(
                http.clone(),
                usage_endpoint
                    .join("/backend-api/accounts/check/v4-2023-04-27")
                    .unwrap(),
                usage_endpoint.join("/backend-api/subscriptions").unwrap(),
            )
            .unwrap(),
            http,
            usage_endpoint,
        };
        let previous = Subscription {
            plan_type: Some("team".into()),
            active_until_ms: Some(1_800_000_000_000),
            status: SubscriptionStatus::Active,
            updated_at_ms: Some(99),
        };

        let QuotaRefreshOutcome::Updated(data) = client
            .refresh_quota(
                "access-secret",
                "account-123",
                1_700_000_000_000,
                &previous,
                true,
            )
            .await
        else {
            panic!("expected updated quota");
        };
        let (_, subscription) = data.quota.normalize(&QuotaSnapshot::default()).unwrap();
        let subscription = subscription.unwrap();
        assert_eq!(subscription.active_until_ms, previous.active_until_ms);
        assert_eq!(subscription.updated_at_ms, Some(1_700_000_000_000));
        server.abort();
    }

    #[tokio::test]
    async fn shared_client_preserves_last_subscription_on_safe_failure() {
        let previous = Subscription {
            plan_type: Some("plus".into()),
            active_until_ms: None,
            status: SubscriptionStatus::Active,
            updated_at_ms: Some(99),
        };
        let (endpoint, server) =
            spawn(Router::new().route("/backend-api/wham/usage", get(upstream_failure))).await;
        let result = CodexQuotaClient::with_endpoint(endpoint)
            .unwrap()
            .refresh_quota("access-secret", "account-123", 100, &previous, false)
            .await;
        let QuotaRefreshOutcome::Failed {
            failure,
            subscription,
        } = result
        else {
            panic!("expected failed quota refresh");
        };
        assert_eq!(failure.code, "quota_upstream");
        assert_eq!(subscription, previous);
        assert!(!format!("{failure:?}").contains("provider-body-secret"));
        server.abort();
    }

    async fn successful_usage(headers: HeaderMap) -> impl IntoResponse {
        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer access-secret")
        );
        assert_eq!(
            headers
                .get(ACCOUNT_ID_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("account-123")
        );
        Json(json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 25,
                    "limit_window_seconds": 18_000,
                    "reset_after_seconds": 1
                }
            }
        }))
    }

    async fn upstream_failure() -> impl IntoResponse {
        (StatusCode::BAD_GATEWAY, "provider-body-secret")
    }

    async fn subscription_account_check() -> impl IntoResponse {
        Json(json!({
            "accounts": [{
                "account": {"id": "account-123"},
                "entitlement": {"subscription_plan": "business"}
            }]
        }))
    }

    async fn subscription_status() -> impl IntoResponse {
        Json(json!({
            "subscription_plan": "business",
            "active_until": "2026-10-10T00:00:00Z"
        }))
    }

    async fn subscription_status_without_expiry() -> impl IntoResponse {
        Json(json!({"subscription_plan": "business"}))
    }

    async fn spawn(router: Router) -> (Url, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (
            Url::parse(&format!("http://{address}/backend-api/wham/usage")).unwrap(),
            server,
        )
    }
}
