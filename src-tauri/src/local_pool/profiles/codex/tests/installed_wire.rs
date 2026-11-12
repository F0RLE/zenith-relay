use super::*;
use axum::{Json, Router, body::Body, extract::State, http::HeaderMap, routing::post};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use zenith_relay_core::{
    CapabilityOrigin, CapabilityStatus, GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey,
    ModelEndpointCapability, ProtocolFeature, ProviderSource, RuntimeLocalKey, RuntimeSource,
    UsageEvent, WireApi, model_metadata::ModelMetadataCatalogHandle,
};

const MODEL: &str = "fixture/synthetic-model";
const CLIENT_KEY: &str = "sk-relay-synthetic-codex-contract";
const UPSTREAM_KEY: &str = "synthetic-codex-upstream";
const TOOL: &str = "mcp__relay_contract__lookup";

fn tool_result_digest(output: &Value) -> String {
    let parts = if let Some(text) = output.as_str() {
        vec![("string", text)]
    } else {
        output
            .as_array()
            .unwrap()
            .iter()
            .map(|part| {
                let kind = part["type"].as_str().unwrap();
                assert!(matches!(kind, "text" | "input_text"));
                (kind, part["text"].as_str().unwrap())
            })
            .collect()
    };
    let mut hash = Sha256::new();
    for (kind, text) in parts {
        hash.update(format!("{kind}\0{}:", text.len()).as_bytes());
        hash.update(text.as_bytes());
    }
    hex::encode(hash.finalize())
}

fn serve(runtime: &tokio::runtime::Runtime, router: Router) -> String {
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let _server = runtime.spawn(async move { axum::serve(listener, router).await.unwrap() });
    origin
}

fn tool_identity(body: &Value) -> Value {
    for tool in body["tools"].as_array().unwrap() {
        if tool["type"] == "function" && tool["name"] == TOOL {
            return json!({"name":TOOL});
        }
        if tool["type"] == "namespace" && tool["name"] == "mcp__relay_contract" {
            assert!(
                tool["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["name"] == "lookup")
            );
            return json!({"name":"lookup","namespace":"mcp__relay_contract"});
        }
    }
    panic!("the installed client did not declare the deterministic fixture tool");
}

fn response_events(model: &Value, identity: Option<Value>) -> String {
    let tool = identity.is_some();
    let item = if let Some(identity) = identity {
        let mut item = json!({"id":"fc_relay_fixture", "type":"function_call",
            "call_id":"call_relay_codex_fixture", "arguments":"{\"value\":\"synthetic\"}", "status":"completed"});
        item.as_object_mut()
            .unwrap()
            .extend(identity.as_object().unwrap().clone());
        item
    } else {
        json!({"id":"msg_relay_fixture","type":"message","role":"assistant","status":"completed",
            "content":[{"type":"output_text","text":"relay-codex-contract-ok","annotations":[]}]})
    };
    let response = json!({"id":if tool {"resp_relay_codex_tool"} else {"resp_relay_codex_answer"},
        "object":"response","created_at":1,"model":model,"status":"completed","output":[item],
        "usage":{"input_tokens":5,"output_tokens":5,"total_tokens":10,
            "input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}});
    let mut started = response.clone();
    started["status"] = "in_progress".into();
    started["output"] = json!([]);
    let mut added = item.clone();
    added["status"] = "in_progress".into();
    if tool {
        added["arguments"] = "".into();
    } else {
        added["content"] = json!([]);
    }
    let mut events = vec![
        json!({"type":"response.created","response":started}),
        json!({"type":"response.output_item.added","output_index":0,"item":added}),
    ];
    if tool {
        events.push(json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":item["id"],"delta":item["arguments"]}));
        events.push(json!({"type":"response.function_call_arguments.done","output_index":0,"item_id":item["id"],"arguments":item["arguments"]}));
    } else {
        events.extend([
            json!({"type":"response.content_part.added","output_index":0,"content_index":0,"item_id":item["id"],"part":{"type":"output_text","text":"","annotations":[]}}),
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"item_id":item["id"],"delta":"relay-codex-contract-ok"}),
            json!({"type":"response.output_text.done","output_index":0,"content_index":0,"item_id":item["id"],"text":"relay-codex-contract-ok"}),
            json!({"type":"response.content_part.done","output_index":0,"content_index":0,"item_id":item["id"],"part":item["content"][0]}),
        ]);
    }
    events.extend([
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.completed","response":response}),
    ]);
    events
        .into_iter()
        .enumerate()
        .map(|(index, mut event)| {
            event["sequence_number"] = json!(index);
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect()
}

async fn upstream(
    State(requests): State<Arc<Mutex<Vec<Value>>>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    assert_eq!(headers["authorization"], format!("Bearer {UPSTREAM_KEY}"));
    assert_eq!(body["model"], MODEL);
    let first = {
        let mut requests = requests.lock().unwrap();
        let first = requests.is_empty();
        assert!(
            requests.len() < 2,
            "the fixture permits exactly two physical sends"
        );
        requests.push(body.clone());
        first
    };
    let identity = first.then(|| tool_identity(&body));
    axum::response::Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(response_events(&body["model"], identity)))
        .unwrap()
}

#[test]
#[ignore = "requires ZENITH_CODEX_BIN; isolated installed Codex, generated catalog, deterministic MCP and loopback upstream"]
fn installed_codex_completes_gateway_tool_cycle() {
    let cli = std::env::var_os("ZENITH_CODEX_BIN")
        .expect("Set ZENITH_CODEX_BIN to the exact installed Codex executable");
    let cli = Path::new(&cli);
    assert!(
        cli.is_absolute() && cli.is_file(),
        "select an absolute Codex executable"
    );
    let cli = cli.canonicalize().unwrap();
    let (root, home, _) = profile_dirs("installed-codex-wire");
    let metadata = ModelMetadataCatalog::from_models_dev_json(&json!({(MODEL):{
        "reasoning":true,"reasoning_effort_levels":["low"],"tool_call":true,"streaming":true,
        "limit":{"context":32000,"output":1024},"modalities":{"input":["text"],"output":["text"]}
    }}).to_string()).unwrap();
    let endpoint = ModelEndpointCapability {
        model_id: MODEL.into(),
        upstream_wire_api: WireApi::Responses,
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
        reasoning_efforts: vec!["low".into()],
        token_limits: Default::default(),
        service_tiers: None,
    };
    let catalog = direct_source_model_catalog_with_capabilities(
        &home,
        &[MODEL.into()],
        &metadata,
        std::slice::from_ref(&endpoint),
    )
    .unwrap()
    .expect("generated fixture catalog");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    let requests: Arc<Mutex<Vec<Value>>> = Arc::default();
    let origin = serve(
        &runtime,
        Router::new()
            .route("/v1/responses", post(upstream))
            .with_state(requests.clone()),
    );
    let mut source = RuntimeSource::unrestricted(ProviderSource {
        id: "synthetic-source".into(),
        name: "Synthetic source".into(),
        base_url: format!("{origin}/v1"),
        api_key: UPSTREAM_KEY.into(),
        proxy_url: None,
        wire_api: WireApi::Responses,
        models: vec![MODEL.into()],
    });
    source.protocol_config.endpoint_hint = Some(WireApi::Responses);
    source.protocol_config.capabilities = vec![endpoint];
    let events: Arc<Mutex<Vec<UsageEvent>>> = Arc::default();
    let captured = events.clone();
    let gateway = GatewayRuntime::from_pool(
        vec![source],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "synthetic-key".into(),
            secret: CLIENT_KEY.into(),
        })],
        GatewayRuntimeOptions {
            max_retry_candidates: 1,
            model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(metadata)),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(move |event| captured.lock().unwrap().push(event)),
    )
    .unwrap();
    let origin = serve(
        &runtime,
        zenith_relay_core::gateway::router(Arc::new(gateway)),
    );
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut command = std::process::Command::new("bun");
    command
        .current_dir(repository)
        .arg("scripts/check/check-codex-wire.mjs")
        .arg(format!("--cli={}", cli.to_str().unwrap()))
        .arg(format!("--catalog={catalog}"))
        .arg(format!("--gateway={origin}"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    let output = command
        .output()
        .expect("start isolated installed Codex qualification");
    println!("{}", String::from_utf8_lossy(&output.stdout));
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }
    assert!(
        output.status.success(),
        "installed Codex tool-cycle qualification"
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["ok"], true);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let tool_result = requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["type"] == "function_call_output" && item["call_id"] == "call_relay_codex_fixture"
        })
        .expect("the original tool result reached the physical upstream");
    assert_eq!(
        result["toolResultDigest"],
        tool_result_digest(&tool_result["output"])
    );
    drop(requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success
        && event.input_tokens == Some(5)
        && event.output_tokens == Some(5)));
    drop(events);
    fs::remove_dir_all(root).unwrap();
}
