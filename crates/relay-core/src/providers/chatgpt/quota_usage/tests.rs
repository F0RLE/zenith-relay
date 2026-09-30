use super::*;
use crate::quota::{QuotaSnapshot, SubscriptionStatus};
use crate::DefaultServiceTier;
use axum::{
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde_json::json;

fn client_with_subscription(usage_endpoint: Url) -> CodexQuotaClient {
    let http = reqwest::Client::builder()
        .redirect(Policy::none())
        .build()
        .unwrap();
    CodexQuotaClient {
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
        scope: ManagementHttpScope::default(),
    }
}

#[test]
fn chatgpt_adapter_implements_the_shared_quota_contract() {
    let client = CodexQuotaClient::with_endpoint(
        Url::parse("http://127.0.0.1:14999/backend-api/wham/usage").unwrap(),
    )
    .unwrap();
    let adapter: &dyn QuotaAdapter = &client;
    assert!(adapter.capabilities().supports_quota);
    assert!(adapter
        .capabilities()
        .supported_windows
        .contains(&QuotaWindowKind::Primary));
}

#[test]
fn chatgpt_task_failure_is_classified_inside_the_provider_adapter() {
    let failure = classify_quota_failure(401, b"task expired");
    assert!(is_agent_identity_task_invalid_failure(&failure));
    assert_eq!(failure.code, "invalid_task_id");
    assert_eq!(failure.http_status(), Some(401));
    assert_eq!(
        crate::quota::classify_quota_http_failure(401, b"task expired").code,
        "quota_unauthorized"
    );
}

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
            },{
                "limit_name":"GPT-5 Ultrafast",
                "rate_limit":{"primary_window":{"used_percent":5}}
            }],
            "rate_limit_reset_credits":{"available_count":2},
            "spend_control":{"individual_limit":{"remaining":"222.75"}}
        }"#,
        1_000,
    )
    .unwrap();
    assert_eq!(data.allowed, Some(true));
    assert_eq!(data.reported_limit_reached, Some(false));
    let (quota, subscription) = data.quota.normalize(&QuotaSnapshot::default()).unwrap();
    assert_eq!(quota.primary.unwrap().available_basis_points, Some(7_500));
    assert_eq!(
        quota.secondary.unwrap().available_basis_points,
        Some(10_000)
    );
    assert_eq!(quota.supplemental.len(), 3);
    assert_eq!(quota.supplemental[0].service_tier, None);
    assert_eq!(quota.supplemental[1].label, "GPT-5 Priority");
    assert_eq!(
        quota.supplemental[1].service_tier,
        Some(DefaultServiceTier::Fast)
    );
    assert_eq!(quota.supplemental[2].label, "GPT-5 Ultrafast");
    assert_eq!(
        quota.supplemental[2].service_tier,
        Some(DefaultServiceTier::Ultrafast)
    );
    assert_eq!(quota.reset_credits_available, Some(2));
    assert_eq!(quota.available_credits_micro_units, None);
    assert!(!quota.provider_credits_available);
    assert!(!quota.provider_credits_unlimited);
    assert_eq!(subscription.unwrap().plan_type.as_deref(), Some("plus"));
}

#[test]
fn provider_credits_follow_the_explicit_cockpit_ledger_shapes() {
    for (body, micro_units, available, unlimited) in [
        (r#"{}"#, None, false, false),
        (r#"{"credits":{"balance":"0"}}"#, Some(0), false, false),
        (
            r#"{"credits":{"remaining":1.25}}"#,
            Some(1_250_000),
            true,
            false,
        ),
        (
            r#"{"spend_control":{"individual_limit":{"limit":"400","used":"48.98"}}}"#,
            None,
            false,
            false,
        ),
        (r#"{"credits":{"unlimited":true}}"#, None, true, true),
        (
            r#"{"credits":[{"credit_amount":1.25},{"credit_amount":"2"}]}"#,
            Some(1_250_000),
            true,
            false,
        ),
        (r#"{"credits":[{"credit_amount":-1}]}"#, None, false, false),
        (
            r#"{"credits":[{"credit_amount":1000000000001}]}"#,
            None,
            false,
            false,
        ),
    ] {
        let quota = parse_codex_usage(body.as_bytes(), 1_000).unwrap().quota;
        assert_eq!(quota.available_credits_micro_units, micro_units);
        assert_eq!(quota.provider_credits_available, available);
        assert_eq!(quota.provider_credits_unlimited, unlimited);
    }
}

#[test]
fn provider_credits_do_not_mistake_a_static_spend_limit_for_the_ledger() {
    let quota = parse_codex_usage(
        br#"{
            "spend_control":{"individual_limit":{"remaining":1000}},
            "credits":{"remaining":927.8}
        }"#,
        1_000,
    )
    .unwrap()
    .quota;

    assert_eq!(quota.available_credits_micro_units, Some(927_800_000));
    assert!(quota.provider_credits_available);
}

#[test]
fn reset_credit_count_accepts_provider_variants() {
    for (body, expected) in [
        (
            r#"{"rate_limit_reset_credits":{"available_count":2}}"#,
            Some(2),
        ),
        (
            r#"{"rate_limit_reset_credits":{"availableCount":"3"}}"#,
            Some(3),
        ),
        (
            r#"{"rate_limit_reset_credits":{"available_count":-1}}"#,
            None,
        ),
        (
            r#"{"rate_limit_reset_credits":{"available_count":"bad"}}"#,
            None,
        ),
    ] {
        assert_eq!(
            parse_codex_usage(body.as_bytes(), 1_000)
                .unwrap()
                .quota
                .reset_credits_available,
            expected
        );
    }
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
    assert_eq!(limited.reported_limit_reached, Some(true));
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

#[test]
fn empty_secondary_provider_window_is_not_reported() {
    let data = parse_codex_usage(
        br#"{
            "rate_limit": {
                "primary_window": {
                    "used_percent": 30,
                    "limit_window_seconds": 2628000,
                    "reset_after_seconds": 1000
                },
                "secondary_window": {
                    "used_percent": 0,
                    "limit_window_seconds": 0,
                    "reset_after_seconds": 0
                }
            }
        }"#,
        1_000,
    )
    .unwrap();
    let (quota, _) = data.quota.normalize(&QuotaSnapshot::default()).unwrap();

    assert!(quota.primary.is_some());
    assert!(quota.secondary.is_none());
}

#[tokio::test]
async fn quota_retains_provider_retry_after_for_the_shared_scheduler() {
    let (base, server) = spawn(Router::new().route(
        "/backend-api/wham/usage",
        get(|| async {
            (
                StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", "120")],
                "{}",
            )
        }),
    ))
    .await;
    let endpoint = base.join("/backend-api/wham/usage").unwrap();
    let failure = CodexQuotaClient::with_endpoint(endpoint)
        .unwrap()
        .refresh_data("synthetic-access", "synthetic-account", 1_000)
        .await
        .unwrap_err();
    assert_eq!(failure.http_status(), Some(429));
    assert_eq!(failure.retry_after_ms(), Some(120_000));
    server.abort();
}

#[tokio::test]
async fn shared_client_sends_safe_headers_and_refreshes_subscription() {
    let router = Router::new()
        .route("/backend-api/wham/usage", get(successful_agent_usage))
        .route(
            "/backend-api/accounts/check/v4-2023-04-27",
            get(subscription_account_check),
        )
        .route("/backend-api/subscriptions", get(subscription_status));
    let (usage_endpoint, server) = spawn(router).await;
    let client = client_with_subscription(usage_endpoint);

    let QuotaRefreshOutcome::Updated(data) = client
        .refresh_quota_with_subscription_authorization(
            HeaderValue::from_static("AgentAssertion test"),
            Some(HeaderValue::from_static("Bearer access-secret")),
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
async fn quota_refresh_drops_an_expiry_from_a_different_subscription_plan() {
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
    let client = client_with_subscription(usage_endpoint);
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
    assert_eq!(subscription.plan_type.as_deref(), Some("business"));
    assert_eq!(subscription.active_until_ms, None);
    assert_eq!(subscription.updated_at_ms, Some(1_700_000_000_000));
    server.abort();
}

#[tokio::test]
async fn failed_subscription_probe_keeps_metadata_and_advances_refresh_time() {
    let router = Router::new()
        .route(
            "/backend-api/wham/usage",
            get(successful_usage_without_plan),
        )
        .route(
            "/backend-api/accounts/check/v4-2023-04-27",
            get(upstream_failure),
        )
        .route("/backend-api/subscriptions", get(upstream_failure));
    let (usage_endpoint, server) = spawn(router).await;
    let client = client_with_subscription(usage_endpoint);
    let previous = Subscription {
        plan_type: Some("business".into()),
        active_until_ms: Some(1_800_000_000_000),
        status: SubscriptionStatus::Active,
        updated_at_ms: Some(99),
    };
    let now_ms = 1_700_000_000_000;

    let QuotaRefreshOutcome::Updated(data) = client
        .refresh_quota("access-secret", "account-123", now_ms, &previous, true)
        .await
    else {
        panic!("expected updated quota");
    };
    let (_, subscription) = data.quota.normalize(&QuotaSnapshot::default()).unwrap();
    let subscription = subscription.unwrap();
    assert_eq!(subscription.plan_type, previous.plan_type);
    assert_eq!(subscription.active_until_ms, previous.active_until_ms);
    assert_eq!(subscription.updated_at_ms, Some(now_ms));
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
    successful_usage_response()
}

async fn successful_agent_usage(headers: HeaderMap) -> impl IntoResponse {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("AgentAssertion test")
    );
    assert_eq!(
        headers
            .get(ACCOUNT_ID_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("account-123")
    );
    successful_usage_response()
}

fn successful_usage_response() -> Json<serde_json::Value> {
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

async fn successful_usage_without_plan() -> impl IntoResponse {
    Json(json!({
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

async fn subscription_account_check(headers: HeaderMap) -> impl IntoResponse {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer access-secret")
    );
    Json(json!({
        "accounts": [{
            "account": {"id": "account-123"},
            "entitlement": {"subscription_plan": "business"}
        }]
    }))
}

async fn subscription_status(headers: HeaderMap) -> impl IntoResponse {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer access-secret")
    );
    Json(json!({
        "subscription_plan": "business",
        "active_until": "2026-10-10T00:00:00Z"
    }))
}

async fn subscription_status_without_expiry(headers: HeaderMap) -> impl IntoResponse {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer access-secret")
    );
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
