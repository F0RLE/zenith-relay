use super::{collect_response_body, valid_access_token, ResponseBodyError};
use crate::quota::QuotaRefreshFailure;
use chrono::{DateTime, Local, TimeZone, Utc};
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, REFERER, USER_AGENT},
    Client, StatusCode,
};
use serde_json::{Map, Value};
use url::Url;

pub const CODEX_ACCOUNTS_CHECK_ENDPOINT: &str =
    "https://chatgpt.com/backend-api/accounts/check/v4-2023-04-27";
pub const CODEX_SUBSCRIPTIONS_ENDPOINT: &str = "https://chatgpt.com/backend-api/subscriptions";
pub const SUBSCRIPTION_REFRESH_INTERVAL_MS: u64 = 30 * 60 * 1_000;

const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_ACCOUNT_ID_BYTES: usize = 512;
// These ChatGPT Web endpoints require browser-shaped headers, not the Codex API identity envelope.
const CHATGPT_WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CodexSubscriptionMetadata {
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub active_until_ms: Option<u64>,
}

#[derive(Clone)]
pub struct CodexSubscriptionClient {
    http: Client,
    accounts_check_endpoint: Url,
    subscriptions_endpoint: Url,
}

impl CodexSubscriptionClient {
    pub fn new(http: Client) -> Result<Self, QuotaRefreshFailure> {
        Self::with_endpoints(
            http,
            Url::parse(CODEX_ACCOUNTS_CHECK_ENDPOINT)
                .map_err(|_| failure("subscription_configuration", false))?,
            Url::parse(CODEX_SUBSCRIPTIONS_ENDPOINT)
                .map_err(|_| failure("subscription_configuration", false))?,
        )
    }

    pub fn with_endpoints(
        http: Client,
        accounts_check_endpoint: Url,
        subscriptions_endpoint: Url,
    ) -> Result<Self, QuotaRefreshFailure> {
        for endpoint in [&accounts_check_endpoint, &subscriptions_endpoint] {
            if !matches!(endpoint.scheme(), "http" | "https")
                || endpoint.host_str().is_none()
                || !endpoint.username().is_empty()
                || endpoint.password().is_some()
            {
                return Err(failure("subscription_configuration", false));
            }
        }
        Ok(Self {
            http,
            accounts_check_endpoint,
            subscriptions_endpoint,
        })
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
            Err(error) if error.retryable => {
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
        let response = self
            .http
            .get(self.accounts_check_endpoint.clone())
            .query(&[(
                "timezone_offset_min",
                -(Local::now().offset().local_minus_utc() / 60),
            )])
            .headers(subscription_headers(
                authorization.clone(),
                "/backend-api/accounts/check/v4-2023-04-27",
            )?)
            .send()
            .await
            .map_err(|_| failure("subscription_transport", true))?;
        let payload = response_json(response).await?;
        let mut metadata = parse_accounts_check(&payload, preferred_account_id)?;
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
        let response = self
            .http
            .get(self.subscriptions_endpoint.clone())
            .query(&[("account_id", account_id)])
            .headers(subscription_headers(
                authorization,
                "/backend-api/subscriptions",
            )?)
            .send()
            .await
            .map_err(|_| failure("subscription_transport", true))?;
        let payload = response_json(response).await?;
        let fallback = parse_subscriptions(&payload, account_id);
        metadata.account_id = fallback.account_id.or(metadata.account_id);
        metadata.plan_type = fallback.plan_type.or(metadata.plan_type);
        metadata.active_until_ms = fallback.active_until_ms.or(metadata.active_until_ms);
        Ok(metadata)
    }
}

pub fn subscription_refresh_due(
    active_until_ms: Option<u64>,
    updated_at_ms: Option<u64>,
    now_ms: u64,
) -> bool {
    if active_until_ms.is_none() {
        return true;
    }
    updated_at_ms
        .map(|updated_at_ms| {
            now_ms.saturating_sub(updated_at_ms) >= SUBSCRIPTION_REFRESH_INTERVAL_MS
        })
        .unwrap_or(true)
}

pub fn merge_subscription_metadata(
    plan_type: &mut Option<String>,
    active_until_ms: &mut Option<u64>,
    metadata: CodexSubscriptionMetadata,
) {
    if crate::quota::subscription_plan_changed(plan_type.as_deref(), metadata.plan_type.as_deref())
        && metadata.active_until_ms.is_none()
    {
        *active_until_ms = None;
    }
    if metadata.plan_type.is_some() {
        *plan_type = metadata.plan_type;
    }
    if metadata.active_until_ms.is_some() {
        *active_until_ms = metadata.active_until_ms;
    }
}

fn authorization_header(access_token: &str) -> Result<HeaderValue, QuotaRefreshFailure> {
    if !valid_access_token(access_token) {
        return Err(failure("subscription_access_token_invalid", false));
    }
    HeaderValue::from_str(&format!("Bearer {access_token}"))
        .map_err(|_| failure("subscription_access_token_invalid", false))
}

fn validate_account_id(value: &str) -> Result<&str, QuotaRefreshFailure> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_ACCOUNT_ID_BYTES
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(failure("subscription_account_id_invalid", false))
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
        .map_err(|_| failure("subscription_configuration", false))?;
    headers.insert("x-openai-target-path", target.clone());
    headers.insert("x-openai-target-route", target);
    Ok(headers)
}

async fn response_json(response: reqwest::Response) -> Result<Value, QuotaRefreshFailure> {
    let status = response.status();
    let body = collect_response_body(response, MAX_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            ResponseBodyError::Transport => failure("subscription_transport", true),
            ResponseBodyError::TooLarge => failure("subscription_response_too_large", false),
        })?;
    if !status.is_success() {
        return Err(http_failure(status));
    }
    serde_json::from_slice(&body).map_err(|_| failure("subscription_invalid_response", false))
}

fn http_failure(status: StatusCode) -> QuotaRefreshFailure {
    match status.as_u16() {
        401 => failure("subscription_unauthorized", false),
        403 => failure("subscription_forbidden", false),
        429 => failure("subscription_rate_limited", true),
        _ if status.is_server_error() => failure("subscription_upstream", true),
        _ => failure("subscription_http_status", false),
    }
}

fn parse_accounts_check(
    payload: &Value,
    preferred_account_id: &str,
) -> Result<CodexSubscriptionMetadata, QuotaRefreshFailure> {
    let records = account_records(payload);
    if records.is_empty() {
        return Err(failure("subscription_account_missing", false));
    }
    let ordered_key = payload
        .get("account_ordering")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let selected = records
        .iter()
        .find(|record| record_account_id(record).as_deref() == Some(preferred_account_id))
        .or_else(|| {
            ordered_key.and_then(|ordered_key| {
                records.iter().find(|record| {
                    record.key.as_deref() == Some(ordered_key)
                        || record_account_id(record).as_deref() == Some(ordered_key)
                })
            })
        })
        .unwrap_or(&records[0]);
    let record = selected
        .node
        .as_object()
        .ok_or_else(|| failure("subscription_invalid_response", false))?;
    let account = record
        .get("account")
        .and_then(Value::as_object)
        .unwrap_or(record);
    let entitlement = record
        .get("entitlement")
        .and_then(Value::as_object)
        .or_else(|| account.get("entitlement").and_then(Value::as_object));
    Ok(CodexSubscriptionMetadata {
        account_id: record_account_id(selected),
        plan_type: entitlement
            .and_then(|value| string_field(value, &["subscription_plan", "plan_type"]))
            .or_else(|| string_field(account, &["plan_type", "planType"])),
        active_until_ms: entitlement
            .and_then(|value| timestamp_field(value, &["expires_at", "active_until"]))
            .or_else(|| {
                timestamp_field(
                    account,
                    &["expires_at", "active_until", "subscription_active_until"],
                )
            }),
    })
}

fn parse_subscriptions(payload: &Value, account_id: &str) -> CodexSubscriptionMetadata {
    let candidates = [
        Some(payload),
        payload.get("data"),
        payload.get("subscription"),
        payload
            .get("data")
            .and_then(|value| value.get("subscription")),
    ];
    let mut metadata = CodexSubscriptionMetadata {
        account_id: Some(account_id.to_string()),
        ..Default::default()
    };
    for object in candidates
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        metadata.plan_type = metadata
            .plan_type
            .or_else(|| string_field(object, &["subscription_plan", "plan_type", "planType"]));
        metadata.active_until_ms = metadata.active_until_ms.or_else(|| {
            timestamp_field(
                object,
                &[
                    "active_until",
                    "activeUntil",
                    "expires_at",
                    "expiresAt",
                    "subscription_active_until",
                ],
            )
        });
    }
    metadata
}

struct AccountCheckRecord {
    key: Option<String>,
    node: Value,
}

fn account_records(payload: &Value) -> Vec<AccountCheckRecord> {
    let accounts = payload.get("accounts").unwrap_or(payload);
    match accounts {
        Value::Array(values) => values
            .iter()
            .filter(|value| value.is_object())
            .map(|node| AccountCheckRecord {
                key: None,
                node: node.clone(),
            })
            .collect(),
        Value::Object(values) if payload.get("accounts").is_some() => values
            .iter()
            .filter(|(_, value)| value.is_object())
            .map(|(key, node)| AccountCheckRecord {
                key: Some(key.clone()),
                node: node.clone(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn record_account_id(record: &AccountCheckRecord) -> Option<String> {
    let object = record.node.as_object()?;
    let account = object
        .get("account")
        .and_then(Value::as_object)
        .unwrap_or(object);
    string_field(
        account,
        &["account_id", "id", "chatgpt_account_id", "workspace_id"],
    )
    .or_else(|| record.key.clone())
}

fn string_field(object: &Map<String, Value>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| object.get(*name).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| {
            !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
        })
        .map(str::to_string)
}

fn timestamp_field(object: &Map<String, Value>, names: &[&str]) -> Option<u64> {
    names
        .iter()
        .find_map(|name| object.get(*name).and_then(parse_subscription_timestamp_ms))
}

pub fn parse_subscription_timestamp_ms(value: &Value) -> Option<u64> {
    match value {
        Value::Number(value) => value
            .as_u64()
            .or_else(|| {
                let value = value.as_f64()?;
                (value.is_finite() && value >= 0.0 && value <= u64::MAX as f64)
                    .then(|| value.trunc() as u64)
            })
            .and_then(normalize_epoch_ms),
        Value::String(value) => {
            let value = value.trim();
            if value.is_empty() || value.len() > 64 {
                return None;
            }
            value
                .parse::<u64>()
                .ok()
                .and_then(normalize_epoch_ms)
                .or_else(|| {
                    DateTime::parse_from_rfc3339(value)
                        .ok()
                        .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
                })
        }
        _ => None,
    }
}

fn normalize_epoch_ms(value: u64) -> Option<u64> {
    let value = if value < 100_000_000_000 {
        value.checked_mul(1_000)?
    } else {
        value
    };
    let signed = i64::try_from(value).ok()?;
    Utc.timestamp_millis_opt(signed).single()?;
    Some(value)
}

fn failure(code: &str, retryable: bool) -> QuotaRefreshFailure {
    QuotaRefreshFailure::new(code, retryable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Request, State},
        http::HeaderMap,
        response::IntoResponse,
        routing::get,
        Json, Router,
    };
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[test]
    fn account_check_prefers_the_requested_record_and_reads_entitlement() {
        let payload = json!({
            "account_ordering": ["first"],
            "accounts": {
                "first": {"account": {"id": "account-first"}},
                "second": {
                    "account": {"id": "account-target", "plan_type": "team"},
                    "entitlement": {
                        "subscription_plan": "business",
                        "expires_at": "2026-09-10T00:00:00Z"
                    }
                }
            }
        });
        let metadata = parse_accounts_check(&payload, "account-target").unwrap();
        assert_eq!(metadata.account_id.as_deref(), Some("account-target"));
        assert_eq!(metadata.plan_type.as_deref(), Some("business"));
        assert_eq!(metadata.active_until_ms, Some(1_788_998_400_000));
    }

    #[test]
    fn subscription_retries_after_thirty_minutes_even_with_a_future_hint() {
        assert_eq!(SUBSCRIPTION_REFRESH_INTERVAL_MS, 30 * 60 * 1_000);
        assert!(!subscription_refresh_due(
            Some(9_000_000),
            Some(1_000),
            1_000 + SUBSCRIPTION_REFRESH_INTERVAL_MS - 1,
        ));
        assert!(subscription_refresh_due(
            Some(9_000_000),
            Some(1_000),
            1_000 + SUBSCRIPTION_REFRESH_INTERVAL_MS,
        ));
        assert!(subscription_refresh_due(None, Some(1_000), 1_001));
    }

    #[test]
    fn subscription_timestamp_accepts_integral_and_fractional_json_numbers() {
        assert_eq!(
            parse_subscription_timestamp_ms(&json!(1_788_998_400.75)),
            Some(1_788_998_400_000)
        );
        assert_eq!(parse_subscription_timestamp_ms(&json!(-1.0)), None);
    }

    #[test]
    fn changed_plan_without_an_expiry_drops_the_previous_plan_expiry() {
        for (previous, observed) in [
            ("free", "plus"),
            ("plus", "business"),
            ("business", "pro"),
            ("team", "free"),
        ] {
            let mut plan_type = Some(previous.to_string());
            let mut active_until_ms = Some(1_788_998_400_000);

            merge_subscription_metadata(
                &mut plan_type,
                &mut active_until_ms,
                CodexSubscriptionMetadata {
                    account_id: None,
                    plan_type: Some(observed.to_string()),
                    active_until_ms: None,
                },
            );

            assert_eq!(plan_type.as_deref(), Some(observed));
            assert_eq!(active_until_ms, None);
        }
    }

    #[tokio::test]
    async fn subscriptions_fallback_uses_the_canonical_account_and_safe_headers() {
        let router = Router::new()
            .route(
                "/backend-api/accounts/check/v4-2023-04-27",
                get(account_check),
            )
            .route("/backend-api/subscriptions", get(subscription));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = CodexSubscriptionClient::with_endpoints(
            Client::new(),
            Url::parse(&format!(
                "http://{address}/backend-api/accounts/check/v4-2023-04-27"
            ))
            .unwrap(),
            Url::parse(&format!("http://{address}/backend-api/subscriptions")).unwrap(),
        )
        .unwrap();

        let metadata = client
            .fetch("access-secret", "account-hint", 1_700_000_000_000)
            .await
            .unwrap();
        assert_eq!(metadata.account_id.as_deref(), Some("account-canonical"));
        assert_eq!(metadata.plan_type.as_deref(), Some("plus"));
        assert_eq!(metadata.active_until_ms, Some(1_788_998_400_000));
        server.abort();
    }

    #[tokio::test]
    async fn transient_subscription_failure_is_retried_once() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route(
                "/backend-api/accounts/check/v4-2023-04-27",
                get(|State(attempts): State<Arc<AtomicUsize>>| async move {
                    if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    Json(json!({
                        "accounts": [{
                            "account": {"id": "account-target"},
                            "entitlement": {
                                "subscription_plan": "plus",
                                "expires_at": "2026-09-10T00:00:00Z"
                            }
                        }]
                    }))
                    .into_response()
                }),
            )
            .with_state(attempts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let endpoint = Url::parse(&format!(
            "http://{address}/backend-api/accounts/check/v4-2023-04-27"
        ))
        .unwrap();
        let client =
            CodexSubscriptionClient::with_endpoints(Client::new(), endpoint.clone(), endpoint)
                .unwrap();

        let metadata = client
            .fetch("access-secret", "account-target", 1_700_000_000_000)
            .await
            .unwrap();

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(metadata.active_until_ms, Some(1_788_998_400_000));
        server.abort();
    }

    async fn account_check(headers: HeaderMap, request: Request) -> impl IntoResponse {
        let timezone_offset = request
            .uri()
            .query()
            .and_then(|query| query.strip_prefix("timezone_offset_min="))
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap();
        assert!((-24 * 60..=24 * 60).contains(&timezone_offset));
        assert_eq!(
            headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer access-secret")
        );
        assert_eq!(
            headers
                .get("x-openai-target-path")
                .and_then(|value| value.to_str().ok()),
            Some("/backend-api/accounts/check/v4-2023-04-27")
        );
        assert_eq!(headers[USER_AGENT], CHATGPT_WEB_USER_AGENT);
        assert!(headers.get("chatgpt-account-id").is_none());
        assert!(headers.get("originator").is_none());
        assert!(headers.get("version").is_none());
        Json(json!({
            "accounts": [{
                "account": {"id": "account-canonical"},
                "entitlement": {"subscription_plan": "plus"}
            }]
        }))
    }

    async fn subscription(headers: HeaderMap, request: Request) -> impl IntoResponse {
        assert_eq!(request.uri().query(), Some("account_id=account-canonical"));
        assert_eq!(headers[USER_AGENT], CHATGPT_WEB_USER_AGENT);
        assert!(headers.get("chatgpt-account-id").is_none());
        assert!(headers.get("originator").is_none());
        Json(json!({
            "subscription_plan": "plus",
            "active_until": "2026-09-10T00:00:00Z"
        }))
    }
}
