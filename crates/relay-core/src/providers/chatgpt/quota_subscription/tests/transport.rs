use super::{Client, CodexSubscriptionClient, Url};
use crate::error_codes;

#[test]
fn subscription_client_rejects_unencrypted_remote_endpoints() {
    let secure = Url::parse("https://subscription.example.invalid/check").unwrap();
    let http = Client::new();
    for endpoint in [
        "http://subscription.example.invalid/check",
        "http://192.168.1.10/check",
        "http://[2001:db8::1]/check",
        "http://localhost.example.invalid/check",
        "http://127.0.0.1.example.invalid/check",
    ] {
        let insecure = Url::parse(endpoint).unwrap();
        for (accounts, subscriptions) in [
            (insecure.clone(), secure.clone()),
            (secure.clone(), insecure),
        ] {
            let result =
                CodexSubscriptionClient::with_endpoints(http.clone(), accounts, subscriptions);
            assert!(
                matches!(result, Err(ref error)
                    if error.code == error_codes::SUBSCRIPTION_CONFIGURATION && !error.retryable),
                "unencrypted remote endpoint must be rejected before sending credentials: {endpoint}"
            );
        }
    }
}

#[test]
fn subscription_client_accepts_https_and_loopback_endpoints() {
    let http = Client::new();
    for endpoint in [
        "https://subscription.example.invalid/check",
        "http://localhost:8123/check",
        "http://127.0.0.1:8123/check",
        "http://127.0.0.2:8123/check",
        "http://[::1]:8123/check",
    ] {
        let endpoint = Url::parse(endpoint).unwrap();
        assert!(
            CodexSubscriptionClient::with_endpoints(http.clone(), endpoint.clone(), endpoint)
                .is_ok()
        );
    }
}

#[test]
fn subscription_client_rejects_non_http_and_url_credentials() {
    let http = Client::new();
    for endpoint in [
        "file:///tmp/subscription",
        "ftp://subscription.example.invalid/check",
        "https://user:password@subscription.example.invalid/check",
        "http://user:password@127.0.0.1:8123/check",
    ] {
        let endpoint = Url::parse(endpoint).unwrap();
        assert!(
            CodexSubscriptionClient::with_endpoints(http.clone(), endpoint.clone(), endpoint)
                .is_err()
        );
    }
}

#[tokio::test]
async fn subscription_account_mismatch_stops_before_fallback_or_retry() {
    use super::{Arc, AtomicUsize, Json, Ordering, Router, State, get, json};

    let attempts = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/check",
            get(|State(attempts): State<Arc<AtomicUsize>>| async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Json(json!({"accounts": [{"account": {"id": "other", "plan_type": "pro"}}]}))
            }),
        )
        .route(
            "/subscriptions",
            get(|State(attempts): State<Arc<AtomicUsize>>| async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Json(json!({"plan_type": "pro"}))
            }),
        )
        .with_state(attempts.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = CodexSubscriptionClient::with_endpoints(
        Client::builder().no_proxy().build().unwrap(),
        Url::parse(&format!("{base}/check")).unwrap(),
        Url::parse(&format!("{base}/subscriptions")).unwrap(),
    )
    .unwrap();
    let result = client.fetch("synthetic", "account-target", 100).await;
    server.abort();
    let failure = result.unwrap_err();
    assert_eq!(failure.code, error_codes::SUBSCRIPTION_ACCOUNT_MISSING);
    assert_eq!(failure.endpoint_stage(), Some("accounts_check"));
    assert!(!failure.retryable);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn subscription_rejections_keep_safe_endpoint_context_without_repeating_403() {
    use super::{
        Arc, AtomicUsize, IntoResponse, Json, Ordering, Router, State, StatusCode, get, json,
    };

    for accounts_blocked in [true, false] {
        let attempts = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route(
                "/check",
                get(move |State(attempts): State<Arc<AtomicUsize>>| async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    if accounts_blocked {
                        return (
                            StatusCode::FORBIDDEN,
                            [("content-type", "application/json; charset=utf-8")],
                            r#"{"error":{"message":"synthetic-private-detail"}}"#,
                        )
                            .into_response();
                    }
                    Json(json!({"accounts": [{
                        "account": {"id": "account-target", "plan_type": "plus"}
                    }]}))
                    .into_response()
                }),
            )
            .route(
                "/subscriptions",
                get(|State(attempts): State<Arc<AtomicUsize>>| async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::FORBIDDEN,
                        [
                            ("content-type", "text/html; charset=utf-8"),
                            ("cf-mitigated", "challenge"),
                        ],
                        "<html>synthetic-private-detail</html>",
                    )
                }),
            )
            .with_state(attempts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = CodexSubscriptionClient::with_endpoints(
            Client::builder().no_proxy().build().unwrap(),
            Url::parse(&format!("{base}/check")).unwrap(),
            Url::parse(&format!("{base}/subscriptions")).unwrap(),
        )
        .unwrap();
        let failure = client
            .fetch("synthetic", "account-target", 100)
            .await
            .unwrap_err();
        assert_eq!(failure.code, error_codes::SUBSCRIPTION_FORBIDDEN);
        assert_eq!(failure.http_status(), Some(403));
        assert!(!failure.retryable);
        assert_eq!(
            failure.endpoint_stage(),
            Some(if accounts_blocked {
                "accounts_check"
            } else {
                "subscriptions"
            })
        );
        assert_eq!(
            failure.response_format(),
            Some(if accounts_blocked { "json" } else { "html" })
        );
        assert_eq!(
            failure.response_mitigation(),
            if accounts_blocked {
                None
            } else {
                Some("cloudflare_challenge")
            }
        );
        assert!(!format!("{failure:?}").contains("synthetic-private-detail"));
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            if accounts_blocked { 1 } else { 2 }
        );
        server.abort();
    }
}

#[tokio::test]
async fn subscription_challenge_diagnostics_require_the_explicit_header() {
    use super::{Arc, AtomicUsize, Ordering, Router, State, StatusCode, get};

    for (status, mitigation, expected) in [
        (
            StatusCode::FORBIDDEN,
            "challenge",
            Some("cloudflare_challenge"),
        ),
        (StatusCode::FORBIDDEN, "synthetic-private-header", None),
        (StatusCode::FORBIDDEN, "", None),
        (StatusCode::OK, "challenge", Some("cloudflare_challenge")),
    ] {
        let attempts = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route(
                "/check",
                get(move |State(attempts): State<Arc<AtomicUsize>>| async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    let mut response = axum::http::Response::builder()
                        .status(status)
                        .header("content-type", "text/html");
                    if !mitigation.is_empty() {
                        response = response.header("cf-mitigated", mitigation);
                    }
                    response
                        .body(axum::body::Body::from(
                            "<html>synthetic-private-body</html>",
                        ))
                        .unwrap()
                }),
            )
            .with_state(attempts.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint =
            Url::parse(&format!("http://{}/check", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = CodexSubscriptionClient::with_endpoints(
            Client::builder().no_proxy().build().unwrap(),
            endpoint.clone(),
            endpoint,
        )
        .unwrap();
        let result = client.fetch("synthetic", "account-target", 100).await;
        server.abort();
        let failure = result.unwrap_err();
        assert_eq!(failure.response_mitigation(), expected);
        assert_eq!(failure.response_format(), Some("html"));
        assert_eq!(failure.http_status(), Some(status.as_u16()));
        assert_eq!(failure.endpoint_stage(), Some("accounts_check"));
        assert_eq!(
            failure.code,
            if status == StatusCode::FORBIDDEN {
                error_codes::SUBSCRIPTION_FORBIDDEN
            } else {
                error_codes::SUBSCRIPTION_INVALID_RESPONSE
            }
        );
        assert!(!failure.retryable);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        let diagnostic = format!("{failure:?}");
        assert!(!diagnostic.contains("synthetic-private-header"));
        assert!(!diagnostic.contains("synthetic-private-body"));
    }
}

#[tokio::test]
async fn incomplete_error_bodies_preserve_http_classification_and_retry_after() {
    use super::{Arc, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    for (status, code, retryable, retry_after, expected_attempts) in [
        (401, error_codes::SUBSCRIPTION_UNAUTHORIZED, false, None, 1),
        (403, error_codes::SUBSCRIPTION_FORBIDDEN, false, None, 1),
        (
            429,
            error_codes::SUBSCRIPTION_RATE_LIMITED,
            true,
            Some(120),
            1,
        ),
        (503, error_codes::SUBSCRIPTION_UPSTREAM, true, Some(120), 1),
        (503, error_codes::SUBSCRIPTION_UPSTREAM, true, None, 2),
        (200, error_codes::SUBSCRIPTION_TRANSPORT, true, None, 2),
    ] {
        let attempts = Arc::new(AtomicUsize::new(0));
        let server_attempts = attempts.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint =
            Url::parse(&format!("http://{}/check", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 4096];
                let mut received = 0;
                while !request[..received]
                    .windows(4)
                    .any(|window| window == b"\r\n\r\n")
                {
                    assert!(received < request.len(), "synthetic request is too large");
                    let count = socket.read(&mut request[received..]).await.unwrap();
                    assert!(count > 0, "synthetic request ended before its headers");
                    received += count;
                }
                server_attempts.fetch_add(1, Ordering::SeqCst);
                let retry_header = retry_after
                    .map(|seconds| format!("Retry-After: {seconds}\r\n"))
                    .unwrap_or_default();
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 {status} Synthetic\r\nContent-Type: text/html\r\nContent-Length: 1024\r\nConnection: close\r\n{retry_header}\r\npartial"
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        let client = CodexSubscriptionClient::with_endpoints(
            Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(3))
                .build()
                .unwrap(),
            endpoint.clone(),
            endpoint,
        )
        .unwrap();
        let result = client.fetch("synthetic", "account-target", 100).await;
        server.abort();
        let failure = result.unwrap_err();
        assert_eq!(failure.code, code, "status {status}");
        assert_eq!(failure.http_status(), Some(status));
        assert_eq!(failure.retryable, retryable);
        assert_eq!(
            failure.retry_after_ms(),
            retry_after.map(|seconds| seconds * 1000)
        );
        assert_eq!(failure.endpoint_stage(), Some("accounts_check"));
        assert_eq!(failure.response_format(), Some("html"));
        assert_eq!(attempts.load(Ordering::SeqCst), expected_attempts);
    }
}
