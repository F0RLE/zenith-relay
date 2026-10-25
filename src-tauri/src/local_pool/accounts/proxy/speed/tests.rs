use super::*;
use axum::{
    body::Body,
    http::{HeaderMap, Response, StatusCode},
    routing::get,
    Router,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[test]
fn throughput_uses_decimal_megabits_and_a_nonzero_time_window() {
    assert_eq!(megabits_per_second(1_000_000, Duration::from_secs(1)), 8.0);
    assert_eq!(
        megabits_per_second(1_048_576, Duration::from_secs(2)),
        4.194304
    );
    assert!(megabits_per_second(1_000_000, Duration::ZERO).is_finite());
    assert!(DOWNLOAD_URL.ends_with(&format!("bytes={DOWNLOAD_BYTES}")));
}

#[test]
fn result_serializes_only_redacted_measurement_fields() {
    let result = ProxySpeedTestResult {
        proxy_id: "proxy_synthetic".into(),
        downloaded_bytes: DOWNLOAD_BYTES,
        elapsed_ms: 1000,
        download_mbps: Some(41.94304),
        error_code: None,
    };
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        serde_json::json!({
            "proxyId": "proxy_synthetic", "downloadedBytes": DOWNLOAD_BYTES,
            "elapsedMs": 1000, "downloadMbps": 41.94304, "errorCode": null,
        })
    );
}

fn binary(body: impl Into<Body>) -> Response<Body> {
    Response::builder()
        .header("content-type", "application/octet-stream")
        .body(body.into())
        .unwrap()
}

async fn mock_proxy() -> (ProxyConfig, tokio::task::JoinHandle<()>, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let received = hits.clone();
    let app = Router::new()
        .route(
            "/download",
            get(move |headers: HeaderMap| {
                let received = received.clone();
                async move {
                    received.fetch_add(1, Ordering::SeqCst);
                    assert!(headers.contains_key("proxy-authorization"));
                    assert!(!headers.contains_key("authorization"));
                    assert_eq!(headers["accept-encoding"], "identity");
                    assert_eq!(headers["cache-control"], "no-store");
                    binary(vec![7; DOWNLOAD_BYTES as usize])
                }
            }),
        )
        .route(
            "/auth",
            get(|| async { StatusCode::PROXY_AUTHENTICATION_REQUIRED }),
        )
        .route(
            "/rejected",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        )
        .route(
            "/redirect",
            get(|| async {
                (
                    StatusCode::FOUND,
                    [("location", "http://unreachable.invalid/download")],
                )
            }),
        )
        .route("/invalid", get(|| async { "Synthetic error page" }))
        .route("/short", get(|| async { binary("short") }))
        .route(
            "/large",
            get(|| async { binary(vec![0; DOWNLOAD_BYTES as usize + 1]) }),
        )
        .route(
            "/encoded",
            get(|| async {
                let mut response = binary(vec![0; DOWNLOAD_BYTES as usize]);
                response.headers_mut().insert(
                    "content-encoding",
                    axum::http::HeaderValue::from_static("gzip"),
                );
                response
            }),
        )
        .route(
            "/chunked",
            get(|| async {
                let chunks = futures_util::stream::iter([
                    Ok::<_, std::io::Error>(vec![0_u8; DOWNLOAD_BYTES as usize]),
                    Ok(vec![0_u8; 1]),
                ]);
                binary(Body::from_stream(chunks))
            }),
        )
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                binary(vec![0; DOWNLOAD_BYTES as usize])
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = ProxyConfig::parse(&format!(
        "http://synthetic:secret@{}",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (proxy, server, hits)
}

#[tokio::test]
async fn download_uses_only_the_selected_proxy_and_counts_the_payload() {
    let (proxy, server, hits) = mock_proxy().await;
    let (bytes, elapsed) = download(
        &proxy,
        "http://unreachable.invalid/download",
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    assert_eq!(bytes, DOWNLOAD_BYTES);
    assert!(elapsed >= Duration::from_millis(1));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn malformed_downloads_and_failures_never_become_zero_speed_results() {
    let (proxy, server, hits) = mock_proxy().await;
    for (path, expected) in [
        ("auth", error_codes::PROXY_CHECK_AUTH_FAILED),
        ("rejected", error_codes::PROXY_CHECK_REJECTED),
        ("redirect", error_codes::PROXY_CHECK_REJECTED),
        ("invalid", error_codes::PROXY_CHECK_INVALID_RESPONSE),
        ("short", error_codes::PROXY_CHECK_INVALID_RESPONSE),
        ("large", error_codes::PROXY_CHECK_INVALID_RESPONSE),
        ("encoded", error_codes::PROXY_CHECK_INVALID_RESPONSE),
        ("chunked", error_codes::PROXY_CHECK_INVALID_RESPONSE),
        ("slow", error_codes::PROXY_CHECK_TIMEOUT),
    ] {
        let code = download(
            &proxy,
            &format!("http://unreachable.invalid/{path}"),
            Duration::from_millis(200),
        )
        .await
        .unwrap_err();
        assert_eq!(code, expected, "{path}");
    }
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "redirect must not be followed"
    );
    server.abort();
}

#[tokio::test]
async fn parallel_speed_tests_wait_in_order_and_continue_after_failure() {
    let slot = SPEED_LIMIT.acquire().await.unwrap();
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = ProxyConfig::parse(&format!(
        "http://synthetic:secret@{}",
        closed.local_addr().unwrap()
    ))
    .unwrap();
    drop(closed);
    let started = std::sync::Mutex::new(Vec::new());
    let mut first = Box::pin(test("proxy_first".into(), &proxy, || {
        started.lock().unwrap().push(1);
        true
    }));
    let mut second = Box::pin(test("proxy_second".into(), &proxy, || {
        started.lock().unwrap().push(2);
        true
    }));
    assert!(futures_util::poll!(&mut first).is_pending());
    assert!(futures_util::poll!(&mut second).is_pending());
    assert!(started.lock().unwrap().is_empty());
    drop(slot);
    assert!(futures_util::poll!(&mut second).is_pending());
    for result in [first.await, second.await] {
        assert_eq!(
            result.error_code.as_deref(),
            Some(error_codes::PROXY_CHECK_CONNECTION_FAILED)
        );
        assert_eq!(result.downloaded_bytes, 0);
        assert!(result.download_mbps.is_none());
    }
    assert_eq!(*started.lock().unwrap(), [1, 2]);
}

#[tokio::test]
async fn queued_wait_does_not_consume_the_measurement_budget() {
    let slot = SPEED_LIMIT.acquire().await.unwrap();
    let (proxy, server, hits) = mock_proxy().await;
    let mut measurement = Box::pin(test_at(
        "proxy_queued".into(),
        &proxy,
        "http://unreachable.invalid/download",
        Duration::from_secs(1),
        || true,
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(1100), &mut measurement)
            .await
            .is_err()
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    drop(slot);
    let result = measurement.await;
    assert_eq!(result.error_code, None);
    assert_eq!(result.downloaded_bytes, DOWNLOAD_BYTES);
    assert!(result.download_mbps.unwrap() > 0.0);
    server.abort();
}

#[tokio::test]
async fn cancelling_a_waiter_and_failed_start_delivery_release_the_queue() {
    let slot = SPEED_LIMIT.acquire().await.unwrap();
    let (proxy, server, hits) = mock_proxy().await;
    let mut cancelled = Box::pin(test("proxy_cancelled".into(), &proxy, || {
        panic!("cancelled waiter must not start")
    }));
    assert!(futures_util::poll!(&mut cancelled).is_pending());
    drop(cancelled);
    drop(slot);
    let result = test("proxy_disconnected".into(), &proxy, || false).await;
    assert_eq!(
        result.error_code.as_deref(),
        Some(error_codes::PROXY_CHECK_UNAVAILABLE)
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert!(SPEED_LIMIT.try_acquire().is_ok());
    server.abort();
}

#[tokio::test]
async fn unavailable_proxy_never_falls_back_to_the_direct_network() {
    let hits = Arc::new(AtomicUsize::new(0));
    let received = hits.clone();
    let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = target.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            target,
            Router::new().route(
                "/download",
                get(move || {
                    let received = received.clone();
                    async move {
                        received.fetch_add(1, Ordering::SeqCst);
                        "must not be requested"
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = ProxyConfig::parse(&format!(
        "http://synthetic:secret@{}",
        closed.local_addr().unwrap()
    ))
    .unwrap();
    drop(closed);
    let code = download(
        &proxy,
        &format!("http://{address}/download"),
        Duration::from_millis(200),
    )
    .await
    .unwrap_err();
    assert!(
        code == error_codes::PROXY_CHECK_CONNECTION_FAILED
            || code == error_codes::PROXY_CHECK_TIMEOUT
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    server.abort();
}
