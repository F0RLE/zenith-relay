use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HOST};
use axum::http::{HeaderMap, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{stream, SinkExt, StreamExt};
use reqwest_websocket::{Message as ClientWsMessage, Upgrade};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use zenith_relay_core::gateway;
use zenith_relay_core::{
    discover_source_models, discover_source_models_and_protocol_bindings,
    discover_source_models_for_protocol_bindings, discover_source_with_protocol_config,
    CapabilityOrigin, CapabilityStatus, ErrorOrigin, GatewayRuntime, GatewayRuntimeOptions,
    LocalGatewayKey, MessagesReasoningMode, ModelEndpointCapability, ProviderSource,
    RuntimeLocalKey, RuntimeSource, SourceAdapter, SourceProtocolBinding, SourceProtocolConfig,
    UsageEvent, WireApi,
};

const LOCAL_KEY: &str = "local-test-key";
const SOURCE_KEY: &str = "upstream-test-key";
const OVERSIZED_MODELS_CONTENT_LENGTH: &str = "4194305";
const MAX_CLIENT_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

#[path = "support/protocol_matrix.rs"]
mod protocol_matrix;

#[path = "support/native_admission.rs"]
mod native_admission;

#[path = "support/cache_context.rs"]
mod cache_context;

#[path = "support/tool_policy.rs"]
mod tool_policy;

#[path = "support/error_recording.rs"]
mod error_recording;
#[path = "support/native_tool_repair.rs"]
mod native_tool_repair;
#[path = "support/protocol_bridges.rs"]
mod protocol_bridges;

#[path = "support/local_server.rs"]
mod local_server;
use local_server::{spawn, TestServer};
#[path = "support/source_discovery.rs"]
mod source_discovery;
#[path = "support/stream_limits.rs"]
mod stream_limits;
#[path = "support/websocket_admission.rs"]
mod websocket_admission;

#[derive(Clone, Debug)]
struct ObservedRequest {
    path: &'static str,
    authorization: Option<String>,
    x_api_key: Option<String>,
    x_goog_api_key: Option<String>,
    anthropic_version: Option<String>,
    x_oai_attestation: Option<String>,
}

#[derive(Clone, Default)]
struct UpstreamState {
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
    bodies: Arc<Mutex<Vec<Value>>>,
    release_stream: Arc<Notify>,
}

#[derive(Clone, Default)]
struct NativeReplayUpstreamState {
    bodies: Arc<Mutex<Vec<Value>>>,
    rejection: NativeReplayRejection,
}

#[derive(Clone, Copy, Default)]
enum NativeReplayRejection {
    #[default]
    PreviousResponseRequiresWebsocket,
    InvalidFunctionCallOutputCallId,
    GenericInvalidRequest,
    ZenithGatewayInvalidRequest,
    ZenithGatewayInvalidRequestStream,
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

async fn prime_source_metadata(gateway: &TestServer) {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/v1/models?client_version=1.0.0",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
}

async fn spawn_messages_bridge_gateway(
    upstream_base_url: &str,
    state: &UpstreamState,
    reasoning_mode: MessagesReasoningMode,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let usage_events = events.clone();
    let source = ProviderSource {
        id: "messages-source".to_string(),
        name: "Synthetic Messages source".to_string(),
        base_url: format!("{upstream_base_url}/v1"),
        api_key: SOURCE_KEY.to_string(),
        wire_api: WireApi::Messages,
        models: vec!["claude-test".to_string()],
    };
    let mut options = GatewayRuntimeOptions::default();
    if reasoning_mode != MessagesReasoningMode::Disabled {
        options
            .model_reasoning_allowed_levels
            .insert("claude-test".to_string(), vec!["high".to_string()]);
    }
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource {
            source,
            protocol_config: SourceProtocolConfig {
                endpoint_hint: Some(WireApi::Messages),
                ..SourceProtocolConfig::default()
            },
            protocol_bindings: vec![SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".to_string()],
            }],
            enabled: true,
            draining: false,
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            last_used_at_ms: None,
        }],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "local-key-1".to_string(),
            secret: LOCAL_KEY.to_string(),
        })],
        options,
        Arc::new(move |event| usage_events.lock().unwrap().push(event)),
    )
    .unwrap();
    let gateway = spawn(gateway::router(Arc::new(runtime))).await;
    prime_source_metadata(&gateway).await;
    state.requests.lock().unwrap().clear();
    state.bodies.lock().unwrap().clear();
    (gateway, events)
}

async fn spawn_mixed_responses_gateway(
    upstream_base_url: &str,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let usage_events = events.clone();
    let source = ProviderSource {
        id: "mixed-source".to_string(),
        name: "Synthetic mixed source".to_string(),
        base_url: format!("{upstream_base_url}/v1"),
        api_key: SOURCE_KEY.to_string(),
        wire_api: WireApi::Responses,
        models: vec!["gpt-test".to_string(), "claude-test".to_string()],
    };
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource {
            source,
            protocol_config: SourceProtocolConfig {
                capabilities: vec![
                    ModelEndpointCapability {
                        model_id: "gpt-test".to_string(),
                        upstream_wire_api: WireApi::Responses,
                        status: CapabilityStatus::Declared,
                        origin: CapabilityOrigin::Catalog,
                        checked_at_ms: 1,
                        features: Default::default(),
                        reasoning_efforts: Vec::new(),
                    },
                    ModelEndpointCapability {
                        model_id: "claude-test".to_string(),
                        upstream_wire_api: WireApi::Messages,
                        status: CapabilityStatus::Declared,
                        origin: CapabilityOrigin::Catalog,
                        checked_at_ms: 1,
                        features: Default::default(),
                        reasoning_efforts: Vec::new(),
                    },
                ],
                endpoint_hint: None,
                ..SourceProtocolConfig::default()
            },
            protocol_bindings: vec![
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
                    model_ids: vec!["gpt-test".to_string()],
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::ResponsesToMessages,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
                    model_ids: vec!["claude-test".to_string()],
                },
            ],
            enabled: true,
            draining: false,
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            last_used_at_ms: None,
        }],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "local-key-1".to_string(),
            secret: LOCAL_KEY.to_string(),
        })],
        GatewayRuntimeOptions::default(),
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
        .route("/v1/messages", post(upstream_messages))
        .layer(DefaultBodyLimit::max(MAX_CLIENT_REQUEST_BODY_BYTES))
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_mixed_catalog_upstream() -> TestServer {
    spawn(Router::new().route("/v1/models", get(upstream_models_with_shared_catalog))).await
}

async fn spawn_messages_upstream() -> (TestServer, UpstreamState) {
    let state = UpstreamState::default();
    let app = Router::new()
        .route("/v1/models", get(upstream_models))
        .route("/v1/messages", post(upstream_messages))
        .layer(DefaultBodyLimit::max(MAX_CLIENT_REQUEST_BODY_BYTES))
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_gemini_bridge_gateway(
    upstream_base_url: &str,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let usage_events = events.clone();
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource {
            source: ProviderSource {
                id: "gemini-source".to_string(),
                name: "Synthetic Gemini source".to_string(),
                base_url: format!("{upstream_base_url}/v1"),
                api_key: SOURCE_KEY.to_string(),
                wire_api: WireApi::Gemini,
                models: vec!["gemini-test".to_string()],
            },
            protocol_config: SourceProtocolConfig {
                endpoint_hint: Some(WireApi::Gemini),
                ..SourceProtocolConfig::default()
            },
            protocol_bindings: vec![SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToGemini,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gemini-test".to_string()],
            }],
            enabled: true,
            draining: false,
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            last_used_at_ms: None,
        }],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "local-key-1".to_string(),
            secret: LOCAL_KEY.to_string(),
        })],
        GatewayRuntimeOptions::default(),
        Arc::new(move |event| usage_events.lock().unwrap().push(event)),
    )
    .unwrap();
    (spawn(gateway::router(Arc::new(runtime))).await, events)
}

async fn spawn_native_gemini_gateway(
    upstream_base_url: &str,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let usage_events = events.clone();
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource {
            source: ProviderSource {
                id: "native-gemini-source".to_string(),
                name: "Synthetic native Gemini source".to_string(),
                base_url: format!("{upstream_base_url}/v1"),
                api_key: SOURCE_KEY.to_string(),
                wire_api: WireApi::Gemini,
                models: vec!["gemini-test".to_string()],
            },
            protocol_config: Default::default(),
            protocol_bindings: vec![SourceProtocolBinding {
                wire_api: WireApi::Gemini,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gemini-test".to_string()],
            }],
            enabled: true,
            draining: false,
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            last_used_at_ms: None,
        }],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "local-key-1".to_string(),
            secret: LOCAL_KEY.to_string(),
        })],
        GatewayRuntimeOptions::default(),
        Arc::new(move |event| usage_events.lock().unwrap().push(event)),
    )
    .unwrap();
    (spawn(gateway::router(Arc::new(runtime))).await, events)
}

async fn spawn_gemini_upstream() -> (TestServer, UpstreamState) {
    let state = UpstreamState::default();
    let app = Router::new()
        .route(
            "/v1/models/gemini-test:generateContent",
            post(upstream_gemini_generate_content),
        )
        .route(
            "/v1/models/gemini-test:streamGenerateContent",
            post(upstream_gemini_stream_generate_content),
        )
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_native_replay_upstream() -> (TestServer, NativeReplayUpstreamState) {
    spawn_native_replay_upstream_with_rejection(
        NativeReplayRejection::PreviousResponseRequiresWebsocket,
    )
    .await
}

async fn spawn_native_replay_upstream_with_rejection(
    rejection: NativeReplayRejection,
) -> (TestServer, NativeReplayUpstreamState) {
    let state = NativeReplayUpstreamState {
        rejection,
        ..NativeReplayUpstreamState::default()
    };
    let app = Router::new()
        .route("/v1/responses", post(native_replay_upstream_responses))
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_strict_function_item_id_upstream() -> (TestServer, UpstreamState) {
    let state = UpstreamState::default();
    let app = Router::new()
        .route(
            "/v1/responses",
            post(strict_function_item_id_upstream_responses),
        )
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_strict_custom_tool_item_id_upstream() -> (TestServer, UpstreamState) {
    let state = UpstreamState::default();
    let app = Router::new()
        .route(
            "/v1/responses",
            post(strict_custom_tool_item_id_upstream_responses),
        )
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_strict_message_item_id_upstream() -> (TestServer, UpstreamState) {
    let state = UpstreamState::default();
    let app = Router::new()
        .route(
            "/v1/responses",
            post(strict_message_item_id_upstream_responses),
        )
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_strict_missing_call_id_upstream() -> (TestServer, UpstreamState) {
    let state = UpstreamState::default();
    let app = Router::new()
        .route(
            "/v1/responses",
            post(strict_missing_call_id_upstream_responses),
        )
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn upstream_models(State(state): State<UpstreamState>, headers: HeaderMap) -> Response<Body> {
    observe(&state, "/v1/models", &headers);
    if has_source_key(&headers) {
        return Json(json!({
            "object": "list",
            "data": [
                {"id": "gpt-test", "object": "model"},
                {"id": "hidden-model", "object": "model"}
            ]
        }))
        .into_response();
    }
    if has_messages_source_key(&headers) {
        return Json(json!({
            "object": "list",
            "data": [
                {
                    "id": "claude-test",
                    "object": "model",
                    "reasoningEffortModes": ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"]
                },
                {"id": "claude-hidden", "object": "model"}
            ]
        }))
        .into_response();
    }
    StatusCode::UNAUTHORIZED.into_response()
}

async fn upstream_image_generation(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    observe(&state, "/v1/images/generations", &headers);
    if !has_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request: Value = serde_json::from_slice(&body).unwrap();
    state.bodies.lock().unwrap().push(request);
    Json(json!({
        "created": 7,
        "data": [{"b64_json": "aW1hZ2U="}]
    }))
    .into_response()
}

async fn upstream_models_with_shared_catalog(headers: HeaderMap) -> Response<Body> {
    if has_source_key(&headers) {
        return Json(json!({
            "object": "list",
            "data": [
                {"id": "gpt-test", "object": "model"},
                {"id": "hidden-model", "object": "model"},
                {"id": "claude-test", "object": "model"}
            ]
        }))
        .into_response();
    }
    if has_messages_source_key(&headers) {
        return Json(json!({
            "object": "list",
            "data": [{"id": "claude-test", "object": "model"}]
        }))
        .into_response();
    }
    StatusCode::UNAUTHORIZED.into_response()
}

async fn upstream_models_rejecting_messages(headers: HeaderMap) -> Response<Body> {
    if has_source_key(&headers) {
        return Json(json!({
            "object": "list",
            "data": [{"id": "gpt-test", "object": "model"}]
        }))
        .into_response();
    }
    if has_messages_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    StatusCode::UNAUTHORIZED.into_response()
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

    if request.get("input").and_then(Value::as_str) == Some("done-only-stream") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from("data: [DONE]\n\n"))
            .unwrap();
    }

    if request.get("input").and_then(Value::as_str) == Some("partial-truncated-stream") {
        let chunks = stream::iter([
            Ok::<_, Infallible>(Bytes::from_static(
                b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"partial-response\"}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
            )),
        ]);
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

    let input = request.get("input").and_then(Value::as_str);
    if matches!(
        input,
        Some("coalesced-terminal" | "coalesced-terminal-after-output")
    ) {
        let prefix = Bytes::from_static(
            concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_once\"}}\n\n",
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"once\"}\n\n",
            )
            .as_bytes(),
        );
        let terminal_and_tail = Bytes::from_static(concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_once\",\"usage\":{\"input_tokens\":2,\"output_tokens\":1,\"total_tokens\":3}}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"duplicate\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_once\"}}\n\n",
        ).as_bytes());
        let chunks = if input == Some("coalesced-terminal") {
            vec![Ok::<_, Infallible>(Bytes::from(
                [prefix.as_ref(), terminal_and_tail.as_ref()].concat(),
            ))]
        } else {
            vec![Ok::<_, Infallible>(prefix), Ok(terminal_and_tail)]
        };
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(stream::iter(chunks)))
            .unwrap();
    }

    let release_stream = state.release_stream;
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
                    Ok::<_, Infallible>(Bytes::from_static(
                        b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"status\":\"completed\"}}\n\n",
                    )),
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

async fn recording_upstream_responses(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    observe(&state, "/v1/responses", &headers);
    if !has_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request: Value = serde_json::from_slice(&body).unwrap();
    state.bodies.lock().unwrap().push(request.clone());
    Json(json!({
        "id": "resp_test",
        "object": "response",
        "model": request["model"],
        "usage": {"input_tokens": 3, "output_tokens": 4, "total_tokens": 7}
    }))
    .into_response()
}

async fn native_replay_upstream_responses(
    State(state): State<NativeReplayUpstreamState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    if !has_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    state.bodies.lock().unwrap().push(request.clone());

    if request.get("previous_response_id").is_some() {
        if matches!(
            state.rejection,
            NativeReplayRejection::ZenithGatewayInvalidRequestStream
        ) && request.get("stream").and_then(Value::as_bool) == Some(true)
        {
            let chunks = stream::unfold(0_u8, |step| async move {
                match step {
                    // Let the first byte arrive after the replay window would
                    // have elapsed if it were measured from request start.
                    0 => {
                        tokio::time::sleep(Duration::from_millis(2_100)).await;
                        Some((
                            Ok::<_, Infallible>(Bytes::from_static(
                                b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_rejected\",\"status\":\"in_progress\"}}\n\n",
                            )),
                            1,
                        ))
                    }
                    1 => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        Some((
                            Ok::<_, Infallible>(Bytes::from_static(
                                b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"invalid_request\",\"message\":\"Zenith AI request is invalid. Check the model, messages, tools, and parameters.\"}}}\n\n",
                            )),
                            2,
                        ))
                    }
                    _ => None,
                }
            });
            return Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(chunks))
                .unwrap();
        }
        let (message, code) = match state.rejection {
            NativeReplayRejection::PreviousResponseRequiresWebsocket => (
                "previous_response_id is only supported on Responses WebSocket v2",
                "websocket_required",
            ),
            NativeReplayRejection::InvalidFunctionCallOutputCallId => (
                "Invalid call_id for function_call_output",
                "invalid_function_call_output_call_id",
            ),
            NativeReplayRejection::GenericInvalidRequest => {
                ("request payload is invalid", "invalid_request")
            }
            NativeReplayRejection::ZenithGatewayInvalidRequest => (
                "Zenith AI request is invalid. Check the model, messages, tools, and parameters.",
                "invalid_request",
            ),
            NativeReplayRejection::ZenithGatewayInvalidRequestStream => (
                "Zenith AI request is invalid. Check the model, messages, tools, and parameters.",
                "invalid_request",
            ),
        };
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": message,
                    "code": code
                }
            })),
        )
            .into_response();
    }

    let has_tool_output = request
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|input| {
            input.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("function_call_output")
            })
        });
    if has_tool_output {
        return native_replay_final_response(
            request.get("stream").and_then(Value::as_bool) == Some(true),
            "resp_native_final",
        );
    }

    if request.get("stream").and_then(Value::as_bool) == Some(true) {
        let chunks = stream::iter([
            Ok::<_, Infallible>(Bytes::from_static(
                b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_native_tool\",\"name\":\"run_command\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_native_tool\",\"status\":\"completed\"}}\n\n",
            )),
        ]);
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(chunks))
            .unwrap();
    }

    Json(json!({
        "id": "resp_native_tool",
        "object": "response",
        "model": request["model"],
        "output": [{
            "type": "function_call",
            "call_id": "call_native_tool",
            "name": "run_command",
            "arguments": "{\"command\":\"pwd\"}"
        }]
    }))
    .into_response()
}

async fn strict_function_item_id_upstream_responses(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    if !has_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    state.bodies.lock().unwrap().push(request.clone());

    if request
        .pointer("/input/1/id")
        .and_then(Value::as_str)
        .is_some_and(|id| id.starts_with("call_"))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": "Invalid 'input[1].id': 'call_cross_provider_01'. Expected an ID that begins with 'fc'."
                }
            })),
        )
            .into_response();
    }

    Json(json!({
        "id": "resp_strict_function_id",
        "object": "response",
        "model": request["model"],
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "History accepted"}]
        }]
    }))
    .into_response()
}

async fn strict_custom_tool_item_id_upstream_responses(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    if !has_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    state.bodies.lock().unwrap().push(request.clone());

    if request
        .pointer("/input/0/id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.starts_with("ctc_"))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": "Invalid 'input[433].id': 'fc_cross_provider_custom_01'. Expected an ID that begins with 'ctc'."
                }
            })),
        )
            .into_response();
    }

    Json(json!({
        "id": "resp_strict_custom_id",
        "object": "response",
        "model": request["model"],
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "History accepted"}]
        }]
    }))
    .into_response()
}

async fn strict_message_item_id_upstream_responses(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    if !has_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    state.bodies.lock().unwrap().push(request.clone());

    if request
        .pointer("/input/0/id")
        .and_then(Value::as_str)
        .is_some_and(|id| id.starts_with("item_"))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": "Invalid 'input[151].id': 'item_foreign_user_01'. Expected an ID that begins with 'msg'."
                }
            })),
        )
            .into_response();
    }

    Json(json!({
        "id": "resp_strict_message_id",
        "object": "response",
        "model": request["model"],
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "History accepted"}]
        }]
    }))
    .into_response()
}

async fn strict_missing_call_id_upstream_responses(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    if !has_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    state.bodies.lock().unwrap().push(request.clone());
    let input = request.get("input").and_then(Value::as_array);
    let has_missing_call_id = input.is_some_and(|input| {
        input.iter().any(|item| {
            let item_type = item.get("type").and_then(Value::as_str);
            let named_standalone_output = item_type == Some("function_call_output")
                && item
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| !name.trim().is_empty());
            !named_standalone_output
                && matches!(
                    item_type,
                    Some(
                        "function_call"
                            | "function_call_output"
                            | "custom_tool_call"
                            | "custom_tool_call_output"
                    )
                )
                && item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .is_none_or(|call_id| call_id.trim().is_empty())
        })
    });
    let missing_output = input.and_then(|input| {
        input.iter().find(|call| {
            let kind = call.get("type").and_then(Value::as_str).unwrap_or_default();
            matches!(kind, "function_call" | "custom_tool_call")
                && !input.iter().any(|result| {
                    result.get("type").and_then(Value::as_str)
                        == Some(format!("{kind}_output").as_str())
                        && result.get("call_id").is_some()
                        && result.get("call_id") == call.get("call_id")
                })
        })
    });
    let message = if has_missing_call_id {
        Some("Missing required field: call_id".to_string())
    } else {
        missing_output.map(|call| {
            format!(
                "No tool output found for {} call {}.",
                if call["type"] == "custom_tool_call" {
                    "custom tool"
                } else {
                    "function"
                },
                call["call_id"].as_str().unwrap_or_default()
            )
        })
    };
    if let Some(message) = message {
        if request.get("stream").and_then(Value::as_bool) == Some(true) {
            let error = json!({"type":"response.failed","response":{"status":"failed"},"error":{"message":message}});
            let chunks = stream::iter([Ok::<_, Infallible>(Bytes::from(format!(
                "data: {error}\n\n"
            )))]);
            return Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(chunks))
                .unwrap();
        }
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": message}})),
        )
            .into_response();
    }
    if request.get("stream").and_then(Value::as_bool) == Some(true) {
        let chunks = stream::iter([
            Ok::<_, Infallible>(Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"History accepted\"}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_strict_call_id\",\"status\":\"completed\"}}\n\n",
            )),
        ]);
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(chunks))
            .unwrap();
    }
    Json(json!({
        "id": "resp_strict_call_id",
        "object": "response",
        "model": request["model"],
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "History accepted"}]
        }]
    }))
    .into_response()
}

fn native_replay_final_response(streaming: bool, response_id: &str) -> Response<Body> {
    if !streaming {
        return Json(json!({
            "id": response_id,
            "object": "response",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Tool result received"}]
            }]
        }))
        .into_response();
    }

    let completed = format!(
        "data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"{response_id}\",\"status\":\"completed\"}}}}\n\n"
    );
    let chunks = stream::iter([
        Ok::<_, Infallible>(Bytes::from_static(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"Tool result received\"}\n\n",
        )),
        Ok::<_, Infallible>(Bytes::from(completed)),
    ]);
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(chunks))
        .unwrap()
}

async fn upstream_messages(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    observe(&state, "/v1/messages", &headers);
    if !has_messages_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    state.bodies.lock().unwrap().push(request.clone());

    if request.get("stream").and_then(Value::as_bool) == Some(true)
        && request
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| !tools.is_empty())
    {
        let chunks = stream::iter([
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_stream_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-test\",\"content\":[],\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Streaming \"}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_stream_1\",\"name\":\"read_file\",\"input\":{}}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"/tmp/a\\\"}\"}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"input_tokens\":3,\"output_tokens\":5}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(chunks))
            .unwrap();
    }
    if request.get("stream").and_then(Value::as_bool) == Some(true) {
        let chunks = stream::iter([
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_stream_text_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-test\",\"content\":[],\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Streaming context\"}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}\n\n",
            )),
            Ok::<_, Infallible>(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);
        return Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(chunks))
            .unwrap();
    }

    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let has_tool_result = messages
        .last()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks
                .iter()
                .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        });
    let input_text = messages
        .last()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks.iter().find_map(|block| {
                (block.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| block.get("text").and_then(Value::as_str))
                    .flatten()
            })
        });
    if input_text == Some("malformed") {
        return Json(json!({
            "id": "msg_malformed",
            "type": "message",
            "content": [{"type": "provider-private-body"}],
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }))
        .into_response();
    }
    if has_tool_result {
        return Json(json!({
            "id": "msg_tool_2",
            "stop_reason": "end_turn",
            "type": "message",
            "role": "assistant",
            "model": "claude-test",
            "content": [{"type": "text", "text": "Tool result received"}],
            "usage": {"input_tokens": 7, "output_tokens": 3}
        }))
        .into_response();
    }
    if request
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty())
    {
        if request["tools"][0]["name"] == "PowerShell" {
            return Json(json!({
                "id": "msg_custom_tool_1",
                "stop_reason": "tool_use",
                "type": "message",
                "role": "assistant",
                "model": "claude-test",
                "content": [{
                    "type": "tool_use",
                    "id": "tool_powershell_1",
                    "name": "PowerShell",
                    "input": {"input": "Get-ChildItem -Force"}
                }],
                "usage": {"input_tokens": 4, "output_tokens": 2}
            }))
            .into_response();
        }
        return Json(json!({
            "id": "msg_tool_1",
            "stop_reason": "tool_use",
            "type": "message",
            "role": "assistant",
            "model": "claude-test",
            "content": [{
                "type": "tool_use",
                "id": "tool_read_file_1",
                "name": "read_file",
                "input": {"path": "/tmp/a"}
            }],
            "usage": {"input_tokens": 4, "output_tokens": 2}
        }))
        .into_response();
    }
    Json(json!({
        "id": "msg_text_1",
        "stop_reason": "end_turn",
        "type": "message",
        "role": "assistant",
        "model": "claude-test",
        "content": [{"type": "text", "text": "Native Messages response"}],
        "usage": {"input_tokens": 2, "output_tokens": 2}
    }))
    .into_response()
}

async fn upstream_gemini_generate_content(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    observe(&state, "/v1/models/gemini-test:generateContent", &headers);
    if !has_gemini_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    state.bodies.lock().unwrap().push(request);
    Json(json!({
        "candidates": [{"content": {"parts": [{"text": "Native Gemini response"}]}}],
        "usageMetadata": {
            "promptTokenCount": 2,
            "candidatesTokenCount": 3,
            "totalTokenCount": 5
        }
    }))
    .into_response()
}

async fn upstream_gemini_stream_generate_content(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    observe(
        &state,
        "/v1/models/gemini-test:streamGenerateContent",
        &headers,
    );
    if !has_gemini_source_key(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    state.bodies.lock().unwrap().push(request);
    let chunks = stream::iter([
        Ok::<_, Infallible>(Bytes::from_static(
            b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Native Gemini \"}]}}]}\n\n",
        )),
        Ok::<_, Infallible>(Bytes::from_static(
            b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Native Gemini stream\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}}\n\n",
        )),
    ]);
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
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
        x_api_key: headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        x_goog_api_key: headers
            .get("x-goog-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        anthropic_version: headers
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        x_oai_attestation: headers
            .get("x-oai-attestation")
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

fn has_messages_source_key(headers: &HeaderMap) -> bool {
    headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        == Some(SOURCE_KEY)
        && headers
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok())
            == Some("2023-06-01")
}

fn has_gemini_source_key(headers: &HeaderMap) -> bool {
    headers
        .get("x-goog-api-key")
        .and_then(|value| value.to_str().ok())
        == Some(SOURCE_KEY)
}
