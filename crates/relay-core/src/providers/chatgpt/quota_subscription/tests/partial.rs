use super::*;
use reqwest::header::HeaderValue;

async fn client(router: Router) -> (CodexSubscriptionClient, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = CodexSubscriptionClient::with_endpoints(
        Client::builder().no_proxy().build().unwrap(),
        Url::parse(&format!("{base}/check")).unwrap(),
        Url::parse(&format!("{base}/subscriptions")).unwrap(),
    )
    .unwrap();
    (client, server)
}

fn authorization() -> HeaderValue {
    HeaderValue::from_static("Bearer synthetic-subscription-test")
}

#[tokio::test]
async fn partial_subscription_read_preserves_account_metadata_and_challenge() {
    let attempts = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
    let router = Router::new()
        .route("/check", get(|State(attempts): State<Arc<(AtomicUsize, AtomicUsize)>>| async move {
            attempts.0.fetch_add(1, Ordering::SeqCst);
            Json(json!({"accounts": [{"account": {"id": "account-target", "plan_type": "plus"}}]}))
        }))
        .route("/subscriptions", get(|State(attempts): State<Arc<(AtomicUsize, AtomicUsize)>>| async move {
            attempts.1.fetch_add(1, Ordering::SeqCst);
            (StatusCode::FORBIDDEN, [("content-type", "text/html"), ("cf-mitigated", "challenge"), ("cf-ray", "0123456789abcdef-FRA")], "<html>synthetic</html>")
        }))
        .with_state(attempts.clone());
    let (client, server) = client(router).await;
    let read = client
        .read_authorized(authorization(), "account-target", 100)
        .await
        .unwrap();
    server.abort();
    assert_eq!(read.metadata.account_id.as_deref(), Some("account-target"));
    assert_eq!(read.metadata.plan_type.as_deref(), Some("plus"));
    assert_eq!(read.metadata.active_until_ms, None);
    let failure = read.fallback_failure.unwrap();
    assert_eq!(failure.code, error_codes::SUBSCRIPTION_FORBIDDEN);
    assert_eq!(failure.http_status(), Some(403));
    assert_eq!(failure.endpoint_stage(), Some("subscriptions"));
    assert_eq!(failure.response_mitigation(), Some("cloudflare_challenge"));
    assert_eq!(failure.cf_ray(), Some("0123456789abcdef-FRA"));
    assert_eq!(attempts.0.load(Ordering::SeqCst), 1);
    assert_eq!(attempts.1.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn subscription_fallback_retries_only_its_own_step() {
    let attempts = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
    let router = Router::new()
        .route("/check", get(|State(attempts): State<Arc<(AtomicUsize, AtomicUsize)>>| async move {
            attempts.0.fetch_add(1, Ordering::SeqCst);
            Json(json!({"accounts": [{"account": {"id": "account-target", "plan_type": "plus"}}]}))
        }))
        .route("/subscriptions", get(|State(attempts): State<Arc<(AtomicUsize, AtomicUsize)>>| async move {
            if attempts.1.fetch_add(1, Ordering::SeqCst) == 0 {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            Json(json!({"account_id": "account-target", "active_until": "2026-09-10T00:00:00Z"})).into_response()
        }))
        .with_state(attempts.clone());
    let (client, server) = client(router).await;
    let read = client
        .read_authorized(authorization(), "account-target", 100)
        .await
        .unwrap();
    server.abort();
    assert!(read.fallback_failure.is_none());
    assert_eq!(read.metadata.active_until_ms, Some(1_788_998_400_000));
    assert_eq!(attempts.0.load(Ordering::SeqCst), 1);
    assert_eq!(attempts.1.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn subscription_fallback_retry_after_preserves_partial_result_without_retry() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/check", get(|| async {
            Json(json!({"accounts": [{"account": {"id": "account-target", "plan_type": "plus"}}]}))
        }))
        .route("/subscriptions", get(|State(attempts): State<Arc<AtomicUsize>>| async move {
            attempts.fetch_add(1, Ordering::SeqCst);
            (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "120")])
        }))
        .with_state(attempts.clone());
    let (client, server) = client(router).await;
    let read = client
        .read_authorized(authorization(), "account-target", 100)
        .await
        .unwrap();
    server.abort();
    assert_eq!(read.metadata.plan_type.as_deref(), Some("plus"));
    let failure = read.fallback_failure.unwrap();
    assert_eq!(failure.http_status(), Some(429));
    assert_eq!(failure.retry_after_ms(), Some(120_000));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn subscription_fallback_account_mismatch_cannot_replace_authenticated_metadata() {
    let router = Router::new()
        .route("/check", get(|| async {
            Json(json!({"accounts": [{"account": {"id": "account-target", "plan_type": "plus"}}]}))
        }))
        .route("/subscriptions", get(|| async {
            Json(json!({"account_id": "other-account", "plan_type": "pro", "active_until": "2026-09-10T00:00:00Z"}))
        }));
    let (client, server) = client(router).await;
    let read = client
        .read_authorized(authorization(), "account-target", 100)
        .await
        .unwrap();
    server.abort();
    assert_eq!(read.metadata.account_id.as_deref(), Some("account-target"));
    assert_eq!(read.metadata.plan_type.as_deref(), Some("plus"));
    assert_eq!(read.metadata.active_until_ms, None);
    let failure = read.fallback_failure.unwrap();
    assert_eq!(failure.code, error_codes::SUBSCRIPTION_ACCOUNT_MISSING);
    assert_eq!(failure.endpoint_stage(), Some("subscriptions"));
}

#[test]
fn subscription_fallback_validates_explicit_identity_in_every_supported_wrapper() {
    for payload in [
        json!({"account_id": "other"}),
        json!({"accountId": "other"}),
        json!({"data": {"account_id": "other"}}),
        json!({"subscription": {"account_id": "other"}}),
        json!({"data": {"subscription": {"account_id": "other"}}}),
        json!({"account": {"id": "other"}}),
        json!({"account_id": "account-target", "subscription": {"account_id": "other"}}),
    ] {
        let failure =
            super::super::parse::parse_subscriptions(&payload, "account-target").unwrap_err();
        assert_eq!(failure.code, error_codes::SUBSCRIPTION_ACCOUNT_MISSING);
    }
    for payload in [
        json!({"account_id": " ACCOUNT-TARGET ", "plan_type": "plus"}),
        json!({"account": {"id": "account-target"}, "plan_type": "plus"}),
        json!({"plan_type": "plus"}),
    ] {
        let metadata =
            super::super::parse::parse_subscriptions(&payload, "account-target").unwrap();
        assert_eq!(metadata.plan_type.as_deref(), Some("plus"));
    }
}

#[test]
fn subscription_fallback_rejects_malformed_identity_and_non_object_responses() {
    for payload in [
        json!(null),
        json!([]),
        json!("invalid"),
        json!({"account_id": null}),
        json!({"account_id": 42}),
        json!({"account_id": ""}),
    ] {
        let failure =
            super::super::parse::parse_subscriptions(&payload, "account-target").unwrap_err();
        assert_eq!(failure.code, error_codes::SUBSCRIPTION_INVALID_RESPONSE);
    }
}

#[test]
fn subscription_ray_diagnostics_reject_arbitrary_header_contents() {
    use crate::quota::QuotaRefreshFailure;
    for value in [
        None,
        Some("synthetic-private-header"),
        Some("0123456789abcdef-FRA\nsecret"),
        Some("0123456789abcdef-fra"),
        Some("0123456789abcdef-TOOLONG"),
    ] {
        let failure = QuotaRefreshFailure::new("test", false).with_cf_ray(value);
        assert_eq!(failure.cf_ray(), None);
        assert!(!format!("{failure:?}").contains("synthetic-private-header"));
    }
    let failure = QuotaRefreshFailure::new("test", false).with_cf_ray(Some("0123456789abcdef-FRA"));
    assert_eq!(failure.cf_ray(), Some("0123456789abcdef-FRA"));
    assert!(std::mem::size_of::<QuotaRefreshFailure>() <= 128);
}
