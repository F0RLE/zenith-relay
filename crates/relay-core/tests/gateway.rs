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
    discover_source_models, discover_source_models_and_protocol_bindings,
    discover_source_models_for_protocol_bindings, GatewayRuntime, GatewayRuntimeOptions,
    LocalGatewayKey, MessagesReasoningMode, ProviderSource, RuntimeLocalKey, RuntimeSource,
    SourceAdapter, SourceProtocolBinding, UsageEvent, WireApi,
};

const LOCAL_KEY: &str = "local-test-key";
const SOURCE_KEY: &str = "upstream-test-key";
const OVERSIZED_MODELS_CONTENT_LENGTH: &str = "4194305";
const MAX_CLIENT_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;

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
async fn models_are_discovered_with_the_source_credential() {
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
    assert_eq!(models, ["gpt-test", "hidden-model"]);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/models");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer upstream-test-key")
    );
}

#[tokio::test]
async fn native_messages_model_discovery_uses_anthropic_headers() {
    let (upstream, state) = spawn_upstream().await;
    let models = discover_source_models_for_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Synthetic Anthropic upstream".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Messages,
            models: vec!["claude-test".into(), "claude-hidden".into()],
        },
        &[SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["claude-test".into()],
        }],
    )
    .await
    .unwrap();
    assert_eq!(models, ["claude-test"]);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/models");
    assert_eq!(requests[0].authorization, None);
    assert_eq!(requests[0].x_api_key.as_deref(), Some(SOURCE_KEY));
    assert_eq!(requests[0].anthropic_version.as_deref(), Some("2023-06-01"));
}

#[tokio::test]
async fn responses_to_messages_discovery_keeps_responses_client_binding() {
    let (upstream, state) = spawn_upstream().await;
    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Synthetic bridged upstream".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: Vec::new(),
        },
        &[SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: Default::default(),
            model_ids: Vec::new(),
        }],
    )
    .await
    .unwrap();
    assert_eq!(discovery.models, ["claude-test", "claude-hidden"]);
    assert_eq!(
        discovery.protocol_bindings,
        [SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: Default::default(),
            model_ids: Vec::new(),
        }]
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/models");
    assert_eq!(requests[0].authorization, None);
    assert_eq!(requests[0].x_api_key.as_deref(), Some(SOURCE_KEY));
    assert_eq!(requests[0].anthropic_version.as_deref(), Some("2023-06-01"));
}

#[tokio::test]
async fn source_wide_catalog_binding_refreshes_new_models() {
    let (upstream, _) = spawn_upstream().await;
    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Synthetic source-wide upstream".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: vec!["gpt-test".into()],
        },
        &[SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gpt-test".into()],
        }],
    )
    .await
    .unwrap();

    assert_eq!(discovery.models, ["gpt-test", "hidden-model"]);
    assert_eq!(
        discovery.protocol_bindings,
        [SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: Vec::new(),
        }]
    );
}

#[tokio::test]
async fn native_responses_catalog_refreshes_after_models_are_split_to_a_messages_bridge() {
    let upstream = spawn_mixed_catalog_upstream().await;
    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Synthetic mixed source-wide upstream".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: vec!["gpt-test".into(), "claude-test".into()],
        },
        &[
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-test".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".into()],
            },
        ],
    )
    .await
    .unwrap();

    assert_eq!(
        discovery.models,
        ["gpt-test", "hidden-model", "claude-test"]
    );
    assert_eq!(
        discovery.protocol_bindings,
        [
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-test".into(), "hidden-model".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".into()],
            },
        ]
    );
}

#[tokio::test]
async fn native_model_discovery_keeps_each_protocol_catalog_separate() {
    let (upstream, state) = spawn_upstream().await;
    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Synthetic mixed upstream".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: Vec::new(),
        },
        &[
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            },
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            },
        ],
    )
    .await
    .unwrap();

    assert_eq!(
        discovery.models,
        ["gpt-test", "hidden-model", "claude-test", "claude-hidden"]
    );
    assert_eq!(
        discovery.protocol_bindings,
        [
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-test".into(), "hidden-model".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".into(), "claude-hidden".into()],
            },
        ]
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.authorization.is_some())
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.x_api_key.is_some())
            .count(),
        1
    );
}

#[tokio::test]
async fn native_and_bridged_responses_discovery_keep_route_catalogs_separate() {
    let (upstream, state) = spawn_upstream().await;
    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Synthetic mixed Responses source".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: Vec::new(),
        },
        &[
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            },
        ],
    )
    .await
    .unwrap();

    assert_eq!(
        discovery.models,
        ["gpt-test", "hidden-model", "claude-test", "claude-hidden"]
    );
    assert_eq!(
        discovery.protocol_bindings,
        [
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-test".into(), "hidden-model".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".into(), "claude-hidden".into()],
            },
        ]
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().any(|request| {
        request.authorization.as_deref() == Some("Bearer upstream-test-key")
            && request.x_api_key.is_none()
    }));
    assert!(requests.iter().any(|request| {
        request.authorization.is_none()
            && request.x_api_key.as_deref() == Some(SOURCE_KEY)
            && request.anthropic_version.as_deref() == Some("2023-06-01")
    }));
}

#[tokio::test]
async fn failed_native_binding_is_not_advertised_after_discovery() {
    let upstream =
        spawn(Router::new().route("/v1/models", get(upstream_models_rejecting_messages))).await;
    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Partial mixed upstream".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: Vec::new(),
        },
        &[
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            },
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            },
        ],
    )
    .await
    .unwrap();

    assert_eq!(discovery.models, ["gpt-test"]);
    assert_eq!(
        discovery.protocol_bindings,
        [SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gpt-test".into()],
        }]
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
            "input": "x".repeat(17 * 1024 * 1024),
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
async fn first_sse_bytes_commit_the_stream_without_waiting_for_text_output() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let request_url = format!("{}/v1/responses", gateway.base_url);
    let response_task = tokio::spawn(async move {
        reqwest::Client::new()
            .post(request_url)
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model": "gpt-test", "input": "hello", "stream": true}))
            .send()
            .await
            .unwrap()
    });
    let response = tokio::time::timeout(Duration::from_secs(1), response_task)
        .await
        .expect("the first native SSE bytes should establish the stream")
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
        .expect("first native SSE chunk was not forwarded")
        .unwrap()
        .unwrap();
    assert_eq!(first, "data: {\"type\":\"response.created\"}\n\n");
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
    assert!(events[0]
        .ttft_ms
        .is_some_and(|ttft| ttft <= events[0].latency_ms));
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
    assert_eq!(events[0].http_status, StatusCode::BAD_GATEWAY.as_u16());
}

#[tokio::test]
async fn responses_never_bridge_to_chat_completions_sources() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let usage_events = events.clone();
    let runtime = GatewayRuntime::new(
        ProviderSource {
            id: "chat-source".to_string(),
            name: "Stateless chat source".to_string(),
            base_url: "http://127.0.0.1:9/v1".to_string(),
            api_key: SOURCE_KEY.to_string(),
            wire_api: WireApi::ChatCompletions,
            models: vec!["gpt-test".to_string()],
        },
        LocalGatewayKey {
            id: "local-key-1".to_string(),
            secret: LOCAL_KEY.to_string(),
        },
        Arc::new(move |event| usage_events.lock().unwrap().push(event)),
    )
    .unwrap();
    let gateway = spawn(gateway::router(Arc::new(runtime))).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "inspect",
            "tools": [{"type": "function", "name": "shell", "parameters": {"type": "object"}}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn truncated_started_stream_reports_failure_in_sse_and_is_recorded_as_incomplete() {
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
    let body = response.text().await.unwrap();
    assert!(body.contains("data: {\"type\":\"response.created\"}"));
    assert!(body.contains("event: response.failed"));
    assert!(body.contains("stream_incomplete"));

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("stream_incomplete")
    );
    assert_eq!(events[0].http_status, StatusCode::BAD_GATEWAY.as_u16());
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
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_rate_limited")
    );
}

#[tokio::test]
async fn native_responses_replays_http_tool_continuation_when_upstream_requires_websocket() {
    let (upstream, state) = spawn_native_replay_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let client = reqwest::Client::new();
    let tools = json!([{
        "type": "function",
        "name": "run_command",
        "parameters": {
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"]
        }
    }]);

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "use a tool",
            "tools": tools
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = first.json().await.unwrap();
    assert_eq!(first["id"], "resp_native_tool");
    assert_eq!(first["output"][0]["type"], "function_call");

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "previous_response_id": first["id"],
            "input": [{
                "type": "function_call_output",
                "call_id": "call_native_tool",
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second: Value = second.json().await.unwrap();
    assert_eq!(
        second["output"][0]["content"][0]["text"],
        "Tool result received"
    );

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[1]["previous_response_id"], "resp_native_tool");
    assert!(bodies[2].get("previous_response_id").is_none());
    let replayed_input = bodies[2]["input"].as_array().unwrap();
    assert_eq!(replayed_input[1]["type"], "function_call");
    assert_eq!(replayed_input[2]["type"], "function_call_output");
    assert_eq!(replayed_input[2]["call_id"], "call_native_tool");
}

#[tokio::test]
async fn native_responses_replays_tool_continuation_after_invalid_call_id_rejection() {
    let (upstream, state) = spawn_native_replay_upstream_with_rejection(
        NativeReplayRejection::InvalidFunctionCallOutputCallId,
    )
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "use a tool",
            "tools": [{
                "type": "function",
                "name": "run_command",
                "parameters": {"type": "object"}
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = first.json().await.unwrap();

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "previous_response_id": first["id"],
            "input": [{
                "type": "function_call_output",
                "call_id": "call_native_tool",
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[1]["previous_response_id"], "resp_native_tool");
    assert!(bodies[2].get("previous_response_id").is_none());
    let replayed_input = bodies[2]["input"].as_array().unwrap();
    assert_eq!(replayed_input[1]["type"], "function_call");
    assert_eq!(replayed_input[2]["type"], "function_call_output");
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(!events[1].success);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("upstream_invalid_request")
    );
    assert!(events[2].success);
}

#[tokio::test]
async fn native_responses_does_not_replay_tool_continuation_after_generic_bad_request() {
    let (upstream, state) =
        spawn_native_replay_upstream_with_rejection(NativeReplayRejection::GenericInvalidRequest)
            .await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "use a tool"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = first.json().await.unwrap();

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "previous_response_id": first["id"],
            "input": [{
                "type": "function_call_output",
                "call_id": "call_native_tool",
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    assert_eq!(state.bodies.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn native_responses_replays_tool_continuation_after_zenith_gateway_invalid_request() {
    let (upstream, state) = spawn_native_replay_upstream_with_rejection(
        NativeReplayRejection::ZenithGatewayInvalidRequest,
    )
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "use a tool",
            "tools": [{
                "type": "function",
                "name": "run_command",
                "parameters": {"type": "object"}
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = first.json().await.unwrap();

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "previous_response_id": first["id"],
            "input": [{
                "type": "function_call_output",
                "call_id": "call_native_tool",
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[1]["previous_response_id"], "resp_native_tool");
    assert!(bodies[2].get("previous_response_id").is_none());
    assert_eq!(bodies[2]["input"][1]["type"], "function_call");
    assert_eq!(bodies[2]["input"][2]["type"], "function_call_output");
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(!events[1].success);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("upstream_invalid_request")
    );
    assert!(events[2].success);
}

#[tokio::test]
async fn native_responses_stream_replays_tool_continuation_after_zenith_gateway_invalid_request() {
    let (upstream, state) = spawn_native_replay_upstream_with_rejection(
        NativeReplayRejection::ZenithGatewayInvalidRequestStream,
    )
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "use a tool",
            "tools": [{
                "type": "function",
                "name": "run_command",
                "parameters": {"type": "object"}
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = first.json().await.unwrap();

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "stream": true,
            "previous_response_id": first["id"],
            "input": [{
                "type": "function_call_output",
                "call_id": "call_native_tool",
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second = second.text().await.unwrap();
    assert!(second.contains("Tool result received"));
    assert!(second.contains("response.completed"));
    assert!(!second.contains("resp_rejected"));

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[1]["previous_response_id"], "resp_native_tool");
    assert!(bodies[2].get("previous_response_id").is_none());
    assert_eq!(bodies[2]["stream"], true);
    assert_eq!(bodies[2]["input"][1]["type"], "function_call");
    assert_eq!(bodies[2]["input"][2]["type"], "function_call_output");
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(!events[1].success);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("upstream_invalid_request")
    );
    assert!(events[2].success);
}

#[tokio::test]
async fn native_responses_stream_replays_tool_continuation_when_upstream_requires_websocket() {
    let (upstream, state) = spawn_native_replay_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let client = reqwest::Client::new();
    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "use a tool",
            "stream": true,
            "tools": [{
                "type": "function",
                "name": "run_command",
                "parameters": {"type": "object"}
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first = first.text().await.unwrap();
    assert!(first.contains("\"type\":\"response.output_item.done\""));
    assert!(first.contains("\"type\":\"response.completed\""));

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "stream": true,
            "previous_response_id": "resp_native_tool",
            "input": [{
                "type": "function_call_output",
                "call_id": "call_native_tool",
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second = second.text().await.unwrap();
    assert!(second.contains("\"delta\":\"Tool result received\""));
    assert!(second.contains("\"type\":\"response.completed\""));

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[1]["previous_response_id"], "resp_native_tool");
    assert!(bodies[2].get("previous_response_id").is_none());
    assert_eq!(bodies[2]["stream"], true);
    let replayed_input = bodies[2]["input"].as_array().unwrap();
    assert_eq!(replayed_input[1]["type"], "function_call");
    assert_eq!(replayed_input[2]["type"], "function_call_output");
}

#[tokio::test]
async fn native_responses_repair_call_prefixed_function_item_ids_after_strict_rejection() {
    let (upstream, state) = spawn_strict_function_item_id_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": [
                {
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Inspect the workspace"}]
                },
                {
                    "type": "function_call",
                    "id": "call_cross_provider_01",
                    "call_id": "call_cross_provider_01",
                    "name": "run_command",
                    "arguments": "{\"command\":\"pwd\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_cross_provider_01",
                    "output": "C:\\workspace"
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["id"], "resp_strict_function_id");

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["input"][1]["id"], "call_cross_provider_01");
    assert_eq!(bodies[1]["input"][1]["id"], "fc_cross_provider_01");
    assert_eq!(bodies[1]["input"][1]["call_id"], "call_cross_provider_01");
    assert_eq!(bodies[1]["input"][2]["call_id"], "call_cross_provider_01");
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
}

#[tokio::test]
async fn native_responses_remove_item_prefixed_message_ids_after_strict_rejection() {
    let (upstream, state) = spawn_strict_message_item_id_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": [
                {
                    "type": "message",
                    "id": "item_foreign_user_01",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Inspect the workspace"}]
                },
                {
                    "id": "item_foreign_assistant_01",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "I will inspect it."}]
                },
                {
                    "type": "message",
                    "id": "msg_native_01",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "Keep changes scoped."}]
                },
                {
                    "type": "function_call",
                    "id": "item_function_01",
                    "call_id": "call_function_01",
                    "name": "run_command",
                    "arguments": "{\"command\":\"pwd\"}"
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["id"], "resp_strict_message_id");

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["input"][0]["id"], "item_foreign_user_01");
    assert_eq!(bodies[0]["input"][1]["id"], "item_foreign_assistant_01");
    assert!(bodies[1]["input"][0].get("id").is_none());
    assert!(bodies[1]["input"][1].get("id").is_none());
    assert_eq!(bodies[1]["input"][2]["id"], "msg_native_01");
    assert_eq!(bodies[1]["input"][3]["id"], "item_function_01");
    assert_eq!(bodies[1]["input"][3]["call_id"], "call_function_01");
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
}

#[tokio::test]
async fn responses_to_messages_bridge_translates_tool_turn_and_preserves_continuation() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Budget)
            .await;
    let client = reqwest::Client::new();
    let tools = json!([{
        "type": "function",
        "name": "read_file",
        "description": "Read one file",
        "parameters": {
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }
    }]);

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "Use the file tool",
            "tools": tools,
            "tool_choice": "auto",
            "reasoning": {"effort": "high"},
            "max_output_tokens": 64
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body: Value = first.json().await.unwrap();
    assert_eq!(first_body["object"], "response");
    assert!(first_body["id"]
        .as_str()
        .unwrap()
        .starts_with("resp_bridge_"));
    assert_eq!(first_body["output"][0]["type"], "function_call");
    assert_eq!(first_body["output"][0]["name"], "read_file");
    let response_id = first_body["id"].as_str().unwrap().to_string();
    let call_id = first_body["output"][0]["call_id"].as_str().unwrap();

    {
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/messages");
        assert_eq!(requests[0].authorization, None);
        assert_eq!(requests[0].x_api_key.as_deref(), Some(SOURCE_KEY));
        assert_eq!(requests[0].anthropic_version.as_deref(), Some("2023-06-01"));
    }

    {
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["model"], "claude-test");
        assert_eq!(bodies[0]["messages"][0]["role"], "user");
        assert_eq!(bodies[0]["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(bodies[0]["thinking"]["type"], "enabled");
        assert_eq!(bodies[0]["thinking"]["budget_tokens"], 16_384);
        assert_eq!(bodies[0]["max_tokens"], 17_408);
    }

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "previous_response_id": response_id,
            "input": [{
                "type": "function_call_output",
                "call_id": call_id,
                "output": {"ok": true}
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second_body: Value = second.json().await.unwrap();
    assert_eq!(second_body["output"][0]["type"], "message");
    assert_eq!(
        second_body["output"][0]["content"][0]["text"],
        "Tool result received"
    );

    {
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        let messages = bodies[1]["messages"].as_array().unwrap();
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        assert_eq!(messages[1]["content"][0]["id"], call_id);
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(messages[2]["content"][0]["tool_use_id"], call_id);
    }

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn bridge_skips_incompatible_candidate_without_cooling_it() {
    let (upstream, state) = spawn_messages_upstream().await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let usage_events = events.clone();
    let source = |id: &str, priority: i32, reasoning_mode| RuntimeSource {
        source: ProviderSource {
            id: id.to_string(),
            name: format!("Synthetic {id}"),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.to_string(),
            wire_api: WireApi::Responses,
            models: vec!["claude-test".to_string()],
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
        priority,
        weight: 1,
        recovery_delay_seconds: 0,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        last_used_at_ms: None,
    };
    let runtime = Arc::new(
        GatewayRuntime::from_pool(
            vec![
                source("incompatible", 10, MessagesReasoningMode::Disabled),
                source("compatible", 0, MessagesReasoningMode::Adaptive),
            ],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "local-key-1".to_string(),
                secret: LOCAL_KEY.to_string(),
            })],
            GatewayRuntimeOptions::default(),
            Arc::new(move |event| usage_events.lock().unwrap().push(event)),
        )
        .unwrap(),
    );
    let gateway = spawn(gateway::router(runtime.clone())).await;
    prime_source_metadata(&gateway).await;
    state.requests.lock().unwrap().clear();
    state.bodies.lock().unwrap().clear();

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "use reasoning",
            "reasoning": {"effort": "high"}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    assert_eq!(events.lock().unwrap().len(), 1);
    assert!(events.lock().unwrap()[0].success);
    let incompatible = runtime
        .candidate_runtime_order()
        .into_iter()
        .find(|candidate| candidate.candidate_id.contains("incompatible"))
        .unwrap();
    assert!(incompatible.available);
    assert_eq!(incompatible.next_retry_at_ms, None);
}

#[tokio::test]
async fn responses_to_messages_bridge_translates_custom_tool_turn_and_continuation() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "List the project files.",
            "tools": [{
                "type": "custom",
                "name": "PowerShell",
                "description": "Runs one PowerShell command."
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body: Value = first.json().await.unwrap();
    assert_eq!(first_body["output"][0]["type"], "custom_tool_call");
    assert_eq!(first_body["output"][0]["name"], "PowerShell");
    assert_eq!(first_body["output"][0]["input"], "Get-ChildItem -Force");
    let response_id = first_body["id"].as_str().unwrap().to_string();

    {
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["tools"][0]["name"], "PowerShell");
        assert_eq!(
            bodies[0]["tools"][0]["input_schema"]["properties"]["input"]["type"],
            "string"
        );
    }

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "previous_response_id": response_id,
            "input": [{
                "type": "custom_tool_call_output",
                "call_id": "tool_powershell_1",
                "output": "Cargo.toml\nsrc"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second_body: Value = second.json().await.unwrap();
    assert_eq!(
        second_body["output"][0]["content"][0]["text"],
        "Tool result received"
    );

    {
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(
            bodies[1]["messages"][1]["content"][0],
            json!({
                "type": "tool_use",
                "id": "tool_powershell_1",
                "name": "PowerShell",
                "input": {"input": "Get-ChildItem -Force"}
            })
        );
        assert_eq!(
            bodies[1]["messages"][2]["content"][0],
            json!({
                "type": "tool_result",
                "tool_use_id": "tool_powershell_1",
                "content": "Cargo.toml\nsrc"
            })
        );
    }

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn source_can_mix_native_and_bridged_responses_models() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, _) = spawn_mixed_responses_gateway(&upstream.base_url).await;
    let client = reqwest::Client::new();

    let native = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "native route"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(native.status(), StatusCode::OK);
    assert_eq!(native.json::<Value>().await.unwrap()["id"], "resp_test");

    let bridged = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-oai-attestation", "must-not-reach-messages")
        .json(&json!({
            "model": "claude-test",
            "input": "bridge route"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bridged.status(), StatusCode::OK);
    let bridged_body: Value = bridged.json().await.unwrap();
    assert!(bridged_body["id"]
        .as_str()
        .is_some_and(|id| id.starts_with("resp_bridge_")));
    assert_eq!(
        bridged_body["output"][0]["content"][0]["text"],
        "Native Messages response"
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer upstream-test-key")
    );
    assert_eq!(requests[1].path, "/v1/messages");
    assert_eq!(requests[1].authorization, None);
    assert_eq!(requests[1].x_api_key.as_deref(), Some(SOURCE_KEY));
    assert_eq!(requests[1].anthropic_version.as_deref(), Some("2023-06-01"));
    assert_eq!(requests[1].x_oai_attestation, None);
}

#[tokio::test]
async fn responses_to_messages_bridge_translates_plain_response() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "Give a plain answer"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["object"], "response");
    assert_eq!(body["status"], "completed");
    assert_eq!(body["model"], "claude-test");
    assert_eq!(body["output"][0]["type"], "message");
    assert_eq!(
        body["output"][0]["content"][0]["text"],
        "Native Messages response"
    );
    assert_eq!(body["usage"]["input_tokens"], 2);
    assert_eq!(body["usage"]["output_tokens"], 2);

    {
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/messages");
        assert_eq!(requests[0].authorization, None);
        assert_eq!(requests[0].x_api_key.as_deref(), Some(SOURCE_KEY));
        assert_eq!(requests[0].anthropic_version.as_deref(), Some("2023-06-01"));
    }
    {
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["model"], "claude-test");
        assert_eq!(bodies[0]["messages"][0]["role"], "user");
        assert_eq!(
            bodies[0]["messages"][0]["content"][0]["text"],
            "Give a plain answer"
        );
    }
    assert_eq!(events.lock().unwrap().len(), 1);
    assert!(events.lock().unwrap()[0].success);
}

#[tokio::test]
async fn responses_to_gemini_bridge_uses_native_routes_for_plain_and_streaming_requests() {
    let (upstream, state) = spawn_gemini_upstream().await;
    let (gateway, events) = spawn_gemini_bridge_gateway(&upstream.base_url).await;
    let client = reqwest::Client::new();

    let plain = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-goog-api-key", "client-google-key")
        .header("x-oai-attestation", "client-attestation")
        .json(&json!({
            "model": "gemini-test",
            "input": "Give a plain answer",
            "temperature": 0.2,
            "max_output_tokens": 32,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(plain.status(), StatusCode::OK);
    let plain: Value = plain.json().await.unwrap();
    assert_eq!(
        plain["output"][0]["content"][0]["text"],
        "Native Gemini response"
    );
    assert_eq!(plain["usage"]["input_tokens"], 2);
    assert_eq!(plain["usage"]["output_tokens"], 3);

    let stream = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gemini-test",
            "input": "Stream a response",
            "stream": true,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    let stream = stream.text().await.unwrap();
    assert!(stream.contains("\"type\":\"response.output_text.delta\""));
    assert!(stream.contains("Native Gemini stream"));
    assert!(stream.contains("\"type\":\"response.completed\""));

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/v1/models/gemini-test:generateContent");
    assert_eq!(
        requests[1].path,
        "/v1/models/gemini-test:streamGenerateContent"
    );
    assert!(requests
        .iter()
        .all(|request| request.authorization.is_none()));
    assert!(requests
        .iter()
        .all(|request| request.x_goog_api_key.as_deref() == Some(SOURCE_KEY)));
    assert!(requests
        .iter()
        .all(|request| request.x_oai_attestation.is_none()));
    drop(requests);

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(
        bodies[0]["contents"][0]["parts"][0]["text"],
        "Give a plain answer"
    );
    assert_eq!(bodies[0]["generationConfig"]["temperature"], 0.2);
    assert_eq!(bodies[0]["generationConfig"]["maxOutputTokens"], 32);
    assert!(bodies.iter().all(|body| body.get("model").is_none()));
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn responses_to_messages_bridge_maps_adaptive_reasoning_without_temperature() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Adaptive)
            .await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "answer",
            "reasoning": {"effort": "minimal"},
            "temperature": 0.2,
            "top_p": 0.9
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["thinking"]["type"], "adaptive");
    assert_eq!(bodies[0]["output_config"]["effort"], "low");
    assert!(bodies[0].get("temperature").is_none());
    assert!(bodies[0].get("top_p").is_none());
    drop(bodies);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].requested_reasoning_effort.as_deref(),
        Some("minimal")
    );
    assert_eq!(events[0].effective_reasoning_effort.as_deref(), Some("low"));
}

#[tokio::test]
async fn disabled_reasoning_fails_before_upstream_but_opaque_tools_are_omitted() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, _) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;
    let client = reqwest::Client::new();

    let reasoning = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "reason",
            "reasoning": {"effort": "high"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reasoning.status(), StatusCode::BAD_REQUEST);
    let reasoning_body: Value = reasoning.json().await.unwrap();
    assert_eq!(
        reasoning_body["error"]["code"],
        "reasoning_effort_not_allowed"
    );

    let opaque_tool = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "tool",
            "tools": [{"type": "web_search"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(opaque_tool.status(), StatusCode::OK);
    let requests = state.requests.lock().unwrap();
    let bodies = state.bodies.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].get("tools").is_none());
}

#[tokio::test]
async fn missing_bridge_continuation_is_rejected_without_context_free_tool_output() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, _) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "previous_response_id": "resp_bridge_missing",
            "input": [{
                "type": "function_call_output",
                "call_id": "tool_missing",
                "output": "unsafe to send without context"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "adapter_continuation_missing");
    assert!(state.requests.lock().unwrap().is_empty());
    assert!(state.bodies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn responses_to_messages_bridge_translates_sse_text_and_tool_arguments() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "stream",
            "stream": true,
            "tools": [{
                "type": "function",
                "name": "read_file",
                "parameters": {"type": "object"}
            }]
        }))
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
    let body = response.text().await.unwrap();
    assert!(body.contains("\"type\":\"response.created\""));
    assert!(body.contains("\"type\":\"response.function_call_arguments.delta\""));
    assert!(body.contains("\"type\":\"response.function_call_arguments.done\""));
    assert!(body.contains("\"delta\":\"{\\\"path\\\":\\\"/tmp/a\\\"}\""));
    assert!(body.contains("\"type\":\"response.output_item.done\""));
    assert!(body.contains("\"type\":\"response.completed\""));
    assert_eq!(state.bodies.lock().unwrap().len(), 1);
    assert_eq!(events.lock().unwrap().len(), 1);
    assert!(events.lock().unwrap()[0].success);
}

#[tokio::test]
async fn responses_to_messages_bridge_preserves_plain_stream_context_for_http_continuation() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "Remember this turn",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body = first.text().await.unwrap();
    let response_id = first_body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find_map(|event| {
            (event["type"] == "response.completed")
                .then(|| event["response"]["id"].as_str().map(str::to_string))
                .flatten()
        })
        .expect("the completed bridge stream exposes a response id");

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "previous_response_id": response_id,
            "input": "What did I ask you to remember?"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    let messages = bodies[1]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"][0]["text"], "Remember this turn");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"][0]["text"], "Streaming context");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(
        messages[2]["content"][0]["text"],
        "What did I ask you to remember?"
    );
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn malformed_messages_response_is_redacted_as_adapter_error() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "malformed"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "adapter_upstream_response_invalid");
    assert!(!body.to_string().contains("provider-private-body"));
    assert_eq!(state.bodies.lock().unwrap().len(), 1);
    assert!(!events.lock().unwrap()[0].success);
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
        wire_api: WireApi::Responses,
        models: vec!["claude-test".to_string()],
    };
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource {
            source,
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
        GatewayRuntimeOptions::default(),
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
                wire_api: WireApi::Responses,
                models: vec!["gemini-test".to_string()],
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
                    "reasoningEffortModes": ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"],
                    "reasoningProbe": {
                        "status": "confirmed",
                        "total": 7,
                        "running": 0,
                        "success": 7,
                        "failed": 0,
                        "confirmed": 7,
                        "rejected": 0,
                        "inconclusive": 0,
                        "pending": 0,
                        "lastProbeAt": "2026-08-20T00:00:00Z"
                    }
                },
                {"id": "claude-hidden", "object": "model"}
            ]
        }))
        .into_response();
    }
    StatusCode::UNAUTHORIZED.into_response()
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
                b"event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"input_tokens\":3,\"output_tokens\":5}}\n\n",
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
                b"event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}\n\n",
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
