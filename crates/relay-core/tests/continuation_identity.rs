use axum::{extract::State, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use zenith_relay_core::{
    AdapterContinuationStore, AdapterRequestContext, CacheWriteTtl, GatewayRuntime,
    GatewayRuntimeOptions, LocalGatewayKey, MessagesReasoningMode, ProviderSource, RuntimeLocalKey,
    RuntimeSource, SourceAdapter, SourceProtocolBinding, WireApi,
};

#[path = "support/local_server.rs"]
mod local_server;

fn reverse_object_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let fields = std::mem::take(object);
            for (key, mut value) in fields.into_iter().rev() {
                reverse_object_fields(&mut value);
                object.insert(key, value);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(reverse_object_fields),
        _ => {}
    }
}

fn file_tool_input(request: &mut Value) -> &mut Value {
    let call = request["messages"][1]["content"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|block| block["type"] == "tool_use")
        .unwrap();
    &mut call["input"]
}

fn file_request() -> Value {
    json!({
        "model":"synthetic-model", "max_tokens":1024,
        "messages":[{"role":"user","content":"Create a synthetic SVG file."}],
        "tools":[{"name":"create_file","input_schema":{
            "type":"object", "properties":{
                "path":{"type":"string"}, "content":{"type":"string"},
                "lines":{"type":"integer"},
                "options":{"type":"object","properties":{
                    "encoding":{"type":"string"},"overwrite":{"type":"boolean"},
                    "layers":{"type":"array","items":{"type":"object"}}
                }}
            },"required":["path","content"]
        }}]
    })
}

fn file_response() -> Value {
    json!({
        "id":"resp_synthetic", "status":"completed", "model":"synthetic-model",
        "output":[
            {"type":"reasoning","id":"rs_synthetic","summary":[],
                "encrypted_content":"synthetic-ciphertext"},
            {"type":"function_call","id":"fc_synthetic","call_id":"call_synthetic",
                "name":"create_file","status":"completed",
                "arguments":r#"{"path":"pelican.svg","content":"<svg></svg>","lines":130,"options":{"encoding":"utf-8","overwrite":true,"layers":[{"kind":"body","color":"white"},{"kind":"wheel","color":"gray"}]}}"#}
        ],"usage":{"input_tokens":12,"output_tokens":6,"total_tokens":18}
    })
}

fn continuation_request(initial: &Value, content: Value) -> Value {
    let mut next = initial.clone();
    next["messages"].as_array_mut().unwrap().extend([
        json!({"role":"assistant","content":content}),
        json!({"role":"user","content":[{
            "type":"tool_result","tool_use_id":"call_synthetic","content":"File created."
        }]}),
    ]);
    next
}

#[test]
fn continuation_replay_ignores_function_argument_object_order_after_file_creation() {
    let adapter = SourceAdapter::between(WireApi::Messages, WireApi::Responses).unwrap();
    let prepare = |request: &Value, previous, response_id_seed| {
        adapter.prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Messages,
            request,
            model: "synthetic-model",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous,
            response_scope: "synthetic-route",
            response_id_seed,
        })
    };
    let initial = file_request();
    let prepared = prepare(&initial, None, "synthetic-first").unwrap();
    let upstream = file_response();
    let completed = prepared
        .translate_response_bytes(&serde_json::to_vec(&upstream).unwrap())
        .unwrap()
        .unwrap();
    let (response_id, continuation) = completed.continuation().unwrap();
    let mut store = AdapterContinuationStore::default();
    store.insert(
        "synthetic-key",
        response_id,
        "synthetic-route",
        continuation.clone(),
        10,
    );

    let mut assistant = completed.response_body()["content"].clone();
    let call = assistant
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|block| block["type"] == "tool_use")
        .unwrap();
    reverse_object_fields(&mut call["input"]);
    let next = continuation_request(&initial, assistant);
    let mut unchanged = next.clone();
    reverse_object_fields(file_tool_input(&mut unchanged));
    assert!(prepare(&unchanged, Some(continuation.clone()), "synthetic-control").is_ok());
    let restored = store
        .get("synthetic-key", response_id, "synthetic-route", 11)
        .unwrap();
    let resumed = prepare(&next, Some(restored), "synthetic-next");
    assert!(
        resumed.is_ok(),
        "equivalent argument objects must restore the original native state: {:?}",
        resumed.as_ref().err().map(|error| error.code())
    );
    let resumed = resumed.unwrap();
    let input = resumed.upstream_body()["input"].as_array().unwrap();
    assert!(input.contains(&upstream["output"][0]));
    assert!(input.contains(&upstream["output"][1]));

    for field in ["path", "content", "lines", "layers"] {
        let mut changed = next.clone();
        let arguments = file_tool_input(&mut changed);
        match field {
            "path" => arguments["path"] = json!("different.svg"),
            "content" => arguments["content"] = json!("<svg>changed</svg>"),
            "lines" => arguments["lines"] = json!(131),
            "layers" => arguments["options"]["layers"]
                .as_array_mut()
                .unwrap()
                .reverse(),
            _ => unreachable!(),
        }
        let restored = store
            .get("synthetic-key", response_id, "synthetic-route", 12)
            .unwrap();
        let rejected = prepare(&changed, Some(restored), "synthetic-changed");
        assert_eq!(
            rejected.unwrap_err().code(),
            "adapter_continuation_mismatch"
        );
    }
}

async fn file_upstream(
    State(observed): State<Arc<Mutex<Vec<Value>>>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let continued = body["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "function_call_output");
    observed.lock().unwrap().push(body);
    Json(if continued {
        json!({"id":"resp_synthetic_final","status":"completed","model":"synthetic-model",
            "output":[{"type":"message","role":"assistant","status":"completed",
                "content":[{"type":"output_text","text":"File verified.","annotations":[]}]}],
            "usage":{"input_tokens":18,"output_tokens":4,"total_tokens":22}})
    } else {
        file_response()
    })
}

#[tokio::test]
async fn file_tool_result_restores_the_native_response_through_gateway_history_lookup() {
    let observed: Arc<Mutex<Vec<Value>>> = Arc::default();
    let provider = local_server::spawn(
        Router::new()
            .route("/v1/responses", post(file_upstream))
            .with_state(observed.clone()),
    )
    .await;
    let mut source = RuntimeSource::unrestricted(ProviderSource {
        id: "synthetic-source".into(),
        name: "Synthetic provider".into(),
        base_url: format!("{}/v1", provider.base_url),
        api_key: "synthetic-provider-key".into(),
        proxy_url: None,
        wire_api: WireApi::Responses,
        models: vec!["synthetic-model".into()],
    });
    source.protocol_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::Messages,
        adapter: SourceAdapter::MessagesToResponses,
        reasoning_mode: MessagesReasoningMode::Adaptive,
        cache_write_ttl: CacheWriteTtl::Provider,
        model_ids: vec!["synthetic-model".into()],
    }];
    let runtime = GatewayRuntime::from_pool(
        vec![source],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "synthetic-local-key".into(),
            secret: "synthetic-local-secret".into(),
        })],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let relay = local_server::spawn(zenith_relay_core::gateway::router(Arc::new(runtime))).await;
    let client = reqwest::Client::new();
    let initial = file_request();
    let first = client
        .post(format!("{}/v1/messages", relay.base_url))
        .bearer_auth("synthetic-local-secret")
        .json(&initial)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), axum::http::StatusCode::OK);
    let first: Value = first.json().await.unwrap();
    let mut next = continuation_request(&initial, first["content"].clone());
    reverse_object_fields(file_tool_input(&mut next));
    let response = client
        .post(format!("{}/v1/messages", relay.base_url))
        .bearer_auth("synthetic-local-secret")
        .json(&next)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["content"][0]["text"], "File verified.");
    {
        let received = observed.lock().unwrap();
        assert_eq!(
            received.len(),
            2,
            "do not regenerate the already executed file tool"
        );
        let replay = received[1]["input"].as_array().unwrap();
        let original = file_response();
        assert!(replay.contains(&original["output"][0]));
        assert!(replay.contains(&original["output"][1]));
        assert_eq!(
            replay
                .iter()
                .filter(|item| item["type"] == "function_call_output")
                .count(),
            1
        );
    }
    file_tool_input(&mut next)["path"] = json!("different.svg");
    let rejected = client
        .post(format!("{}/v1/messages", relay.base_url))
        .bearer_auth("synthetic-local-secret")
        .json(&next)
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), axum::http::StatusCode::BAD_REQUEST);
    let rejected: Value = rejected.json().await.unwrap();
    assert!(
        rejected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("the adapter no longer has the prior response needed for this continuation"),
        "unexpected synthetic rejection: {rejected}"
    );
    assert_eq!(
        observed.lock().unwrap().len(),
        2,
        "changed history must fail before dispatch"
    );
}
