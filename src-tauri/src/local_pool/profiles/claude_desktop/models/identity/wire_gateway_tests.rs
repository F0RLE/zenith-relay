use super::*;
use axum::{Json, Router, body::Body, extract::State, http::HeaderMap, routing::post};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zenith_relay_core::{
    CapabilityOrigin, GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey,
    ModelEndpointCapability, ProviderSource, RuntimeLocalKey, RuntimeSource, SourceProtocolBinding,
    SourceProtocolConfig, UsageEvent, model_metadata::ModelMetadataCatalogHandle,
};

const ACK: &str = "relay-contract-ok";
const CLIENT_KEY: &str = "sk-ant-synthetic-loopback-contract";
pub(super) const UPSTREAM_KEY: &str = "synthetic-upstream-key";

pub(super) struct LoopbackServer {
    pub(super) origin: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) async fn serve(router: Router) -> LoopbackServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    LoopbackServer { origin, task }
}

async fn upstream(
    State(requests): State<Arc<Mutex<Vec<Value>>>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    assert_eq!(headers.get("x-api-key").unwrap(), UPSTREAM_KEY);
    requests.lock().unwrap().push(body.clone());
    let events = [
        (
            "message_start",
            json!({"type":"message_start","message":{
                "id":"msg_synthetic_gateway","type":"message","role":"assistant",
                "model":body["model"],"content":[],"stop_reason":null,"stop_sequence":null,
                "usage":{"input_tokens":5,"output_tokens":0}
            }}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        ),
        (
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":ACK}}),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":5}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ];
    sse_reply(events, false)
}

pub(super) fn sse_reply(
    events: impl IntoIterator<Item = (&'static str, Value)>,
    done: bool,
) -> axum::response::Response {
    let mut body = events
        .into_iter()
        .map(|(event, value)| format!("event: {event}\ndata: {value}\n\n"))
        .collect::<String>();
    if done {
        body.push_str("data: [DONE]\n\n");
    }
    axum::response::Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(body))
        .unwrap()
}

pub(super) fn gateway_for(
    runtime: &tokio::runtime::Runtime,
    upstream: &LoopbackServer,
    identity: &str,
    levels: &[&str],
    protocol: WireApi,
) -> (LoopbackServer, Arc<Mutex<Vec<UsageEvent>>>) {
    let source = RuntimeSource {
        source: ProviderSource {
            id: "synthetic-source".into(),
            name: "Synthetic source".into(),
            base_url: format!("{}/v1", upstream.origin),
            api_key: UPSTREAM_KEY.into(),
            proxy_url: None,
            wire_api: protocol,
            models: vec!["gateway-model".into()],
        },
        protocol_config: SourceProtocolConfig {
            endpoint_hint: Some(protocol),
            capabilities: vec![ModelEndpointCapability {
                model_id: "gateway-model".into(),
                upstream_wire_api: protocol,
                status: CapabilityStatus::Declared,
                origin: CapabilityOrigin::Manual,
                checked_at_ms: 1,
                features: [
                    ProtocolFeature::Text,
                    ProtocolFeature::Streaming,
                    ProtocolFeature::FunctionTools,
                    ProtocolFeature::Reasoning,
                ]
                .into_iter()
                .map(|feature| (feature, CapabilityStatus::Declared))
                .collect(),
                reasoning_efforts: levels.iter().map(|level| (*level).to_string()).collect(),
                token_limits: Default::default(),
                service_tiers: None,
            }],
            ..Default::default()
        },
        protocol_bindings: vec![SourceProtocolBinding::legacy(
            protocol,
            &["gateway-model".into()],
        )],
        enabled: true,
        draining: false,
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        last_used_at_ms: None,
    };
    let events: Arc<Mutex<Vec<UsageEvent>>> = Arc::default();
    let captured = events.clone();
    let gateway_runtime = GatewayRuntime::from_pool(
        vec![source],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "synthetic-key".into(),
            secret: CLIENT_KEY.into(),
        })],
        GatewayRuntimeOptions {
            max_retry_candidates: 1,
            model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(identity_catalog(
                identity, levels,
            ))),
            ..Default::default()
        },
        Arc::new(move |event| captured.lock().unwrap().push(event)),
    )
    .unwrap();
    let gateway = runtime.block_on(serve(zenith_relay_core::gateway::router(Arc::new(
        gateway_runtime,
    ))));
    (gateway, events)
}

pub(super) fn run_probe(command: &mut std::process::Command, context: &str) {
    let mut child = command
        .spawn()
        .expect("Start the isolated installed-Code gateway qualification");
    let deadline = Instant::now() + Duration::from_secs(90);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("installed-Code gateway qualification timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        status.success(),
        "installed-Code gateway qualification: {context}"
    );
}

#[test]
#[ignore = "requires ZENITH_CLAUDE_CODE_BIN; real installed Code through an isolated Relay gateway and synthetic loopback upstream"]
fn installed_claude_code_roundtrips_generated_identity_through_gateway() {
    let cli = std::env::var_os("ZENITH_CLAUDE_CODE_BIN")
        .expect("Set ZENITH_CLAUDE_CODE_BIN to the exact installed Claude Code executable");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    for (identity, levels, efforts) in [
        ("claude-opus-4-6", vec!["low", "high", "max"], "max"),
        (
            "claude-opus-4-7",
            vec!["low", "high", "xhigh", "max"],
            "max,xhigh",
        ),
    ] {
        let models =
            inference_models_for_client(&[catalog_model_for(identity, &levels)], true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let upstream = runtime.block_on(serve(
            Router::new()
                .route("/v1/messages", post(upstream))
                .with_state(requests.clone()),
        ));
        let (gateway, events) =
            gateway_for(&runtime, &upstream, identity, &levels, WireApi::Messages);
        let mut command = installed_wire_command(&cli, &models[0], efforts);
        command.args(["--gateway", &gateway.origin]);
        run_probe(&mut command, identity);
        let expected: Vec<_> = efforts.split(',').collect();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), expected.len());
        for (request, effort) in requests.iter().zip(&expected) {
            assert_eq!(request["model"], "gateway-model");
            assert_eq!(request["output_config"]["effort"], *effort);
            assert_eq!(request["stream"], true);
        }
        let events = events.lock().unwrap();
        assert_eq!(events.len(), expected.len());
        for (event, effort) in events.iter().zip(expected) {
            assert!(event.success);
            assert_eq!(event.http_status, 200);
            assert_eq!(event.candidate_id.as_deref(), Some("synthetic-source"));
            assert_eq!(event.requested_model.as_deref(), models[0]["name"].as_str());
            assert_eq!(event.resolved_model.as_deref(), Some("gateway-model"));
            assert_eq!(event.requested_reasoning_effort.as_deref(), Some(effort));
            assert_eq!(event.effective_reasoning_effort.as_deref(), Some(effort));
        }
    }
}
