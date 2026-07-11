use super::oauth::{collect_limited, LimitedBodyError};
use reqwest::header::{HeaderValue, AUTHORIZATION};
use reqwest::redirect::Policy;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::time::Duration;
use url::Url;
use zenith_relay_core::quota::{
    QuotaAdapterCapabilities, QuotaRefreshData, QuotaRefreshFailure, QuotaWindowInput,
    QuotaWindowKind, ResetTime, Subscription, SubscriptionInput, SupplementalQuotaWindowInput,
};
use zenith_relay_core::ProxyConfig;

pub const CODEX_QUOTA_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";

const ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";
const MAX_ACCOUNT_ID_BYTES: usize = 512;
const MAX_ACCESS_TOKEN_BYTES: usize = 64 * 1024;
const MAX_QUOTA_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Clone)]
pub struct CodexQuotaClient {
    http: reqwest::Client,
    usage_endpoint: Url,
}

impl CodexQuotaClient {
    pub fn new() -> Result<Self, QuotaRefreshFailure> {
        Self::new_with_proxy(None)
    }

    pub fn new_with_proxy(proxy: Option<&ProxyConfig>) -> Result<Self, QuotaRefreshFailure> {
        let usage_endpoint = Url::parse(CODEX_QUOTA_ENDPOINT)
            .map_err(|_| QuotaRefreshFailure::new("invalid_configuration", false))?;
        Self::with_endpoint_and_proxy(usage_endpoint, proxy)
    }

    #[cfg(test)]
    fn with_endpoint(usage_endpoint: Url) -> Result<Self, QuotaRefreshFailure> {
        Self::with_endpoint_and_proxy(usage_endpoint, None)
    }

    fn with_endpoint_and_proxy(
        usage_endpoint: Url,
        proxy: Option<&ProxyConfig>,
    ) -> Result<Self, QuotaRefreshFailure> {
        let builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(20))
            .user_agent("Zenith Relay");
        let http = match proxy {
            Some(proxy) => proxy.apply(builder),
            None => builder,
        }
        .build()
        .map_err(|_| QuotaRefreshFailure::new("invalid_configuration", false))?;
        Ok(Self {
            http,
            usage_endpoint,
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
    ) -> QuotaRefreshOutcome {
        match self
            .refresh_data(access_token, chatgpt_account_id, now_ms)
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
        let account_id = HeaderValue::from_str(chatgpt_account_id)
            .map_err(|_| QuotaRefreshFailure::new("invalid_chatgpt_account_id", false))?;
        if chatgpt_account_id.is_empty() || chatgpt_account_id.len() > MAX_ACCOUNT_ID_BYTES {
            return Err(QuotaRefreshFailure::new(
                "invalid_chatgpt_account_id",
                false,
            ));
        }
        let authorization = HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|_| QuotaRefreshFailure::new("invalid_access_token", false))?;
        let response = self
            .http
            .get(self.usage_endpoint.clone())
            .header(AUTHORIZATION, authorization)
            .header(ACCOUNT_ID_HEADER, account_id)
            .send()
            .await
            .map_err(|_| QuotaRefreshFailure::new("quota_transport", true))?;
        let status = response.status();
        let body = collect_limited(response, MAX_QUOTA_RESPONSE_BYTES)
            .await
            .map_err(|error| match error {
                LimitedBodyError::Transport => QuotaRefreshFailure::new("quota_transport", true),
                LimitedBodyError::TooLarge => {
                    QuotaRefreshFailure::new("quota_response_too_large", false)
                }
            })?;
        if !status.is_success() {
            let (code, retryable) = match status.as_u16() {
                401 => ("quota_unauthorized", false),
                403 => ("quota_forbidden", false),
                429 => ("quota_rate_limited", true),
                _ if status.is_server_error() => ("quota_upstream", true),
                _ => ("quota_http_status", false),
            };
            return Err(QuotaRefreshFailure::new(code, retryable));
        }

        parse_usage_payload(&body, now_ms)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CodexQuotaRefreshData {
    pub quota: QuotaRefreshData,
    pub allowed: Option<bool>,
    pub limit_reached: Option<bool>,
    pub rate_limit_reached_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum QuotaRefreshOutcome {
    Updated(CodexQuotaRefreshData),
    Failed {
        failure: QuotaRefreshFailure,
        subscription: Subscription,
    },
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
    rate_limit_reached_type: Option<RateLimitReachedType>,
}

#[derive(Deserialize)]
struct RateLimitStatus {
    allowed: bool,
    limit_reached: bool,
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

#[derive(Deserialize)]
struct RateLimitReachedType {
    #[serde(rename = "type")]
    kind: String,
}

fn parse_usage_payload(
    body: &[u8],
    now_ms: u64,
) -> Result<CodexQuotaRefreshData, QuotaRefreshFailure> {
    let payload: UsagePayload = serde_json::from_slice(body)
        .map_err(|_| QuotaRefreshFailure::new("quota_invalid_response", false))?;
    let supplemental = collect_supplemental_windows(&payload, now_ms);
    let (primary, secondary, allowed, limit_reached) = match payload.rate_limit {
        Some(rate_limit) => (
            rate_limit
                .primary_window
                .map(|window| map_window(window, QuotaWindowKind::Primary, now_ms))
                .transpose()?,
            rate_limit
                .secondary_window
                .map(|window| map_window(window, QuotaWindowKind::Secondary, now_ms))
                .transpose()?,
            Some(rate_limit.allowed),
            Some(rate_limit.limit_reached),
        ),
        None => (None, None, None, None),
    };
    let plan_type = payload.plan_type.and_then(|value| safe_label(&value));
    let subscription = plan_type.map(|plan_type| SubscriptionInput {
        plan_type: Some(plan_type),
        active_until_ms: None,
        forbidden: false,
        observed_at_ms: now_ms,
    });
    let reset_credits_available = payload
        .rate_limit_reset_credits
        .and_then(|credits| u32::try_from(credits.available_count).ok());

    Ok(CodexQuotaRefreshData {
        quota: QuotaRefreshData {
            primary,
            secondary,
            supplemental,
            subscription,
            reset_credits_available,
            observed_at_ms: now_ms,
        },
        allowed,
        limit_reached,
        rate_limit_reached_type: payload
            .rate_limit_reached_type
            .and_then(|value| safe_label(&value.kind)),
    })
}

fn collect_supplemental_windows(
    payload: &UsagePayload,
    now_ms: u64,
) -> Vec<SupplementalQuotaWindowInput> {
    let mut windows = Vec::new();
    if let Some(rate_limit) = payload.code_review_rate_limit.as_ref() {
        append_supplemental_windows(
            &mut windows,
            "code_review",
            "Code Review",
            rate_limit,
            now_ms,
        );
    }
    for (index, entry) in payload
        .additional_rate_limits
        .as_deref()
        .unwrap_or_default()
        .iter()
        .take(15)
        .enumerate()
    {
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
            now_ms,
        );
    }
    windows
}

fn append_supplemental_windows(
    output: &mut Vec<SupplementalQuotaWindowInput>,
    id_prefix: &str,
    label: &str,
    rate_limit: &SupplementalRateLimitStatus,
    now_ms: u64,
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
        let Ok(window) = map_window(window.clone(), kind, now_ms) else {
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
    now_ms: u64,
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
    let window_minutes = window
        .limit_window_seconds
        .filter(|seconds| *seconds > 0)
        .and_then(|seconds| u32::try_from((seconds.saturating_add(59)) / 60).ok());
    Ok(QuotaWindowInput {
        kind,
        available_percent: Some(100.0 - used_percent),
        explicitly_full: None,
        reset,
        window_minutes,
        provider_cycle_id: None,
        observed_at_ms: now_ms,
    })
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
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;
    use zenith_relay_core::quota::{QuotaSnapshot, SubscriptionStatus};

    #[tokio::test]
    async fn usage_payload_normalizes_windows_plan_and_reset_credits() {
        let (endpoint, server) =
            spawn(Router::new().route("/backend-api/wham/usage", get(successful_usage))).await;
        let client = CodexQuotaClient::with_endpoint(endpoint).unwrap();
        let result = client
            .refresh_quota(
                "access-secret",
                "account-123",
                1_700_000_000_000,
                &Subscription::default(),
            )
            .await;
        let QuotaRefreshOutcome::Updated(data) = result else {
            panic!("expected updated quota");
        };
        assert_eq!(data.allowed, Some(false));
        assert_eq!(data.limit_reached, Some(true));
        assert_eq!(
            data.rate_limit_reached_type.as_deref(),
            Some("rate_limit_reached")
        );
        let (quota, subscription) = data.quota.normalize(&QuotaSnapshot::default()).unwrap();
        let primary = quota.primary.unwrap();
        assert_eq!(primary.available_basis_points, Some(7_500));
        assert_eq!(primary.window_minutes, Some(300));
        assert_eq!(primary.reset_at_ms, Some(1_700_000_300_000));
        let secondary = quota.secondary.unwrap();
        assert_eq!(secondary.available_basis_points, Some(10_000));
        assert_eq!(secondary.reset_at_ms, Some(1_700_000_000_000 + 604_800_000));
        assert_eq!(quota.supplemental.len(), 2);
        assert_eq!(quota.supplemental[0].id, "code_review:primary");
        assert_eq!(quota.supplemental[0].label, "Code Review");
        assert_eq!(
            quota.supplemental[0].window.available_basis_points,
            Some(6_000)
        );
        assert_eq!(quota.supplemental[1].id, "additional:0:secondary");
        assert_eq!(quota.supplemental[1].label, "GPT-5 Priority");
        assert_eq!(quota.reset_credits_available, Some(2));
        let subscription = subscription.unwrap();
        assert_eq!(subscription.plan_type.as_deref(), Some("pro"));
        assert_eq!(subscription.status, SubscriptionStatus::Active);
        server.abort();
    }

    #[test]
    fn optional_quota_windows_are_absent_or_skipped_when_unusable() {
        let data = parse_usage_payload(
            br#"{
                "rate_limit":{
                    "allowed":true,
                    "limit_reached":false,
                    "primary_window":{"used_percent":25,"limit_window_seconds":18000}
                },
                "code_review_rate_limit":{"primary_window":{"limit_window_seconds":18000}},
                "additional_rate_limits":[{
                    "limit_name":"Broken optional limit",
                    "rate_limit":{"primary_window":{"used_percent":101,"limit_window_seconds":18000}}
                }]
            }"#,
            1_000,
        )
        .unwrap();
        assert!(data.quota.supplemental.is_empty());
        assert_eq!(
            data.quota
                .normalize(&QuotaSnapshot::default())
                .unwrap()
                .0
                .primary
                .unwrap()
                .available_basis_points,
            Some(7_500)
        );

        let without_optional = parse_usage_payload(
            br#"{
                "rate_limit":{
                    "allowed":true,
                    "limit_reached":false,
                    "primary_window":{"used_percent":25}
                }
            }"#,
            1_000,
        )
        .unwrap();
        assert!(without_optional.quota.supplemental.is_empty());
    }

    #[test]
    fn primary_window_rejects_missing_or_invalid_usage_percentage() {
        for body in [
            br#"{"rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{}}}"#.as_slice(),
            br#"{"rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{"used_percent":101}}}"#.as_slice(),
        ] {
            assert_eq!(
                parse_usage_payload(body, 1_000).unwrap_err().code,
                "quota_invalid_percentage"
            );
        }
    }

    #[tokio::test]
    async fn http_and_parse_failures_preserve_previous_subscription() {
        let previous = Subscription {
            plan_type: Some("plus".into()),
            active_until_ms: None,
            status: SubscriptionStatus::Active,
            updated_at_ms: Some(99),
        };
        for (router, expected_code) in [
            (
                Router::new().route("/backend-api/wham/usage", get(upstream_failure)),
                "quota_upstream",
            ),
            (
                Router::new().route("/backend-api/wham/usage", get(invalid_usage)),
                "quota_invalid_response",
            ),
        ] {
            let (endpoint, server) = spawn(router).await;
            let result = CodexQuotaClient::with_endpoint(endpoint)
                .unwrap()
                .refresh_quota("access-secret", "account-123", 100, &previous)
                .await;
            let QuotaRefreshOutcome::Failed {
                failure,
                subscription,
            } = result
            else {
                panic!("expected failed quota refresh");
            };
            assert_eq!(failure.code, expected_code);
            assert_eq!(subscription, previous);
            let rendered = format!("{failure:?}");
            assert!(!rendered.contains("access-secret"));
            assert!(!rendered.contains("provider-body-secret"));
            server.abort();
        }
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
                "allowed": false,
                "limit_reached": true,
                "primary_window": {
                    "used_percent": 25,
                    "limit_window_seconds": 18_000,
                    "reset_after_seconds": 1,
                    "reset_at": 1_700_000_300
                },
                "secondary_window": {
                    "used_percent": 0,
                    "limit_window_seconds": 604_800,
                    "reset_after_seconds": 604_800,
                    "reset_at": 0
                }
            },
            "code_review_rate_limit": {
                "primary_window": {
                    "used_percent": 40,
                    "limit_window_seconds": 18_000,
                    "reset_after_seconds": 300
                }
            },
            "additional_rate_limits": [{
                "limit_name": "  GPT-5   Priority  ",
                "rate_limit": {
                    "secondary_window": {
                        "used_percent": 10,
                        "limit_window_seconds": 604_800,
                        "reset_after_seconds": 604_800
                    }
                }
            }],
            "rate_limit_reached_type": { "type": "rate_limit_reached" },
            "rate_limit_reset_credits": { "available_count": 2 }
        }))
    }

    async fn upstream_failure() -> impl IntoResponse {
        (StatusCode::BAD_GATEWAY, "provider-body-secret")
    }

    async fn invalid_usage() -> impl IntoResponse {
        (StatusCode::OK, "provider-body-secret")
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
