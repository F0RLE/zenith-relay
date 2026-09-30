use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{
        header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HOST},
        HeaderMap, StatusCode,
    },
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    net::SocketAddr,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zenith_relay_core::WireApi;
use zenith_relay_server::{
    config::Config,
    http,
    state::{AppState, GatewayKeyRecord, SourceRecord},
    store::{Store, Vault},
};

#[path = "support/tool_policy.rs"]
mod tool_policy;

#[path = "support/rotation_upgrade.rs"]
mod rotation_upgrade;

#[path = "support/batch_import.rs"]
mod batch_import;
#[path = "support/configuration_updates.rs"]
mod configuration_updates;
#[path = "support/load_and_quota.rs"]
mod load_and_quota;
#[path = "support/membership_updates.rs"]
mod membership_updates;
#[path = "support/proxies_and_presets.rs"]
mod proxies_and_presets;
#[path = "support/remote_session.rs"]
mod remote_session;
#[path = "support/server_identity.rs"]
mod server_identity;
#[path = "support/source_lifecycle.rs"]
mod source_lifecycle;
#[path = "support/source_protocols.rs"]
mod source_protocols;

fn add_rebuild_failing_source(state: &AppState) {
    state
        .vault
        .save("source:broken", "synthetic-source-key")
        .unwrap();
    // A zero weight fails GatewayRuntime::build, which forces the rebuild to fail.
    state
        .store
        .save_source(&SourceRecord {
            id: "broken-source".into(),
            name: "Broken source".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "https://example.test/v1".into(),
            secret_ref: "source:broken".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_config: Default::default(),
            protocol_bindings: Vec::new(),
            models: vec!["broken-model".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 0,
            recovery_delay_seconds: 0,
            model_price_overrides: BTreeMap::new(),
            detected_model_prices: BTreeMap::new(),
            last_error_code: None,
        })
        .unwrap();
}

async fn batch_preview(client: &reqwest::Client, origin: &str, content: String) -> Value {
    let response = client
        .post(format!("{origin}/accounts/import/batch/preview"))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"content": content}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json().await.unwrap()
}

async fn assert_batch_error(
    client: &reqwest::Client,
    origin: &str,
    content: String,
    expected_code: &str,
) {
    let response = client
        .post(format!("{origin}/accounts/import/batch/preview"))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"content": content}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], expected_code);
}

struct RunningServer {
    origin: String,
    state: Arc<AppState>,
    task: tokio::task::JoinHandle<()>,
}

async fn spawn_server(root: &Path) -> RunningServer {
    spawn_server_with_token(root, "synthetic-management-token-value").await
}

async fn spawn_server_with_token(root: &Path, management_token: &str) -> RunningServer {
    let (account_check_url, _account_check_task) = spawn_import_account_check_upstream().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = Config {
        bind: address,
        data_dir: root.to_path_buf(),
        public_base_url: url::Url::parse(&format!("http://{address}")).unwrap(),
        management_token: management_token.to_string(),
        vault_key: [9; 32],
        account_check_url,
    };
    let store = Arc::new(Store::open(root.join("relay.sqlite")).unwrap());
    let vault = Arc::new(Vault::open(&root.join("vault"), config.vault_key).unwrap());
    let state = AppState::new(config, store, vault).unwrap();
    state.rebuild_runtime().await.unwrap();
    let router = http::router(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    RunningServer {
        origin: format!("http://{address}"),
        state,
        task,
    }
}

async fn spawn_import_account_check_upstream() -> (url::Url, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().route("/accounts/check", get(test_import_account_check));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (
        url::Url::parse(&format!("http://{address}/accounts/check")).unwrap(),
        task,
    )
}

async fn test_import_account_check(headers: HeaderMap) -> Json<Value> {
    assert!(headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("Bearer ")));
    let account_ids = [
        "synthetic-chatgpt-account-id",
        "synthetic-zenith-account",
        "synthetic-batch-account-one",
        "synthetic-batch-account-two",
        "synthetic-document-account-1",
        "synthetic-document-account-2",
        "synthetic-document-account-3",
        "synthetic-array-account",
        "synthetic-label-account",
        "synthetic-line-account-one",
        "synthetic-line-account-two",
        "synthetic-owned-account-one",
        "synthetic-owned-account-two",
        "synthetic-abandoned-account",
        "synthetic-cleanup-trigger-account",
        "synthetic-proxy-account-id",
        "synthetic-preset-account-id",
    ];
    Json(json!({
        "account_ordering": account_ids,
        "accounts": account_ids.into_iter().map(|id| {
            (id.to_string(), json!({"account": {"id": id}}))
        }).collect::<serde_json::Map<_, _>>(),
    }))
}

async fn spawn_upstream() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new()
        .route("/v1/models", get(models_response))
        .route("/v1/responses", post(upstream_response))
        .route("/account/responses", post(account_response))
        .route("/account/responses/compact", post(account_compact))
        .route("/account/alpha/search", post(account_search));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), task)
}

async fn spawn_scope_upstream() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new()
        .route("/v1/models", get(scope_models_response))
        .route("/v1/responses", post(upstream_response));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), task)
}

async fn spawn_mixed_protocol_upstream() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().route("/v1/models", get(mixed_protocol_models));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), task)
}

async fn mixed_protocol_models(request: Request) -> impl IntoResponse {
    if request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some("Bearer synthetic-upstream-api-key")
    {
        return Json(json!({"data":[{"id":"gpt-native"}]})).into_response();
    }
    if request
        .headers()
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        == Some("synthetic-upstream-api-key")
        && request
            .headers()
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok())
            == Some("2023-06-01")
    {
        return Json(json!({"data":[{"id":"claude-native"}]})).into_response();
    }
    StatusCode::UNAUTHORIZED.into_response()
}

async fn spawn_messages_upstream() -> (String, Arc<Mutex<Vec<()>>>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/v1/models", get(native_messages_models))
        .route("/v1/messages", post(native_messages_response))
        .with_state(requests.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), requests, task)
}

async fn native_messages_models(
    State(requests): State<Arc<Mutex<Vec<()>>>>,
    _request: Request,
) -> impl IntoResponse {
    requests.lock().unwrap().push(());
    Json(json!({"data":[{"id":"claude-native"}]}))
}

async fn native_messages_response(
    State(requests): State<Arc<Mutex<Vec<()>>>>,
    request: Request,
) -> Response {
    let (_parts, body) = request.into_parts();
    let body = to_bytes(body, 64 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert!(body["tools"].is_array());
    assert_eq!(body["tools"][0]["name"], "read_file");
    requests.lock().unwrap().push(());
    (
        StatusCode::OK,
        Json(json!({
            "id": "msg_native",
            "type": "message",
            "role": "assistant",
            "model": "claude-native",
            "content": [{
                "type": "tool_use",
                "id": "toolu_native",
                "name": "read_file",
                "input": {"path": "README.md"}
            }],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })),
    )
        .into_response()
}

async fn spawn_source_lifecycle_upstream(
) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/v1/models", get(source_lifecycle_models))
        .route("/v1/responses", post(source_lifecycle_response))
        .with_state(observed.clone());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("http://{address}"), observed, task)
}

async fn source_lifecycle_models(
    State(observed): State<Arc<Mutex<Vec<String>>>>,
    request: Request,
) -> impl IntoResponse {
    observe_source_authorization(&observed, &request);
    Json(json!({"data":[{"id":"gpt-source-lifecycle"}]}))
}

async fn source_lifecycle_response(
    State(observed): State<Arc<Mutex<Vec<String>>>>,
    request: Request,
) -> impl IntoResponse {
    observe_source_authorization(&observed, &request);
    Json(json!({
        "id":"response-source-lifecycle",
        "object":"response",
        "model":"gpt-source-lifecycle",
        "output":[],
        "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
    }))
}

fn observe_source_authorization(observed: &Mutex<Vec<String>>, request: &Request) {
    let authorization = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    observed.lock().unwrap().push(authorization);
}

#[derive(Clone)]
struct LoadUpstreamState {
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
    barrier: Arc<tokio::sync::Barrier>,
}

async fn spawn_load_upstream(
    requests: usize,
) -> (String, LoadUpstreamState, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = LoadUpstreamState {
        active: Arc::new(AtomicUsize::new(0)),
        max_active: Arc::new(AtomicUsize::new(0)),
        total: Arc::new(AtomicUsize::new(0)),
        barrier: Arc::new(tokio::sync::Barrier::new(requests)),
    };
    let router = Router::new()
        .route("/v1/models", get(models_response))
        .route("/v1/responses", post(load_upstream_response))
        .with_state(state.clone());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("http://{address}"), state, task)
}

async fn spawn_account_proxy(
    response_id: &'static str,
) -> (SocketAddr, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let marker = hits.clone();
    let router = Router::new().fallback(any(move |request: Request| {
        let marker = marker.clone();
        async move {
            marker.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.method(), axum::http::Method::POST);
            assert!(request
                .headers()
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("Bearer synthetic-proxy-access-token")));
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from(format!(
                    "data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"{response_id}\",\"object\":\"response\",\"model\":\"gpt-proxy-test\",\"output\":[],\"usage\":{{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}}}}\n\n"
                )))
                .unwrap()
        }
    }));
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (address, hits, task)
}

async fn models_response(request: Request) -> impl IntoResponse {
    assert_eq!(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer synthetic-upstream-api-key")
    );
    Json(json!({
        "data":[{
            "id":"gpt-test",
            "reasoningEffortModes":["low", "medium", "high"]
        }]
    }))
}

async fn scope_models_response(request: Request) -> impl IntoResponse {
    assert_eq!(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer synthetic-upstream-api-key")
    );
    Json(json!({"data":[{"id":"gpt-active"},{"id":"gpt-draining"}]}))
}

async fn upstream_response(request: Request) -> Response {
    assert_eq!(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer synthetic-upstream-api-key")
    );
    let body = to_bytes(request.into_body(), 64 * 1024).await.unwrap();
    let stream = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|value| value.get("stream").and_then(Value::as_bool))
        .unwrap_or(false);
    if stream {
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from("data: {\"type\":\"response.output_text.delta\",\"delta\":\"OK\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"))
            .unwrap();
    }
    (
        StatusCode::OK,
        Json(json!({
            "id":"response-test",
            "object":"response",
            "status":"completed",
            "model":"gpt-test",
            "error": null,
            "output":[],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        })),
    )
        .into_response()
}

async fn load_upstream_response(
    State(state): State<LoadUpstreamState>,
    request: Request,
) -> Response {
    assert_eq!(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer synthetic-upstream-api-key")
    );
    let _ = to_bytes(request.into_body(), 64 * 1024).await.unwrap();
    let active = state.active.fetch_add(1, Ordering::Relaxed) + 1;
    state.max_active.fetch_max(active, Ordering::Relaxed);
    state.total.fetch_add(1, Ordering::Relaxed);
    state.barrier.wait().await;
    state.active.fetch_sub(1, Ordering::Relaxed);
    (
        StatusCode::OK,
        Json(json!({
            "id":"concurrent-response",
            "object":"response",
            "model":"gpt-test",
            "output":[],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        })),
    )
        .into_response()
}

async fn account_response(request: Request) -> Response {
    assert_eq!(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer synthetic-access-token")
    );
    assert_eq!(
        request
            .headers()
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok()),
        Some("synthetic-chatgpt-account-id")
    );
    let body = to_bytes(request.into_body(), 64 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert!(body["input"].is_array());
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"OK\"}\n\ndata: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"message\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"account-response-test\",\"object\":\"response\",\"model\":\"gpt-test\",\"output\":[],\"usage\":{\"input_tokens\":1,\"input_tokens_details\":{\"cached_tokens\":1},\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        ))
        .unwrap()
}

async fn account_compact(request: Request) -> Response {
    assert_account_authorization(&request);
    let body = to_bytes(request.into_body(), 64 * 1024).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert!(body.get("stream").is_none());
    Json(json!({"type":"compaction","items":[]})).into_response()
}

async fn account_search(request: Request) -> Response {
    assert_account_authorization(&request);
    assert_eq!(
        request
            .headers()
            .get("x-session-id")
            .and_then(|value| value.to_str().ok()),
        Some("remote-session")
    );
    Json(json!({"results":[{"title":"remote result"}]})).into_response()
}

fn assert_account_authorization(request: &Request) {
    assert_eq!(
        request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer synthetic-access-token")
    );
    assert_eq!(
        request
            .headers()
            .get("chatgpt-account-id")
            .and_then(|value| value.to_str().ok()),
        Some("synthetic-chatgpt-account-id")
    );
}

async fn assert_websocket_upgrade(origin: &str, key: &str) {
    assert_websocket_status(origin, key, "101").await;
}

async fn assert_websocket_status(origin: &str, key: &str, status: &str) {
    let address = origin.strip_prefix("http://").unwrap();
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let request = format!(
        "GET /v1/responses HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {key}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = [0_u8; 1024];
    let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut response))
        .await
        .unwrap()
        .unwrap();
    let response = String::from_utf8_lossy(&response[..read]);
    assert!(
        response.starts_with(&format!("HTTP/1.1 {status} ")),
        "{response}"
    );
}
