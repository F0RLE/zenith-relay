use super::*;
use axum::http::{Method, header};

async fn observe(router: Router, timeout: std::time::Duration) -> Vec<SubscriptionEndpointProbe> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = CodexSubscriptionClient::with_endpoints(
        Client::builder()
            .no_proxy()
            .timeout(timeout)
            .build()
            .unwrap(),
        Url::parse(&format!("{base}/check")).unwrap(),
        Url::parse(&format!("{base}/subscriptions")).unwrap(),
    )
    .unwrap();
    let observations = client.probe_anonymous().await;
    server.abort();
    observations
}

#[tokio::test]
async fn anonymous_probe_uses_get_without_account_secrets_and_never_reads_bodies() {
    let router = Router::new()
        .route(
            "/check",
            get(|request: Request| async move {
                assert_eq!(request.method(), Method::GET);
                assert!(!request.headers().contains_key(header::AUTHORIZATION));
                assert!(!request.headers().contains_key(header::COOKIE));
                assert!(request.uri().query().is_none());
                assert_eq!(
                    request.headers()["x-openai-target-path"],
                    "/backend-api/accounts/check/v4-2023-04-27"
                );
                (
                    StatusCode::UNAUTHORIZED,
                    [("content-type", "application/json")],
                    "synthetic-private-body",
                )
            }),
        )
        .route(
            "/subscriptions",
            get(|request: Request| async move {
                assert!(!request.headers().contains_key(header::AUTHORIZATION));
                assert!(!request.headers().contains_key(header::COOKIE));
                (
                    StatusCode::FORBIDDEN,
                    [
                        ("content-type", "text/html"),
                        ("cf-mitigated", "challenge"),
                        ("cf-ray", "0123456789abcdef-FRA"),
                    ],
                    "synthetic-private-body",
                )
            }),
        );
    let observed = observe(router, std::time::Duration::from_secs(2)).await;
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0].http_status, Some(401));
    assert_eq!(observed[1].http_status, Some(403));
    assert_eq!(
        observed[1].response_mitigation,
        Some("cloudflare_challenge")
    );
    assert_eq!(observed[1].cf_ray.as_deref(), Some("0123456789abcdef-FRA"));
    assert!(
        !serde_json::to_string(&observed)
            .unwrap()
            .contains("synthetic-private-body")
    );
}

#[tokio::test]
async fn anonymous_probe_stops_on_retry_after_or_challenge_without_retrying() {
    for (status, headers) in [
        (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "120")]),
        (StatusCode::FORBIDDEN, [("cf-mitigated", "challenge")]),
    ] {
        let attempts = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .fallback(move |State(count): State<Arc<AtomicUsize>>| async move {
                count.fetch_add(1, Ordering::SeqCst);
                (status, headers)
            })
            .with_state(attempts.clone());
        let observed = observe(router, std::time::Duration::from_secs(2)).await;
        assert_eq!(observed.len(), 1);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(observed[0].http_status, Some(status.as_u16()));
    }
}

#[tokio::test]
async fn anonymous_probe_reports_timeouts_without_provider_error_text() {
    let router = Router::new().fallback(|| async {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        StatusCode::OK
    });
    let observed = observe(router, std::time::Duration::from_millis(20)).await;
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].error_code, Some("timeout"));
    assert_eq!(observed[0].http_status, None);
}
