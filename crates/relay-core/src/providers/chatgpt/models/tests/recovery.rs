use super::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

fn short_timeout_client(endpoint: Url) -> AccountModelsClient {
    let mut client = AccountModelsClient::with_endpoint_proxy_timeout_and_user_agent(
        endpoint,
        None,
        Duration::from_millis(200),
        "Zenith Relay Test",
    )
    .unwrap();
    client.endpoint_is_custom = true;
    client
}

#[tokio::test]
async fn native_catalog_recovers_from_one_timeout_without_manual_refresh() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let (endpoint, server) = spawn(Router::new().route(
        "/backend-api/codex/models",
        get(move || {
            let calls = handler_calls.clone();
            async move {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Json(json!({"models": [{"slug": "gpt-test"}]}))
            }
        }),
    ))
    .await;
    let result = short_timeout_client(endpoint)
        .discover("synthetic-access", "synthetic-account", "1.0.0")
        .await;
    server.abort();
    assert_eq!(result, Ok(vec!["gpt-test".into()]));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn native_catalog_retries_a_timed_out_body_not_only_response_headers() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let (endpoint, server) = spawn(Router::new().route(
        "/backend-api/codex/models",
        get(move || {
            let calls = handler_calls.clone();
            async move {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    let body = futures_util::stream::once(async {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        Ok::<_, std::io::Error>("{\"models\": []}")
                    });
                    return axum::response::Response::new(axum::body::Body::from_stream(body));
                }
                Json(json!({"models": [{"slug": "gpt-test"}]})).into_response()
            }
        }),
    ))
    .await;
    let result = short_timeout_client(endpoint)
        .discover("synthetic-access", "synthetic-account", "1.0.0")
        .await;
    server.abort();
    assert_eq!(result, Ok(vec!["gpt-test".into()]));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn native_catalog_stops_after_two_transport_attempts() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let (endpoint, server) = spawn(Router::new().route(
        "/backend-api/codex/models",
        get(move || {
            let calls = handler_calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(2)).await;
                Json(json!({"models": [{"slug": "gpt-test"}]}))
            }
        }),
    ))
    .await;
    let error = short_timeout_client(endpoint)
        .discover("synthetic-access", "synthetic-account", "1.0.0")
        .await
        .unwrap_err();
    server.abort();
    assert_eq!(error.code, ModelDiscoveryFailureCode::Timeout);
    assert!(error.retryable);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn catalog_http_errors_and_invalid_json_do_not_trigger_transport_retries() {
    for (status, body, expected) in [
        (
            StatusCode::UNAUTHORIZED,
            "{}",
            ModelDiscoveryFailureCode::Unauthorized,
        ),
        (
            StatusCode::FORBIDDEN,
            "{}",
            ModelDiscoveryFailureCode::Forbidden,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "{}",
            ModelDiscoveryFailureCode::RateLimited,
        ),
        (
            StatusCode::BAD_GATEWAY,
            "{}",
            ModelDiscoveryFailureCode::Upstream,
        ),
        (
            StatusCode::OK,
            "not-json",
            ModelDiscoveryFailureCode::InvalidResponse,
        ),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let (endpoint, server) = spawn(Router::new().route(
            "/backend-api/codex/models",
            get(move || {
                handler_calls.fetch_add(1, Ordering::SeqCst);
                async move { (status, [("retry-after", "120")], body) }
            }),
        ))
        .await;
        let error = short_timeout_client(endpoint)
            .discover("synthetic-access", "synthetic-account", "1.0.0")
            .await
            .unwrap_err();
        server.abort();
        assert_eq!(error.code, expected);
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(error.retry_after_ms, Some(120_000));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn timed_out_http_error_body_keeps_retry_after_without_an_inline_retry() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let (endpoint, server) = spawn(Router::new().route(
        "/backend-api/codex/models",
        get(move || {
            handler_calls.fetch_add(1, Ordering::SeqCst);
            async {
                let body = futures_util::stream::once(async {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Ok::<_, std::io::Error>("{}")
                });
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "120")],
                    axum::body::Body::from_stream(body),
                )
            }
        }),
    ))
    .await;
    let error = short_timeout_client(endpoint)
        .discover("synthetic-access", "synthetic-account", "1.0.0")
        .await
        .unwrap_err();
    server.abort();
    assert_eq!(error.retry_after_ms, Some(120_000));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn catalog_retry_rechecks_the_owning_credential_revision() {
    let current = Arc::new(AtomicBool::new(true));
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_current = current.clone();
    let handler_calls = calls.clone();
    let (endpoint, server) = spawn(Router::new().route(
        "/backend-api/codex/models",
        get(move || {
            handler_calls.fetch_add(1, Ordering::SeqCst);
            handler_current.store(false, Ordering::SeqCst);
            async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                Json(json!({"models": [{"slug": "gpt-stale"}]}))
            }
        }),
    ))
    .await;
    let error = short_timeout_client(endpoint)
        .with_http_scope(ManagementHttpScope::checked(move || {
            current.load(Ordering::SeqCst)
        }))
        .discover("synthetic-access", "synthetic-account", "1.0.0")
        .await
        .unwrap_err();
    server.abort();
    assert_eq!(error.code, ModelDiscoveryFailureCode::Transport);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelling_catalog_discovery_does_not_leave_a_detached_retry() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let (endpoint, server) = spawn(Router::new().route(
        "/backend-api/codex/models",
        get(move || {
            handler_calls.fetch_add(1, Ordering::SeqCst);
            async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                Json(json!({"models": [{"slug": "gpt-test"}]}))
            }
        }),
    ))
    .await;
    let read = tokio::spawn(async move {
        short_timeout_client(endpoint)
            .discover("synthetic-access", "synthetic-account", "1.0.0")
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    read.abort();
    assert!(read.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(500)).await;
    server.abort();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn bps_permission_transport_failure_is_not_replayed_as_a_native_catalog() {
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let (endpoint, server) = spawn(Router::new().route(
        "/backend-api/codex/models",
        get(move || {
            handler_calls.fetch_add(1, Ordering::SeqCst);
            async {
                axum::response::Response::new(axum::body::Body::from_stream(
                    futures_util::stream::once(async {
                        Err::<&str, _>(std::io::Error::other("synthetic body failure"))
                    }),
                ))
            }
        }),
    ))
    .await;
    let error = short_timeout_client(endpoint)
        .with_oauth_client_kind(super::super::super::OAuthClientKind::ExcelBps)
        .discover("synthetic-access", "synthetic-account", "1.0.0")
        .await
        .unwrap_err();
    server.abort();
    assert_eq!(error.code, ModelDiscoveryFailureCode::Transport);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
