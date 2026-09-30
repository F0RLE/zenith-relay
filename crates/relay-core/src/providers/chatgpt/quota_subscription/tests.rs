use super::*;
use axum::{
    extract::{Request, State},
    http::HeaderMap,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use base64::Engine;
use reqwest::{
    header::{AUTHORIZATION, USER_AGENT},
    Client, StatusCode,
};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use url::Url;

async fn check_client(router: Router) -> (CodexSubscriptionClient, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let endpoint = Url::parse(&format!(
        "http://{address}/backend-api/accounts/check/v4-2023-04-27"
    ))
    .unwrap();
    let client =
        CodexSubscriptionClient::with_endpoints(Client::new(), endpoint.clone(), endpoint).unwrap();
    (client, server)
}

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
fn account_check_identity_requires_an_authenticated_match() {
    let payload = json!({
        "account_ordering": ["second", "first"],
        "accounts": {
            "first": {"account": {"id": "account-first"}},
            "second": {"account": {"id": "account-second"}}
        }
    });

    assert_eq!(
        account_ids_from_check_response(&payload),
        vec!["account-second", "account-first"]
    );
    assert_eq!(
        resolve_account_check_account_id(&payload, &["account-first"]),
        Ok("account-first".to_string())
    );
    assert_eq!(
        resolve_account_check_account_id(&payload, &["unrelated-account"]),
        Err(AccountCheckIdentityError::Mismatch)
    );
    assert_eq!(
        resolve_account_check_account_id(&json!({"accounts": []}), &[]),
        Err(AccountCheckIdentityError::Missing)
    );
    assert_eq!(
        resolve_account_check_account_id(&payload, &[]),
        Ok("account-second".to_string())
    );
}

#[test]
fn account_check_hints_are_unverified_and_bounded() {
    let payload = json!({
        "https://api.openai.com/auth": {
            "chatgpt_account_id": "account-from-jwt",
            "account_id": "account-alias"
        }
    });
    let token = format!(
        "header.{}.signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
    );
    assert_eq!(
        unverified_chatgpt_account_id_hints(&token),
        vec!["account-from-jwt", "account-alias"]
    );
    assert!(unverified_chatgpt_account_id_hints("not-a-jwt").is_empty());
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

#[test]
fn refreshed_same_plan_without_expiry_drops_a_past_expiry() {
    let mut plan_type = Some("plus".to_string());
    let mut active_until_ms = Some(1_000);

    merge_subscription_metadata_at(
        &mut plan_type,
        &mut active_until_ms,
        CodexSubscriptionMetadata {
            account_id: None,
            plan_type: Some("plus".into()),
            active_until_ms: None,
        },
        Some(2_000),
    );

    assert_eq!(plan_type.as_deref(), Some("plus"));
    assert_eq!(active_until_ms, None);
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
    let (client, server) = check_client(router).await;

    let metadata = client
        .fetch("access-secret", "account-target", 1_700_000_000_000)
        .await
        .unwrap();

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(metadata.active_until_ms, Some(1_788_998_400_000));
    server.abort();
}

#[tokio::test]
async fn subscription_retry_after_prevents_an_early_nested_retry() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/backend-api/accounts/check/v4-2023-04-27",
            get(|State(attempts): State<Arc<AtomicUsize>>| async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "3600")],
                    "later",
                )
            }),
        )
        .with_state(attempts.clone());
    let (client, server) = check_client(router).await;
    let failure = client.fetch("synthetic", "account", 0).await.unwrap_err();
    assert_eq!(failure.retry_after_ms(), Some(3_600_000));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
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
