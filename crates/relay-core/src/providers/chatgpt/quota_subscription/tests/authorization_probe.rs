use super::*;
use reqwest::header::{COOKIE, HeaderValue};

async fn diagnostic_client(
    router: Router,
) -> (CodexSubscriptionClient, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = CodexSubscriptionClient::with_endpoints(
        Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .unwrap(),
        Url::parse(&format!("{base}/check")).unwrap(),
        Url::parse(&format!("{base}/subscriptions")).unwrap(),
    )
    .unwrap();
    (client, server)
}

fn account_check() -> Json<serde_json::Value> {
    Json(json!({"accounts":{"alias":{"account":{"id":"canonical","plan_type":"plus"}}}}))
}

#[tokio::test]
async fn authorization_probe_keeps_the_same_query_and_only_changes_the_bearer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/check",
            get(|request: Request| async move {
                assert_eq!(request.headers()[AUTHORIZATION], "Bearer synthetic-secret");
                assert!(!request.headers().contains_key(COOKIE));
                assert!(
                    request
                        .uri()
                        .query()
                        .unwrap()
                        .starts_with("timezone_offset_min=")
                );
                account_check()
            }),
        )
        .route(
            "/subscriptions",
            get(
                |State(calls): State<Arc<AtomicUsize>>, request: Request| async move {
                    assert_eq!(request.uri().query(), Some("account_id=canonical"));
                    assert!(!request.headers().contains_key(COOKIE));
                    assert_eq!(request.headers()[USER_AGENT], CHATGPT_WEB_USER_AGENT);
                    assert_eq!(
                        request.headers()["x-openai-target-path"],
                        "/backend-api/subscriptions"
                    );
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        assert!(!request.headers().contains_key(AUTHORIZATION));
                        (
                            StatusCode::UNAUTHORIZED,
                            [
                                ("content-type", "application/json"),
                                ("cf-mitigated", "none"),
                            ],
                            "synthetic-private-body",
                        )
                    } else {
                        assert_eq!(request.headers()[AUTHORIZATION], "Bearer synthetic-secret");
                        (
                            StatusCode::FORBIDDEN,
                            [("content-type", "text/html"), ("cf-mitigated", "challenge")],
                            "synthetic-private-body",
                        )
                    }
                },
            ),
        )
        .with_state(calls.clone());
    let (client, server) = diagnostic_client(router).await;
    let result = client
        .probe_authorization(
            HeaderValue::from_static("Bearer synthetic-secret"),
            "canonical",
        )
        .await
        .unwrap();
    server.abort();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(result[0].authentication, "none");
    assert_eq!(result[0].observation.http_status, Some(401));
    assert_eq!(result[1].authentication, "bearer");
    assert_eq!(result[1].observation.http_status, Some(403));
    assert_eq!(
        result[1].observation.response_mitigation,
        Some("cloudflare_challenge")
    );
    let output = serde_json::to_string(&result).unwrap();
    for private in ["synthetic-secret", "synthetic-private-body", "canonical"] {
        assert!(!output.contains(private));
    }
}

#[tokio::test]
async fn authorization_probe_stops_before_sending_credentials_after_a_provider_pause() {
    for (status, key, value) in [
        (StatusCode::FORBIDDEN, "cf-mitigated", "challenge"),
        (StatusCode::TOO_MANY_REQUESTS, "retry-after", "120"),
        (StatusCode::UNAUTHORIZED, "retry-after", "120"),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route("/check", get(|| async { account_check() }))
            .route(
                "/subscriptions",
                get(
                    move |State(calls): State<Arc<AtomicUsize>>, request: Request| async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        assert!(!request.headers().contains_key(AUTHORIZATION));
                        (status, [(key, value)])
                    },
                ),
            )
            .with_state(calls.clone());
        let (client, server) = diagnostic_client(router).await;
        let result = client
            .probe_authorization(
                HeaderValue::from_static("Bearer synthetic-secret"),
                "canonical",
            )
            .await
            .unwrap();
        server.abort();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(result.len(), 1);
        assert!(result[0].observation.stops_probe());
    }
}

#[tokio::test]
async fn authorization_probe_rejects_an_account_mismatch_before_the_fallback() {
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/check", get(|| async { account_check() }))
        .route(
            "/subscriptions",
            get(|State(calls): State<Arc<AtomicUsize>>| async move {
                calls.fetch_add(1, Ordering::SeqCst);
                StatusCode::OK
            }),
        )
        .with_state(calls.clone());
    let (client, server) = diagnostic_client(router).await;
    assert!(
        client
            .probe_authorization(
                HeaderValue::from_static("Bearer synthetic-secret"),
                "another-account"
            )
            .await
            .is_err()
    );
    server.abort();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn authorization_probe_honors_a_stale_scope_before_any_request() {
    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .fallback(|State(calls): State<Arc<AtomicUsize>>| async move {
            calls.fetch_add(1, Ordering::SeqCst);
            StatusCode::OK
        })
        .with_state(calls.clone());
    let (client, server) = diagnostic_client(router).await;
    let client = client
        .with_http_scope(crate::scheduler::refresh::http::ManagementHttpScope::checked(|| false));
    assert!(
        client
            .probe_authorization(
                HeaderValue::from_static("Bearer synthetic-secret"),
                "canonical"
            )
            .await
            .is_err()
    );
    server.abort();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
