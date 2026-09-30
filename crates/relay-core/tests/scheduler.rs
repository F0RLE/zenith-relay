use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, Response, StatusCode, Uri};
use axum::routing::{get, post};
use axum::Router;
use futures_util::stream;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use zenith_relay_core::gateway;
use zenith_relay_core::{
    DefaultServiceTier, GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey,
    MessagesReasoningMode, ProviderSource, RuntimeLocalKey, RuntimeSource, SourceAdapter,
    SourceProtocolBinding, UsageEvent, WireApi,
};

const LOCAL_KEY: &str = "p2-local-key";
const MODEL: &str = "gpt-p2";

#[path = "support/rotation_recovery.rs"]
mod rotation_recovery;

#[path = "support/cooldown_budget.rs"]
mod cooldown_budget;
#[path = "support/model_visibility.rs"]
mod model_visibility;
#[path = "support/pre_output_fallback.rs"]
mod pre_output_fallback;
#[path = "support/protocol_routes.rs"]
mod protocol_routes;
#[path = "support/stream_safety.rs"]
mod stream_safety;

#[derive(Clone, Debug)]
struct ObservedRequest {
    path: String,
    authorization: Option<String>,
    x_api_key: Option<String>,
    anthropic_version: Option<String>,
    anthropic_beta: Option<String>,
    claude_code_session_id: Option<String>,
    body: Value,
}

#[derive(Clone)]
enum Reply {
    Json {
        status: StatusCode,
        body: Value,
        cache_control: &'static str,
        retry_after: Option<&'static str>,
    },
    Oversized {
        status: StatusCode,
        cache_control: &'static str,
    },
    Stream {
        chunks: Vec<StreamChunk>,
        cache_control: &'static str,
    },
}

#[derive(Clone)]
enum StreamChunk {
    Data(&'static str),
    Error,
}

#[derive(Clone)]
struct UpstreamState {
    key: String,
    replies: Arc<Mutex<VecDeque<Reply>>>,
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
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

fn source(
    id: &str,
    server: &TestServer,
    key: &str,
    models: &[&str],
    priority: i32,
) -> RuntimeSource {
    RuntimeSource {
        source: ProviderSource {
            id: id.to_string(),
            name: id.to_string(),
            base_url: format!("{}/v1", server.base_url),
            api_key: key.to_string(),
            wire_api: WireApi::Responses,
            models: models.iter().map(|model| (*model).to_string()).collect(),
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

fn source_with_protocol(
    id: &str,
    server: &TestServer,
    key: &str,
    models: &[&str],
    priority: i32,
    wire_api: WireApi,
) -> RuntimeSource {
    let mut source = source(id, server, key, models, priority);
    source.source.wire_api = wire_api;
    source.protocol_config.endpoint_hint = Some(wire_api);
    source.protocol_bindings = vec![SourceProtocolBinding::legacy(
        wire_api,
        &source.source.models,
    )];
    source
}

fn local_key(id: &str, secret: &str, source_ids: Option<Vec<&str>>) -> RuntimeLocalKey {
    RuntimeLocalKey {
        key: LocalGatewayKey {
            id: id.to_string(),
            secret: secret.to_string(),
        },
        enabled: true,
        source_ids: source_ids.map(|ids| ids.into_iter().map(str::to_string).collect()),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        model_prefix: None,
    }
}

async fn spawn_gateway(
    sources: Vec<RuntimeSource>,
    keys: Vec<RuntimeLocalKey>,
    max_retry_candidates: usize,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    spawn_gateway_with_options(
        sources,
        keys,
        GatewayRuntimeOptions {
            model_metadata_catalog: None,
            max_retry_candidates,
            tool_policy: Default::default(),
            pool_routing: None,
            hidden_models: Vec::new(),
            default_service_tier: Default::default(),
            quota_stale_after_ms: zenith_relay_core::QUOTA_STALE_AFTER_MS,
            image_base_model: None,
            image_pricing_catalog: None,
            model_reasoning_allowed_levels: Default::default(),
            response_affinity_store: None,
        },
    )
    .await
}

async fn spawn_gateway_with_options(
    sources: Vec<RuntimeSource>,
    keys: Vec<RuntimeLocalKey>,
    mut options: GatewayRuntimeOptions,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    // Transport fixtures declare their intended order instead of relying on
    // the obsolete priority/quota score of Automatic mode.
    if options.pool_routing.is_none() {
        options.pool_routing = Some(ordered_policy(
            &sources
                .iter()
                .map(|source| source.source.id.as_str())
                .collect::<Vec<_>>(),
        ));
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let runtime = GatewayRuntime::from_pool(
        sources,
        keys,
        options,
        Arc::new(move |event| captured.lock().unwrap().push(event)),
    )
    .unwrap();
    (spawn(gateway::router(Arc::new(runtime))).await, events)
}

async fn spawn_upstream(key: &str, replies: Vec<Reply>) -> (TestServer, UpstreamState) {
    let state = UpstreamState {
        key: key.to_string(),
        replies: Arc::new(Mutex::new(replies.into())),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let router = Router::new()
        .route("/v1/models", get(upstream))
        .route("/v1/responses", post(upstream))
        .route("/v1/chat/completions", post(upstream))
        .route("/v1/messages", post(upstream))
        .with_state(state.clone());
    (spawn(router).await, state)
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

async fn upstream(
    State(state): State<UpstreamState>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let parsed_body = serde_json::from_slice(&body).unwrap_or(Value::Null);
    state.requests.lock().unwrap().push(ObservedRequest {
        path: uri.path().to_string(),
        authorization: headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        x_api_key: headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        anthropic_version: headers
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        anthropic_beta: headers
            .get("anthropic-beta")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        claude_code_session_id: headers
            .get("x-claude-code-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        body: parsed_body,
    });
    let authorized = if uri.path() == "/v1/messages" {
        headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            == Some(state.key.as_str())
    } else {
        let expected = format!("Bearer {}", state.key);
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            == Some(expected.as_str())
    };
    if !authorized {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::empty())
            .unwrap();
    }

    let reply = state
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| response_reply("default-response", "default"));
    match reply {
        Reply::Json {
            status,
            body,
            cache_control,
            retry_after,
        } => {
            let mut response = Response::builder()
                .status(status)
                .header(CONTENT_TYPE, "application/json")
                .header(CACHE_CONTROL, cache_control);
            if let Some(retry_after) = retry_after {
                response = response.header("retry-after", retry_after);
            }
            response.body(Body::from(body.to_string())).unwrap()
        }
        Reply::Oversized {
            status,
            cache_control,
        } => Response::builder()
            .status(status)
            .header(CONTENT_TYPE, "application/json")
            .header(CACHE_CONTROL, cache_control)
            .header("content-length", "16777217")
            .body(Body::empty())
            .unwrap(),
        Reply::Stream {
            chunks,
            cache_control,
        } => {
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
                .header(CACHE_CONTROL, cache_control)
                .body(Body::from_stream(chunks))
                .unwrap()
        }
    }
}

fn ordered_policy(ids: &[&str]) -> zenith_relay_core::PoolRoutingPolicy {
    use zenith_relay_core::{
        PoolMemberKind, PoolRoutingMember, PoolRoutingMode, PoolRoutingPolicy,
    };
    PoolRoutingPolicy {
        mode: PoolRoutingMode::InOrder,
        members: ids
            .iter()
            .map(|id| PoolRoutingMember {
                kind: PoolMemberKind::Source,
                id: (*id).into(),
                weight: 1,
                max_concurrency: 0,
            })
            .collect(),
        ..Default::default()
    }
}

fn overload_reply(cache_control: &'static str, retry_after: Option<&'static str>) -> Reply {
    Reply::Json {
        status: StatusCode::SERVICE_UNAVAILABLE,
        body: json!({"error":{"code":"server_is_overloaded"}}),
        cache_control,
        retry_after,
    }
}

fn status_reply(
    status: StatusCode,
    cache_control: &'static str,
    retry_after: Option<&'static str>,
) -> Reply {
    Reply::Json {
        status,
        body: json!({"error": {"message": status.as_str()}}),
        cache_control,
        retry_after,
    }
}

fn response_reply(id: &str, cache_control: &'static str) -> Reply {
    Reply::Json {
        status: StatusCode::OK,
        body: json!({
            "id": id,
            "object": "response",
            "model": MODEL,
            "status": "completed",
            "output": [],
            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
        }),
        cache_control,
        retry_after: None,
    }
}

async fn request(gateway: &TestServer, stream: bool) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "hello", "stream": stream}))
        .send()
        .await
        .unwrap()
}

async fn request_with_session(gateway: &TestServer, session: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-session-id", session)
        .json(&json!({"model": MODEL, "input": "hello"}))
        .send()
        .await
        .unwrap()
}

async fn models(gateway: &TestServer, key: &str) -> Vec<String> {
    let body: Value = reqwest::Client::new()
        .get(format!("{}/v1/models", gateway.base_url))
        .bearer_auth(key)
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
