use super::*;
use crate::{
    GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeContinuationStores,
    RuntimeLocalKey, RuntimeSource,
};
use axum::{Json, Router, extract::State, routing::post};
use serde_json::json;
use std::sync::Mutex;

#[tokio::test]
async fn http_agent_messages_survive_restart_and_reject_unavailable_history_without_dispatch() {
    type Captured = Arc<Mutex<Vec<Value>>>;
    async fn upstream(State(captured): State<Captured>, Json(request): Json<Value>) -> Json<Value> {
        captured.lock().unwrap().push(request);
        Json(
            json!({"id":"resp_synthetic","model":"synthetic-model","status":"completed",
            "output":[{"type":"message","id":"msg_synthetic","role":"assistant","status":"completed",
                "content":[{"type":"output_text","text":"synthetic reply"}]}],
            "usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}}),
        )
    }
    let captured = Captured::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/v1/responses", post(upstream))
        .with_state(captured.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let build_runtime = |protected_key| {
        Arc::new(
            GatewayRuntime::from_pool(
                vec![RuntimeSource::unrestricted(ProviderSource {
                    id: "source".into(),
                    name: "source".into(),
                    base_url: format!("http://{address}/v1"),
                    api_key: "synthetic-upstream".into(),
                    wire_api: WireApi::Responses,
                    models: vec!["synthetic-model".into()],
                    proxy_url: None,
                })],
                vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                    id: "key".into(),
                    secret: "synthetic-local".into(),
                })],
                GatewayRuntimeOptions {
                    continuation_state: Some(RuntimeContinuationStores::with_agent_message_key(
                        protected_key,
                    )),
                    ..Default::default()
                },
                Arc::new(|_| {}),
            )
            .unwrap(),
        )
    };
    // Mint the saved history in a different host with no shared in-memory state.
    let sealed = build_runtime([19; 32])
        .agent_message_context("key")
        .seal_argument("collaboration.spawn_agent", "message", "synthetic task")
        .unwrap();
    let runtime = build_runtime([19; 32]);
    for (encrypted, expected_status) in [
        (sealed, StatusCode::OK),
        ("foreign-provider-ciphertext".to_owned(), StatusCode::OK),
        (
            runtime
                .agent_message_context("another-key")
                .seal_argument("collaboration.spawn_agent", "message", "synthetic task")
                .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
        (
            build_runtime([20; 32])
                .agent_message_context("key")
                .seal_argument("collaboration.spawn_agent", "message", "synthetic task")
                .unwrap(),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let request = Request::builder().method("POST").uri("/v1/responses")
            .header("host", "localhost").header("authorization", "Bearer synthetic-local")
            .header("content-type", "application/json")
            .body(Body::from(json!({"model":"synthetic-model","input":[{
                "type":"agent_message","author":"/root","recipient":"/root/worker","encrypted_content":encrypted
            }]}).to_string())).unwrap();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            execute_client_request(runtime.clone(), request, WireApi::Responses),
        )
        .await
        .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        assert_eq!(
            status,
            expected_status,
            "{}",
            String::from_utf8_lossy(&body)
        );
        assert!(!String::from_utf8_lossy(&body).contains("synthetic task"));
        if status == StatusCode::BAD_REQUEST {
            let error: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                error["error"]["code"],
                crate::AdapterError::agent_message_unavailable().code()
            );
            assert_eq!(
                error["error"]["param"],
                "input.agent_message.encrypted_content"
            );
            assert!(
                error["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("start a new chat")
            );
        }
    }
    let invalid = runtime
        .agent_message_context("another-key")
        .seal_argument("collaboration.spawn_agent", "message", "synthetic task")
        .unwrap();
    let key = runtime
        .authenticate(Some(&axum::http::HeaderValue::from_static(
            "Bearer synthetic-local",
        )))
        .unwrap();
    let response =
        super::super::account::execute_account_endpoint(super::super::account::AccountExecution {
            runtime: runtime.clone(),
            key,
            request: json!({"input":[{"type":"agent_message","encrypted_content":invalid}]}),
            requested_model: "synthetic-model".into(),
            resolved_model: "synthetic-model".into(),
            client_headers: Default::default(),
            endpoint: crate::gateway::request::AccountEndpoint::Compact,
            responses_lite: None,
            rewrite_model: false,
            wait_for_candidate_availability: false,
            request_origin: None,
        })
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        error["error"]["code"],
        crate::AdapterError::agent_message_unavailable().code()
    );
    assert_eq!(
        error["error"]["param"],
        "input.agent_message.encrypted_content"
    );
    server.abort();
    let requests = captured.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "another local or host key must fail before any upstream dispatch"
    );
    assert_eq!(requests[0]["input"][0]["role"], "user");
    assert_eq!(
        requests[0]["input"][0]["content"][1]["text"],
        "synthetic task"
    );
    assert_eq!(requests[1]["input"][0]["type"], "agent_message");
    assert_eq!(
        requests[1]["input"][0]["encrypted_content"],
        "foreign-provider-ciphertext"
    );
}
