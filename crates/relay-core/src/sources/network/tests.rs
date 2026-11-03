use super::*;
use axum::{Router, http::StatusCode, routing::get};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn admission_and_transport_timeouts_preserve_their_known_status() {
    for failure in [
        HttpSendError::Admission(HttpAdmissionError::TimedOut),
        HttpSendError::Transport { timeout: true },
    ] {
        let mut observation = ConnectionProbeObservation::pending(false);
        observation.finish(Instant::now(), Ok(Err(ProbeFailure::http(failure))));
        assert_eq!(observation.status, ConnectionProbeStatus::Timeout);
        assert_eq!(observation.http_status, None);
    }
    assert_eq!(
        ProbeFailure::http(HttpSendError::Transport { timeout: false }),
        ProbeFailure::Unavailable
    );
}

fn source(base_url: String) -> ProviderSource {
    ProviderSource {
        id: "synthetic-source".into(),
        name: "Synthetic".into(),
        base_url,
        api_key: "synthetic-private-key".into(),
        wire_api: WireApi::Responses,
        models: vec!["synthetic-model".into()],
        proxy_url: None,
    }
}

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("http://{address}/v1"), task)
}

#[tokio::test]
async fn connection_check_uses_get_only_and_never_returns_wire_details() {
    let generations = Arc::new(AtomicUsize::new(0));
    let count = generations.clone();
    let (base_url, server) = serve(
        Router::new()
            .route("/v1/models", get(|| async { "synthetic-private-body" }))
            .route(
                "/v1/responses",
                get(|upgrade: axum::extract::WebSocketUpgrade| async {
                    upgrade.on_upgrade(|socket| async { drop(socket) })
                })
                .post(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    async { StatusCode::INTERNAL_SERVER_ERROR }
                }),
            ),
    )
    .await;
    let input = source(base_url);
    let result = check_source_connection(&input, &[]).await.unwrap();
    assert_eq!(result.http.status, ConnectionProbeStatus::Responded);
    assert_eq!(result.http.http_status, Some(200));
    assert!(result.http.http_version.is_some());
    let websocket = result.websocket.as_ref().unwrap();
    assert_eq!(websocket.http_status, Some(101));
    assert_eq!(websocket.websocket_upgraded, Some(true));
    assert_eq!(generations.load(Ordering::SeqCst), 0);
    let encoded = serde_json::to_string(&result).unwrap();
    for private in [
        &input.api_key,
        &input.base_url,
        &input.models[0],
        "synthetic-private-body",
    ] {
        assert!(!encoded.contains(private));
    }
    server.abort();
}

#[tokio::test]
async fn websocket_rejection_preserves_http_reachability_without_inference() {
    let (base_url, server) = serve(
        Router::new()
            .route("/v1/models", get(|| async { StatusCode::OK }))
            .route(
                "/v1/responses",
                get(|| async { (StatusCode::FORBIDDEN, "synthetic-private-reason") }),
            ),
    )
    .await;
    let result = check_source_connection(&source(base_url), &[])
        .await
        .unwrap();
    assert_eq!(result.http.http_status, Some(200));
    let websocket = result.websocket.unwrap();
    assert_eq!(websocket.status, ConnectionProbeStatus::Responded);
    assert_eq!(websocket.http_status, Some(403));
    assert_eq!(websocket.websocket_upgraded, Some(false));
    server.abort();
}

#[tokio::test]
async fn configured_proxy_failure_never_uses_a_direct_connection() {
    let direct = Arc::new(AtomicUsize::new(0));
    let seen = direct.clone();
    let (base_url, server) = serve(Router::new().route(
        "/v1/models",
        get(move || {
            seen.fetch_add(1, Ordering::SeqCst);
            async { StatusCode::OK }
        }),
    ))
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let mut input = source(base_url);
    input.proxy_url = Some(proxy);
    let result = check_source_connection(&input, &[]).await.unwrap();
    assert_eq!(result.http.status, ConnectionProbeStatus::Unavailable);
    assert_eq!(result.http.http_status, None);
    assert_eq!(direct.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn non_responses_sources_do_not_probe_websocket_or_follow_redirects() {
    let redirects = Arc::new(AtomicUsize::new(0));
    let seen = redirects.clone();
    let (base_url, server) = serve(
        Router::new()
            .route(
                "/v1/models",
                get(|| async { axum::response::Redirect::temporary("/private") }),
            )
            .route(
                "/private",
                get(move || {
                    seen.fetch_add(1, Ordering::SeqCst);
                    async { StatusCode::OK }
                }),
            ),
    )
    .await;
    let mut input = source(base_url);
    input.wire_api = WireApi::ChatCompletions;
    let result = check_source_connection(&input, &[]).await.unwrap();
    assert_eq!(result.http.http_status, Some(307));
    assert_eq!(result.websocket, None);
    assert_eq!(redirects.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn throttled_http_check_does_not_issue_another_websocket_probe() {
    let upgrades = Arc::new(AtomicUsize::new(0));
    let seen = upgrades.clone();
    let (base_url, server) = serve(
        Router::new()
            .route(
                "/v1/models",
                get(|| async { StatusCode::TOO_MANY_REQUESTS }),
            )
            .route(
                "/v1/responses",
                get(move || {
                    seen.fetch_add(1, Ordering::SeqCst);
                    async { StatusCode::FORBIDDEN }
                }),
            ),
    )
    .await;
    let result = check_source_connection(&source(base_url), &[])
        .await
        .unwrap();
    assert_eq!(result.http.http_status, Some(429));
    assert_eq!(result.websocket, None);
    assert_eq!(upgrades.load(Ordering::SeqCst), 0);
    server.abort();
}
