use axum::body::{Body, Bytes};
use axum::extract::ws::{Message as AxumWsMessage, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, Response, StatusCode, Uri};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::future::{join_all, BoxFuture};
use futures_util::stream;
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message as ClientWsMessage, Upgrade};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::{Barrier, Notify};
use tokio::task::JoinHandle;
use zenith_relay_core::accounts::{
    AccountAuthState, ReauthReason, TokenAuthority, TokenPersistenceAdapter,
    TokenPersistenceFailure, TokenRefresh, TokenRefreshAdapter, TokenRefreshFailure, TokenSet,
};
use zenith_relay_core::gateway;
use zenith_relay_core::providers::chatgpt::{
    AgentIdentityCredential, CODEX_MODELS_CLIENT_VERSION, CODEX_ORIGINATOR,
};
use zenith_relay_core::{
    CandidateHealth, CandidateQuota, CandidateScope, DefaultServiceTier, GatewayRuntime,
    GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeChatGptAccount,
    RuntimeChatGptAuth, RuntimeMixedLocalKey, RuntimeSource, SelectionReason, SourceAdapter,
    SourceProtocolBinding, UsageEvent, WireApi,
};

const LOCAL_KEY: &str = "p3-local-key";
const MODEL: &str = "gpt-p3";
const OFFICIAL_CODEX_MODEL: &str = "gpt-5.6-terra";

fn reference_metadata_options() -> GatewayRuntimeOptions {
    use zenith_relay_core::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};
    let mut records = serde_json::Map::new();
    for model in [MODEL, OFFICIAL_CODEX_MODEL] {
        records.insert(
            format!("openai/{model}"),
            json!({
                "name": "Reference Model", "reasoning": true,
                "reasoning_effort_levels": ["low", "high", "xhigh"],
                "default_reasoning_effort": "high", "tool_call": true,
                "modalities": {"input": ["text", "image"], "output": ["text"]}
            }),
        );
    }
    GatewayRuntimeOptions {
        model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(
            ModelMetadataCatalog::from_models_dev_json(&Value::Object(records).to_string())
                .unwrap(),
        )),
        ..GatewayRuntimeOptions::default()
    }
}

#[path = "support/client_compatibility.rs"]
mod client_compatibility;
#[path = "support/compaction.rs"]
mod compaction;
#[path = "support/long_requests.rs"]
mod long_requests;
#[path = "support/rotation_policy.rs"]
mod rotation_policy;

#[path = "support/auth_refresh.rs"]
mod auth_refresh;
#[path = "support/catalog_projection.rs"]
mod catalog_projection;
#[path = "support/content_repair.rs"]
mod content_repair;
#[path = "support/continuation_replay.rs"]
mod continuation_replay;
#[path = "support/http_continuation.rs"]
mod http_continuation;
#[path = "support/http_rejection.rs"]
mod http_rejection;
#[path = "support/lite_and_wake.rs"]
mod lite_and_wake;
#[path = "support/model_identity.rs"]
mod model_identity;
#[path = "support/route_capacity.rs"]
mod route_capacity;
#[path = "support/route_fallback.rs"]
mod route_fallback;
#[path = "support/transport_matrix.rs"]
mod transport_matrix;
#[path = "support/websocket_fallback.rs"]
mod websocket_fallback;
#[path = "support/websocket_retry.rs"]
mod websocket_retry;
#[path = "support/websocket_session.rs"]
mod websocket_session;

#[derive(Clone, Debug)]
struct ObservedRequest {
    path: String,
    authorization: Option<String>,
    chatgpt_account_id: Option<String>,
    originator: Option<String>,
    responses_lite: Option<String>,
    session_id: Option<String>,
    turn_state: Option<String>,
    body: Value,
}

#[derive(Clone)]
enum Reply {
    Json(StatusCode, Value),
    JsonWithHeaders(StatusCode, Value, Vec<(&'static str, &'static str)>),
    Stream(Vec<StreamChunk>),
    RejectCompactTransportFields,
}

#[derive(Clone)]
enum StreamChunk {
    Data(&'static str),
    Error,
}

#[derive(Clone, Default)]
struct UpstreamState {
    replies: Arc<Mutex<VecDeque<Reply>>>,
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
    delay: Duration,
    request_barrier: Option<Arc<Barrier>>,
    model_catalog: Value,
}

#[derive(Clone, Default)]
struct HeldStreamState {
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
    release: Arc<Notify>,
}

#[derive(Clone, Default)]
struct HeldThenJsonState {
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
    release: Arc<Notify>,
}

#[derive(Clone, Default)]
struct ConnectionAffinityState {
    owners: Arc<Mutex<HashMap<SocketAddr, String>>>,
    account_ids: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone, Default)]
struct WebSocketUpstreamState {
    headers: Arc<Mutex<Vec<HeaderMap>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    behavior: WebSocketBehavior,
    model_catalog: Value,
}

#[derive(Clone, Default)]
enum WebSocketBehavior {
    #[default]
    Success,
    GatedSuccess(Arc<Barrier>),
    Events(Arc<Vec<Value>>),
    Sequence(Arc<Mutex<VecDeque<Vec<Value>>>>),
    Hold(Arc<Notify>),
    Close,
    OutputThenClose,
    SuccessThenSetupClose(Arc<AtomicUsize>),
    UnauthorizedOnce(Arc<AtomicUsize>),
}

struct TestServer {
    base_url: String,
    task: JoinHandle<()>,
    runtime: Option<Arc<GatewayRuntime>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct RefreshAdapter {
    calls: AtomicUsize,
    delay: Duration,
    access_token: &'static str,
}

impl TokenRefreshAdapter for RefreshAdapter {
    fn refresh<'a>(
        &'a self,
        account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            assert_eq!(account_id, "relay-refresh-account");
            assert_eq!(refresh_token, "refresh-secret");
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            TokenRefresh::new(self.access_token, None, None, Some(now_ms + 60_000))
                .map_err(|_| unreachable!())
        })
    }
}

#[derive(Default)]
struct PersistenceAdapter {
    token_writes: AtomicUsize,
    persisted_accounts: Mutex<Vec<String>>,
    auth_states: Mutex<Vec<(String, AccountAuthState)>>,
}

impl TokenPersistenceAdapter for PersistenceAdapter {
    fn persist<'a>(
        &'a self,
        account_id: &'a str,
        _tokens: &'a TokenSet,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            self.token_writes.fetch_add(1, Ordering::SeqCst);
            self.persisted_accounts
                .lock()
                .unwrap()
                .push(account_id.to_string());
            Ok(())
        })
    }

    fn persist_auth_state<'a>(
        &'a self,
        account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            self.auth_states
                .lock()
                .unwrap()
                .push((account_id.to_string(), auth_state));
            Ok(())
        })
    }

    fn persist_agent_task_id<'a>(
        &'a self,
        _account_id: &'a str,
        _expected_task_id: Option<&'a str>,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>> {
        Box::pin(async move { Ok(task_id.to_string()) })
    }
}

fn account(
    id: &str,
    chatgpt_account_id: &str,
    server: &TestServer,
    quota_remaining_basis_points: i32,
) -> RuntimeChatGptAccount {
    RuntimeChatGptAccount {
        oauth_client_kind: Default::default(),
        id: id.to_string(),
        source_id: "openai-codex".to_string(),
        chatgpt_account_id: chatgpt_account_id.to_string(),
        chatgpt_user_id: None,
        responses_url: format!("{}/v1/responses", server.base_url),
        basis_points_enabled: false,
        basis_points_headers: None,
        models: vec![MODEL.to_string()],
        enabled: true,
        draining: false,
        priority: 0,
        weight: 1,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        health: CandidateHealth::Healthy,
        quota: u64::try_from(quota_remaining_basis_points)
            .ok()
            .filter(|remaining| *remaining > 0)
            .map_or(CandidateQuota::Unknown, CandidateQuota::Available),
        quota_updated_at_ms: None,
        quota_snapshot: Default::default(),
        subscription_plan_type: None,
        subscription_expires_at_ms: None,
        last_used_at_ms: None,
        cooldowns: Default::default(),
        consecutive_failures: 0,
        proxy: None,
    }
}

fn source(id: &str, server: &TestServer, key: &str, priority: i32) -> RuntimeSource {
    RuntimeSource {
        source: ProviderSource {
            id: id.to_string(),
            name: id.to_string(),
            base_url: format!("{}/v1", server.base_url),
            api_key: key.to_string(),
            wire_api: WireApi::Responses,
            models: vec![MODEL.to_string()],
        },
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        enabled: true,
        draining: false,
        priority,
        weight: 1,
        recovery_delay_seconds: 0,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        last_used_at_ms: None,
    }
}

fn mixed_key(
    source_ids: Option<Vec<&str>>,
    account_ids: Option<Vec<&str>>,
) -> RuntimeMixedLocalKey {
    RuntimeMixedLocalKey {
        key: LocalGatewayKey {
            id: "local-key".to_string(),
            secret: LOCAL_KEY.to_string(),
        },
        enabled: true,
        source_ids: source_ids.map(|ids| ids.into_iter().map(str::to_string).collect()),
        account_ids: account_ids.map(|ids| ids.into_iter().map(str::to_string).collect()),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        model_prefix: None,
        wire_apis: None,
    }
}

fn refresh_adapter() -> Arc<RefreshAdapter> {
    Arc::new(RefreshAdapter {
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
        access_token: "unused-access",
    })
}

async fn ready_authority(account_id: &str, access_token: &str) -> Arc<TokenAuthority> {
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, account_id, access_token).await;
    authority
}

async fn register_ready(authority: &TokenAuthority, account_id: &str, access_token: &str) {
    authority
        .register(
            account_id,
            TokenSet::access_only(access_token, None, 0).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
}

async fn spawn_mixed_gateway(
    sources: Vec<RuntimeSource>,
    accounts: Vec<RuntimeChatGptAccount>,
    keys: Vec<RuntimeMixedLocalKey>,
    authority: Arc<TokenAuthority>,
    refresh: Arc<RefreshAdapter>,
    persistence: Arc<PersistenceAdapter>,
) -> (
    TestServer,
    Arc<Mutex<Vec<UsageEvent>>>,
    Arc<RefreshAdapter>,
    Arc<PersistenceAdapter>,
) {
    spawn_mixed_gateway_with_options(
        sources,
        accounts,
        keys,
        authority,
        refresh,
        persistence,
        GatewayRuntimeOptions::default(),
    )
    .await
}

async fn spawn_mixed_gateway_with_options(
    sources: Vec<RuntimeSource>,
    accounts: Vec<RuntimeChatGptAccount>,
    keys: Vec<RuntimeMixedLocalKey>,
    authority: Arc<TokenAuthority>,
    refresh: Arc<RefreshAdapter>,
    persistence: Arc<PersistenceAdapter>,
    options: GatewayRuntimeOptions,
) -> (
    TestServer,
    Arc<Mutex<Vec<UsageEvent>>>,
    Arc<RefreshAdapter>,
    Arc<PersistenceAdapter>,
) {
    spawn_mixed_gateway_with_agent_identities_and_options(
        sources,
        accounts,
        keys,
        authority,
        refresh,
        persistence,
        options,
        HashMap::new(),
    )
    .await
}

async fn spawn_mixed_gateway_with_agent_identities(
    sources: Vec<RuntimeSource>,
    accounts: Vec<RuntimeChatGptAccount>,
    keys: Vec<RuntimeMixedLocalKey>,
    authority: Arc<TokenAuthority>,
    refresh: Arc<RefreshAdapter>,
    persistence: Arc<PersistenceAdapter>,
    agent_identities: HashMap<String, AgentIdentityCredential>,
) -> (
    TestServer,
    Arc<Mutex<Vec<UsageEvent>>>,
    Arc<RefreshAdapter>,
    Arc<PersistenceAdapter>,
) {
    spawn_mixed_gateway_with_agent_identities_and_options(
        sources,
        accounts,
        keys,
        authority,
        refresh,
        persistence,
        GatewayRuntimeOptions::default(),
        agent_identities,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn spawn_mixed_gateway_with_agent_identities_and_options(
    sources: Vec<RuntimeSource>,
    accounts: Vec<RuntimeChatGptAccount>,
    keys: Vec<RuntimeMixedLocalKey>,
    authority: Arc<TokenAuthority>,
    refresh: Arc<RefreshAdapter>,
    persistence: Arc<PersistenceAdapter>,
    options: GatewayRuntimeOptions,
    agent_identities: HashMap<String, AgentIdentityCredential>,
) -> (
    TestServer,
    Arc<Mutex<Vec<UsageEvent>>>,
    Arc<RefreshAdapter>,
    Arc<PersistenceAdapter>,
) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let runtime = Arc::new(
        GatewayRuntime::from_mixed_pool(
            sources,
            accounts,
            keys,
            RuntimeChatGptAuth {
                token_authority: authority,
                refresh_adapter: refresh.clone(),
                persistence_adapter: persistence.clone(),
                refresh_skew_ms: 0,
                agent_identities,
            },
            options,
            Arc::new(move |event| captured.lock().unwrap().push(event)),
        )
        .unwrap(),
    );
    let mut server = spawn(gateway::router(runtime.clone())).await;
    server.runtime = Some(runtime);
    (server, events, refresh, persistence)
}

async fn spawn_upstream(replies: Vec<Reply>) -> (TestServer, UpstreamState) {
    spawn_delayed_upstream_with_replies(replies, Duration::ZERO).await
}

async fn spawn_gated_upstream(
    replies: Vec<Reply>,
    request_barrier: Arc<Barrier>,
) -> (TestServer, UpstreamState) {
    spawn_upstream_with_catalog_and_delay_and_barrier(
        replies,
        default_upstream_model_catalog(),
        Duration::ZERO,
        Some(request_barrier),
    )
    .await
}

async fn spawn_upstream_with_catalog(
    replies: Vec<Reply>,
    model_catalog: Value,
) -> (TestServer, UpstreamState) {
    spawn_upstream_with_catalog_and_delay(replies, model_catalog, Duration::ZERO).await
}

async fn spawn_delayed_upstream_with_replies(
    replies: Vec<Reply>,
    delay: Duration,
) -> (TestServer, UpstreamState) {
    spawn_upstream_with_catalog_and_delay(replies, default_upstream_model_catalog(), delay).await
}

async fn spawn_upstream_with_catalog_and_delay(
    replies: Vec<Reply>,
    model_catalog: Value,
    delay: Duration,
) -> (TestServer, UpstreamState) {
    spawn_upstream_with_catalog_and_delay_and_barrier(replies, model_catalog, delay, None).await
}

async fn spawn_upstream_with_catalog_and_delay_and_barrier(
    replies: Vec<Reply>,
    model_catalog: Value,
    delay: Duration,
    request_barrier: Option<Arc<Barrier>>,
) -> (TestServer, UpstreamState) {
    let state = UpstreamState {
        replies: Arc::new(Mutex::new(replies.into())),
        requests: Arc::new(Mutex::new(Vec::new())),
        delay,
        request_barrier,
        model_catalog,
    };
    let app = Router::new()
        .route("/v1/models", get(upstream_models))
        .route("/v1/responses", post(upstream))
        .route("/v1/responses/compact", post(upstream))
        .route("/v1/alpha/search", post(upstream))
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_websocket_upstream() -> (TestServer, WebSocketUpstreamState) {
    spawn_websocket_upstream_with_behavior(WebSocketBehavior::Success).await
}

async fn spawn_replayable_websocket_upstream() -> (TestServer, WebSocketUpstreamState) {
    spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![
        json!({"type":"response.completed","response":{
            "id":"replayable-ws-response",
            "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic answer"}]}]
        }}),
    ]))).await
}

async fn spawn_websocket_upstream_with_behavior(
    behavior: WebSocketBehavior,
) -> (TestServer, WebSocketUpstreamState) {
    spawn_websocket_upstream_with_catalog(behavior, default_upstream_model_catalog()).await
}

async fn spawn_websocket_upstream_with_catalog(
    behavior: WebSocketBehavior,
    model_catalog: Value,
) -> (TestServer, WebSocketUpstreamState) {
    let state = WebSocketUpstreamState {
        behavior,
        model_catalog,
        ..WebSocketUpstreamState::default()
    };
    let app = Router::new()
        .route("/v1/models", get(upstream_websocket_models))
        .route("/v1/responses", get(upstream_websocket))
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_delayed_upstream(delay: Duration) -> (TestServer, UpstreamState) {
    spawn_delayed_upstream_with_replies(Vec::new(), delay).await
}

async fn spawn_held_stream_upstream() -> (TestServer, HeldStreamState) {
    let state = HeldStreamState::default();
    let app = Router::new()
        .route("/v1/responses", post(held_stream_upstream))
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_held_then_json_upstream() -> (TestServer, HeldThenJsonState) {
    let state = HeldThenJsonState::default();
    let app = Router::new()
        .route("/v1/responses", post(held_then_json_upstream))
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_connection_affinity_upstream() -> (TestServer, ConnectionAffinityState) {
    let state = ConnectionAffinityState::default();
    let app = Router::new()
        .route("/v1/responses", post(connection_affinity_upstream))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (
        TestServer {
            base_url: format!("http://{address}"),
            task,
            runtime: None,
        },
        state,
    )
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
        runtime: None,
    }
}

async fn upstream(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response<Body> {
    let body_value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    state.requests.lock().unwrap().push(ObservedRequest {
        path: uri.path().to_string(),
        authorization: header(&headers, AUTHORIZATION.as_str()),
        chatgpt_account_id: header(&headers, "chatgpt-account-id"),
        originator: header(&headers, "originator"),
        responses_lite: header(&headers, "x-openai-internal-codex-responses-lite"),
        session_id: header(&headers, "x-session-id"),
        turn_state: header(&headers, "x-codex-turn-state"),
        body: body_value.clone(),
    });
    if let Some(request_barrier) = &state.request_barrier {
        request_barrier.wait().await;
    }
    tokio::time::sleep(state.delay).await;
    match state
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| {
            success_reply_for_model(
                "default-response",
                body_value["model"].as_str().unwrap_or(MODEL),
            )
        }) {
        Reply::Json(status, body) => Response::builder()
            .status(status)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        Reply::JsonWithHeaders(status, body, headers) => {
            let mut response = Response::builder()
                .status(status)
                .header(CONTENT_TYPE, "application/json");
            for (name, value) in headers {
                response = response.header(name, value);
            }
            response.body(Body::from(body.to_string())).unwrap()
        }
        Reply::Stream(chunks) => {
            let chunks = stream::unfold(VecDeque::from(chunks), |mut chunks| async move {
                let chunk = chunks.pop_front()?;
                tokio::time::sleep(Duration::from_millis(10)).await;
                let item = match chunk {
                    StreamChunk::Data(data) => {
                        Ok::<_, io::Error>(Bytes::from_static(data.as_bytes()))
                    }
                    StreamChunk::Error => Err(io::Error::other("synthetic stream failure")),
                };
                Some((item, chunks))
            });
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(chunks))
                .unwrap()
        }
        Reply::RejectCompactTransportFields => {
            let has_compact_transport_field = uri.path() == "/v1/responses/compact"
                && (body_value.get("store").is_some() || body_value.get("stream").is_some());
            if has_compact_transport_field {
                Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "error": {
                                "code": "unsupported_compact_transport_field",
                                "message": "compact does not accept store or stream"
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap()
            } else {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"type": "compaction", "items": []}).to_string(),
                    ))
                    .unwrap()
            }
        }
    }
}

async fn held_stream_upstream(
    State(state): State<HeldStreamState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response<Body> {
    state.requests.lock().unwrap().push(ObservedRequest {
        path: uri.path().to_string(),
        authorization: header(&headers, AUTHORIZATION.as_str()),
        chatgpt_account_id: header(&headers, "chatgpt-account-id"),
        originator: header(&headers, "originator"),
        responses_lite: header(&headers, "x-openai-internal-codex-responses-lite"),
        session_id: header(&headers, "x-session-id"),
        turn_state: header(&headers, "x-codex-turn-state"),
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    });
    held_stream_response(state.release)
}

async fn held_then_json_upstream(
    State(state): State<HeldThenJsonState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response<Body> {
    let request_count = {
        let mut requests = state.requests.lock().unwrap();
        requests.push(ObservedRequest {
            path: uri.path().to_string(),
            authorization: header(&headers, AUTHORIZATION.as_str()),
            chatgpt_account_id: header(&headers, "chatgpt-account-id"),
            originator: header(&headers, "originator"),
            responses_lite: header(&headers, "x-openai-internal-codex-responses-lite"),
            session_id: header(&headers, "x-session-id"),
            turn_state: header(&headers, "x-codex-turn-state"),
            body: serde_json::from_slice(&body).unwrap_or(Value::Null),
        });
        requests.len()
    };

    if request_count == 1 {
        held_stream_response(state.release)
    } else {
        Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "id": "queued-response",
                    "object": "response",
                    "model": MODEL,
                    "output": [],
                    "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                })
                .to_string(),
            ))
            .unwrap()
    }
}

fn held_stream_response(release: Arc<Notify>) -> Response<Body> {
    let chunks = stream::unfold(0_u8, move |step| {
        let release = release.clone();
        async move {
            let chunk = match step {
                0 => Bytes::from_static(
                    b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"first\"}\n\n",
                ),
                1 => {
                    release.notified().await;
                    Bytes::from_static(b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"held-response\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n")
                }
                2 => Bytes::from_static(b"data: [DONE]\n\n"),
                _ => return None,
            };
            Some((Ok::<_, io::Error>(chunk), step + 1))
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(chunks))
        .unwrap()
}

async fn connection_affinity_upstream(
    State(state): State<ConnectionAffinityState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response<Body> {
    let account_id = header(&headers, "chatgpt-account-id").unwrap_or_default();
    state.account_ids.lock().unwrap().push(account_id.clone());
    let conflict = {
        let mut owners = state.owners.lock().unwrap();
        match owners.entry(peer) {
            std::collections::hash_map::Entry::Occupied(owner) => owner.get() != &account_id,
            std::collections::hash_map::Entry::Vacant(owner) => {
                owner.insert(account_id);
                false
            }
        }
    };
    let (status, body) = if conflict {
        (
            StatusCode::BAD_GATEWAY,
            json!({"error": {"code": "connection_identity_conflict"}}),
        )
    } else {
        (
            StatusCode::OK,
            json!({
                "id": "isolated-response",
                "object": "response",
                "model": MODEL,
                "output": [],
                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
            }),
        )
    };
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn upstream_websocket(
    State(state): State<WebSocketUpstreamState>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response<Body> {
    state.headers.lock().unwrap().push(headers);
    if let WebSocketBehavior::UnauthorizedOnce(attempts) = &state.behavior {
        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"error":{"code":"token_expired"}}).to_string(),
                ))
                .unwrap();
        }
    }
    websocket.on_upgrade(move |mut socket| async move {
        while let Some(Ok(message)) = socket.recv().await {
            let request = match message {
                AxumWsMessage::Text(text) => serde_json::from_slice(text.as_bytes()),
                AxumWsMessage::Binary(bytes) => serde_json::from_slice(&bytes),
                AxumWsMessage::Close(_) => break,
                AxumWsMessage::Ping(payload) => {
                    if socket.send(AxumWsMessage::Pong(payload)).await.is_err() {
                        break;
                    }
                    continue;
                }
                AxumWsMessage::Pong(_) => continue,
            };
            let Ok(request) = request else {
                break;
            };
            state.requests.lock().unwrap().push(request);
            if let WebSocketBehavior::GatedSuccess(barrier) = &state.behavior {
                barrier.wait().await;
            }
            let setup_only =
                if let WebSocketBehavior::SuccessThenSetupClose(attempts) = &state.behavior {
                    attempts.fetch_add(1, Ordering::SeqCst) > 0
                } else {
                    false
                };
            let events = match &state.behavior {
                WebSocketBehavior::Success
                | WebSocketBehavior::GatedSuccess(_)
                | WebSocketBehavior::UnauthorizedOnce(_) => vec![
                    json!({"type": "response.output_text.delta", "delta": "hello"}),
                    json!({
                        "type": "response.completed",
                        "response": {
                            "id": "ws-response",
                            "usage": {
                                "input_tokens": 11,
                                "input_tokens_details": {"cached_tokens": 7},
                                "output_tokens": 5,
                                "output_tokens_details": {"reasoning_tokens": 2},
                                "total_tokens": 16
                            }
                        }
                    }),
                ],
                WebSocketBehavior::SuccessThenSetupClose(_) if !setup_only => vec![
                    json!({"type": "response.output_text.delta", "delta": "hello"}),
                    json!({
                        "type": "response.completed",
                        "response": {
                            "id": "ws-response",
                            "usage": {
                                "input_tokens": 11,
                                "input_tokens_details": {"cached_tokens": 7},
                                "output_tokens": 5,
                                "output_tokens_details": {"reasoning_tokens": 2},
                                "total_tokens": 16
                            }
                        }
                    }),
                ],
                WebSocketBehavior::SuccessThenSetupClose(_) => {
                    vec![json!({"type": "response.created", "response": {"id": "setup-only"}})]
                }
                WebSocketBehavior::Events(events) => events.as_ref().clone(),
                WebSocketBehavior::Sequence(events) => {
                    events.lock().unwrap().pop_front().unwrap_or_default()
                }
                WebSocketBehavior::Hold(release) => {
                    if socket
                        .send(AxumWsMessage::Text(
                            json!({"type": "response.output_text.delta", "delta": "held"})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    release.notified().await;
                    vec![json!({
                        "type": "response.completed",
                        "response": {
                            "id": "ws-held-response",
                            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                        }
                    })]
                }
                WebSocketBehavior::Close => {
                    let _ = socket.send(AxumWsMessage::Close(None)).await;
                    return;
                }
                WebSocketBehavior::OutputThenClose => {
                    vec![json!({"type": "response.output_text.delta", "delta": "partial"})]
                }
            };
            for event in events {
                tokio::time::sleep(Duration::from_millis(2)).await;
                if socket
                    .send(AxumWsMessage::Text(event.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            if setup_only {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            if setup_only || matches!(&state.behavior, WebSocketBehavior::OutputThenClose) {
                let _ = socket.send(AxumWsMessage::Close(None)).await;
                return;
            }
        }
    })
}

async fn upstream_websocket_models(
    State(state): State<WebSocketUpstreamState>,
    uri: Uri,
) -> (StatusCode, Json<Value>) {
    let client_version = uri
        .query()
        .and_then(|query| query.strip_prefix("client_version="))
        .unwrap_or_default();
    let status = if client_version == CODEX_MODELS_CLIENT_VERSION {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, Json(state.model_catalog))
}

async fn receive_websocket_json(socket: &mut reqwest_websocket::WebSocket) -> Value {
    receive_websocket_json_with_timeout(socket, Duration::from_secs(2)).await
}

async fn receive_websocket_json_with_timeout(
    socket: &mut reqwest_websocket::WebSocket,
    timeout_duration: Duration,
) -> Value {
    loop {
        let message = tokio::time::timeout(timeout_duration, socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let ClientWsMessage::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

async fn receive_websocket_completion(socket: &mut reqwest_websocket::WebSocket) -> Value {
    receive_websocket_completion_with_timeout(socket, Duration::from_secs(2)).await
}

async fn receive_websocket_completion_with_timeout(
    socket: &mut reqwest_websocket::WebSocket,
    timeout_duration: Duration,
) -> Value {
    loop {
        let value = receive_websocket_json_with_timeout(socket, timeout_duration).await;
        if value["type"] == "response.completed" {
            return value;
        }
    }
}

async fn upstream_models(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> (StatusCode, Json<Value>) {
    let client_version = uri
        .query()
        .and_then(|query| query.strip_prefix("client_version="))
        .unwrap_or_default();
    state.requests.lock().unwrap().push(ObservedRequest {
        path: uri.path().to_string(),
        authorization: header(&headers, AUTHORIZATION.as_str()),
        chatgpt_account_id: header(&headers, "chatgpt-account-id"),
        originator: header(&headers, "originator"),
        responses_lite: header(&headers, "x-openai-internal-codex-responses-lite"),
        session_id: header(&headers, "x-session-id"),
        turn_state: header(&headers, "x-codex-turn-state"),
        body: json!({ "client_version": client_version }),
    });
    let status = if client_version == CODEX_MODELS_CLIENT_VERSION {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, Json(state.model_catalog))
}

fn default_upstream_model_catalog() -> Value {
    json!({
        "models": [{
            "slug": MODEL,
            "display_name": "GPT P3",
            "visibility": "list",
            "supported_in_api": true,
            "service_tiers": [{
                "id": "priority",
                "name": "Fast",
                "description": "Synthetic fast tier"
            }],
            "additional_speed_tiers": ["fast"],
            "default_service_tier": "priority",
            "use_responses_lite": true,
            "supports_parallel_tool_calls": true,
            "default_reasoning_level": "high",
            "supported_reasoning_levels": [
                {"effort": "low", "description": "Low"},
                {"effort": "high", "description": "High"},
                {"effort": "xhigh", "description": "Extra high"}
            ],
            "supports_reasoning_summary_parameter": true,
            "supports_reasoning_summaries": true,
            "default_reasoning_summary": "detailed"
        }]
    })
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn success_reply(id: &str) -> Reply {
    success_reply_for_model(id, MODEL)
}

fn success_reply_for_model(id: &str, model: &str) -> Reply {
    Reply::Json(
        StatusCode::OK,
        json!({
            "id": id,
            "object": "response",
            "model": model,
            "output": [],
            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
        }),
    )
}

fn successful_sse_reply() -> Reply {
    Reply::Stream(vec![
        StreamChunk::Data(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
        ),
        StreamChunk::Data(
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        ),
        StreamChunk::Data("data: [DONE]\n\n"),
    ])
}

async fn request(gateway: &TestServer, stream: bool) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "hello",
            "stream": stream,
            "max_output_tokens": 16
        }))
        .send()
        .await
        .unwrap()
}

async fn models(gateway: &TestServer) -> Vec<String> {
    let body: Value = reqwest::Client::new()
        .get(format!("{}/v1/models", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model["id"].as_str().map(str::to_string))
        .collect()
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
