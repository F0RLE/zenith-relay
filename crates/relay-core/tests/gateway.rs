use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HOST};
use axum::http::{HeaderMap, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use zenith_relay_core::gateway;
use zenith_relay_core::{
    discover_source_models, GatewayRuntime, LocalGatewayKey, ProviderSource, UsageEvent, WireApi,
};

const LOCAL_KEY: &str = "local-test-key";
const SOURCE_KEY: &str = "upstream-test-key";
const OVERSIZED_MODELS_CONTENT_LENGTH: &str = "4194305";
const MAX_CLIENT_REQUEST_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
struct ObservedRequest {
    path: &'static str,
    authorization: Option<String>,
}

#[derive(Clone, Default)]
struct UpstreamState {
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
    release_stream: Arc<Notify>,
}

struct TestServer {
    base_url: String,
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn invalid_local_key_stops_before_upstream_execution() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .get(format!("{}/v1/models", gateway.base_url))
        .bearer_auth("wrong-local-key")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn non_local_host_stops_before_auth_and_upstream_execution() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .get(format!("{}/v1/models", gateway.base_url))
        .header(HOST, "example.test")
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
    assert!(state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn models_are_discovered_and_filtered_with_the_source_credential() {
    let (upstream, state) = spawn_upstream().await;
    let models = discover_source_models(&ProviderSource {
        id: "source-1".into(),
        name: "Synthetic upstream".into(),
        base_url: format!("{}/v1", upstream.base_url),
        api_key: SOURCE_KEY.into(),
        wire_api: WireApi::Responses,
        models: vec!["gpt-test".into()],
    })
    .await
    .unwrap();
    assert_eq!(models, ["gpt-test"]);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/models");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer upstream-test-key")
    );
}

#[tokio::test]
async fn model_discovery_rejects_a_bad_source_key() {
    let (upstream, _) = spawn_upstream().await;
    assert!(discover_source_models(&ProviderSource {
        id: "source-1".into(),
        name: "Synthetic upstream".into(),
        base_url: format!("{}/v1", upstream.base_url),
        api_key: "wrong-source-key".into(),
        wire_api: WireApi::Responses,
        models: vec!["gpt-test".into()],
    })
    .await
    .is_err());
}

#[tokio::test]
async fn model_discovery_rejects_an_oversized_body() {
    let upstream = spawn(Router::new().route(
        "/v1/models",
        get(|| async {
            Response::builder()
                .status(StatusCode::OK)
                .header("content-length", OVERSIZED_MODELS_CONTENT_LENGTH)
                .body(Body::empty())
                .unwrap()
        }),
    ))
    .await;
    assert!(discover_source_models(&ProviderSource {
        id: "source-1".into(),
        name: "Synthetic upstream".into(),
        base_url: format!("{}/v1", upstream.base_url),
        api_key: SOURCE_KEY.into(),
        wire_api: WireApi::Responses,
        models: vec![],
    })
    .await
    .is_err());
}

#[tokio::test]
async fn non_stream_response_and_usage_are_redacted() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "private prompt",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"], "resp_test");
    assert_eq!(body["usage"]["total_tokens"], 7);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer upstream-test-key")
    );
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].input_tokens, Some(3));
    assert_eq!(events[0].output_tokens, Some(4));
    assert_eq!(events[0].total_tokens, Some(7));
    let serialized = serde_json::to_string(&events[0]).unwrap();
    assert!(!serialized.contains("private prompt"));
    assert!(!serialized.contains(LOCAL_KEY));
    assert!(!serialized.contains(SOURCE_KEY));
}

#[tokio::test]
async fn large_client_requests_are_forwarded_with_a_bounded_limit() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "x".repeat(2 * 1024 * 1024),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.requests.lock().unwrap().len(), 1);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "x".repeat(MAX_CLIENT_REQUEST_BODY_BYTES),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "request_too_large");
    assert_eq!(state.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn oversized_non_stream_response_is_rejected_and_recorded() {
    let upstream = spawn(Router::new().route(
        "/v1/responses",
        post(|| async {
            let chunks = stream::iter([
                Ok::<_, Infallible>(Bytes::from(vec![b'x'; 8 * 1024 * 1024])),
                Ok::<_, Infallible>(Bytes::from(vec![b'x'; 8 * 1024 * 1024 + 1])),
            ]);
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from_stream(chunks))
                .unwrap()
        }),
    ))
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": "gpt-test", "input": "private prompt"}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "upstream_error");
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].http_status, StatusCode::BAD_GATEWAY.as_u16());
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_body_too_large")
    );
}

#[tokio::test]
async fn sse_chunks_cross_the_gateway_before_the_stream_finishes() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": "gpt-test", "input": "hello", "stream": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );

    let mut chunks = response.bytes_stream();
    let first = tokio::time::timeout(Duration::from_secs(1), chunks.next())
        .await
        .expect("first SSE chunk was buffered")
        .unwrap()
        .unwrap();
    assert_eq!(first, "data: {\"type\":\"response.created\"}\n\n");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), chunks.next())
            .await
            .is_err()
    );
    state.release_stream.notify_one();
    let second = tokio::time::timeout(Duration::from_secs(1), chunks.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        second,
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n"
    );
    let third = tokio::time::timeout(Duration::from_secs(1), chunks.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(third, "data: [DONE]\n\n");
    assert!(chunks.next().await.is_none());

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer upstream-test-key")
    );
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert!(events[0].ttft_ms.is_some_and(|ttft| ttft >= 50));
    assert!(events[0]
        .ttft_ms
        .is_some_and(|ttft| ttft <= events[0].latency_ms));
    assert!(events[0].latency_ms >= 50);
}

#[tokio::test]
async fn fragmented_terminal_sse_records_usage_before_client_disconnect() {
    let (upstream, _) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "terminal-fragmented",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    let mut chunks = response.bytes_stream();
    let mut received = Vec::new();
    while !received.windows(2).any(|window| window == b"\n\n") {
        let chunk = tokio::time::timeout(Duration::from_secs(1), chunks.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        received.extend_from_slice(&chunk);
    }
    drop(chunks);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if !events.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].input_tokens, Some(2));
    assert_eq!(events[0].output_tokens, Some(3));
    assert_eq!(events[0].total_tokens, Some(5));
    assert_ne!(
        events[0].error_category.as_deref(),
        Some("client_cancelled")
    );
}

#[tokio::test]
async fn failed_terminal_sse_is_not_recorded_as_success() {
    let (upstream, _) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "terminal-failed",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    let _ = response.bytes().await.unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_terminal")
    );
}

#[tokio::test]
async fn truncated_success_stream_is_recorded_as_incomplete() {
    let (upstream, _) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "truncated-stream",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.bytes().await.unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("stream_incomplete")
    );
}

#[tokio::test]
async fn non_success_stream_done_does_not_override_upstream_status() {
    let (upstream, _) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "limited-stream",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let _ = response.bytes().await.unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].http_status,
        StatusCode::TOO_MANY_REQUESTS.as_u16()
    );
    assert_eq!(events[0].error_category.as_deref(), Some("upstream_status"));
}

async fn spawn_gateway(
    upstream_base_url: &str,
    models: Vec<&str>,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let usage_events = events.clone();
    let runtime = GatewayRuntime::new(
        ProviderSource {
            id: "source-1".to_string(),
            name: "Synthetic upstream".to_string(),
            base_url: format!("{upstream_base_url}/v1"),
            api_key: SOURCE_KEY.to_string(),
            wire_api: WireApi::Responses,
            models: models.into_iter().map(str::to_string).collect(),
        },
        LocalGatewayKey {
            id: "local-key-1".to_string(),
            secret: LOCAL_KEY.to_string(),
        },
        Arc::new(move |event| usage_events.lock().unwrap().push(event)),
    )
    .unwrap();
    (spawn(gateway::router(Arc::new(runtime))).await, events)
}

async fn spawn_upstream() -> (TestServer, UpstreamState) {
    let state = UpstreamState::default();
    let app = Router::new()
        .route("/v1/models", get(upstream_models))
        .route("/v1/responses", post(upstream_responses))
        .layer(DefaultBodyLimit::max(MAX_CLIENT_REQUEST_BODY_BYTES))
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn(app: Router) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        base_url: format!("http://{address}"),
        task,
    }
}

async fn upstream_models(State(state): State<UpstreamState>, headers: HeaderMap) -> Response<Body> {
    observe(&state, "/v1/models", &headers);
    if !has_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({
        "object": "list",
        "data": [
            {"id": "gpt-test", "object": "model"},
            {"id": "hidden-model", "object": "model"}
        ]
    }))
    .into_response()
}

async fn upstream_responses(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    observe(&state, "/v1/responses", &headers);
    if !has_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request: Value = serde_json::from_slice(&body).unwrap();
    if request.get("stream").and_then(Value::as_bool) != Some(true) {
        return Json(json!({
            "id": "resp_test",
            "object": "response",
            "model": request["model"],
            "usage": {"input_tokens": 3, "output_tokens": 4, "total_tokens": 7}
        }))
        .into_response();
    }

    if request.get("input").and_then(Value::as_str) == Some("terminal-failed") {
        let chunks = stream::iter([
            Ok::<_, Infallible>(Bytes::from_static(
                b"data: {\"type\":\"response.failed\",\"response\":{}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n")),
        ]);
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(chunks))
            .unwrap();
    }

    if request.get("input").and_then(Value::as_str) == Some("truncated-stream") {
        let chunks = stream::iter([Ok::<_, Infallible>(Bytes::from_static(
            b"data: {\"type\":\"response.created\"}\n\n",
        ))]);
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(chunks))
            .unwrap();
    }

    if request.get("input").and_then(Value::as_str) == Some("limited-stream") {
        let chunks = stream::iter([Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n"))]);
        return Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(chunks))
            .unwrap();
    }

    if request.get("input").and_then(Value::as_str) == Some("terminal-fragmented") {
        let chunks = stream::unfold(0_u8, |step| async move {
            match step {
                0 => Some((
                    Ok::<_, Infallible>(Bytes::from_static(
                        b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":2,",
                    )),
                    1,
                )),
                1 => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    Some((
                        Ok::<_, Infallible>(Bytes::from_static(
                            b"\"output_tokens\":3,\"total_tokens\":5}}}\n\n",
                        )),
                        2,
                    ))
                }
                _ => {
                    std::future::pending::<()>().await;
                    None
                }
            }
        });
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .header(CACHE_CONTROL, "no-cache")
            .body(Body::from_stream(chunks))
            .unwrap();
    }

    let release_stream = state.release_stream.clone();
    let chunks = stream::unfold(0_u8, move |step| {
        let release_stream = release_stream.clone();
        async move {
            match step {
                0 => Some((
                    Ok::<_, Infallible>(Bytes::from_static(
                        b"data: {\"type\":\"response.created\"}\n\n",
                    )),
                    1,
                )),
                1 => {
                    release_stream.notified().await;
                    Some((
                        Ok::<_, Infallible>(Bytes::from_static(
                            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
                        )),
                        2,
                    ))
                }
                2 => Some((
                    Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n")),
                    3,
                )),
                _ => None,
            }
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .header(CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(chunks))
        .unwrap()
}

fn observe(state: &UpstreamState, path: &'static str, headers: &HeaderMap) {
    state.requests.lock().unwrap().push(ObservedRequest {
        path,
        authorization: headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
    });
}

fn has_source_key(headers: &HeaderMap) -> bool {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some("Bearer upstream-test-key")
}
