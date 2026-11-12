use super::*;
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message, Upgrade};

#[tokio::test]
async fn strict_output_with_complete_tool_history_can_leave_an_incompatible_bps_owner() {
    for websocket in [false, true] {
        let (server, runtime) = runtime_with_tool_owner().await;
        let (base, task) = super::recovery::gateway(runtime.clone()).await;
        let request = strict_request();
        if websocket {
            let response = websocket_response(&base, request.clone()).await;
            assert_eq!(response["status"], "completed", "{response}");
        } else {
            let response = super::retry_admission::bounded_client()
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let body: Value = response.json().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{body}");
        }
        task.abort();
        let bodies = crate::poison::mutex(&server.state.generation_bodies);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["text"]["format"], request["text"]["format"]);
        assert_eq!(bodies[0]["input"], request["input"]);
        let affinity = runtime
            .tool_call_affinity_key("key-1", "synthetic-call")
            .unwrap();
        assert_eq!(
            runtime
                .response_affinity_candidate(&affinity, crate::unix_time_ms())
                .as_deref(),
            Some("synthetic-account-0")
        );
    }
}

#[tokio::test]
async fn strict_output_cannot_move_an_opaque_predecessor_or_unpaired_tool_result() {
    for opaque in [false, true] {
        let (server, runtime) = runtime_with_tool_owner().await;
        let mut request = strict_request();
        if opaque {
            runtime.bind_response_affinity(
                Some("resp-synthetic"),
                "synthetic-account-0",
                crate::unix_time_ms(),
            );
            request["previous_response_id"] = json!("resp-synthetic");
            request["input"] = json!("Synthetic follow-up");
        } else {
            request["input"].as_array_mut().unwrap().remove(0);
        }
        let (base, task) = super::recovery::gateway(runtime).await;
        let response = super::retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&request)
            .send()
            .await
            .unwrap();
        task.abort();
        assert!(response.status().is_client_error());
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 0);
    }
}

async fn runtime_with_tool_owner() -> (AccessServer, Arc<GatewayRuntime>) {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    server
        .state
        .stream_generations
        .store(true, Ordering::SeqCst);
    *crate::poison::mutex(&server.state.generation_body) = json!({
        "id":"synthetic-review","model":"gpt-test","status":"completed",
        "output":[{"type":"message","role":"assistant","content":[{
            "type":"output_text","text":"{\"approved\":false}"
        }]}]
    });
    let runtime = Arc::new(super::retry_admission::pool_runtime(&server, 1, true).await);
    runtime.bind_tool_call_affinity(
        "key-1",
        "synthetic-call",
        "synthetic-account-0",
        crate::unix_time_ms(),
    );
    (server, runtime)
}

fn strict_request() -> Value {
    let format = json!({
        "type":"json_schema","name":"command_review","strict":true,
        "schema":{
            "type":"object","properties":{"approved":{"type":"boolean"}},
            "required":["approved"],"additionalProperties":false
        }
    });
    let input = json!([
        {"type":"function_call","call_id":"synthetic-call","name":"inspect","arguments":"{}"},
        {"type":"function_call_output","call_id":"synthetic-call","output":"synthetic result"}
    ]);
    json!({
        "model":"gpt-test","input":input,
        "tools":[{"type":"function","name":"inspect","parameters":{"type":"object","properties":{}}}],
        "text":{"format":format}
    })
}

async fn websocket_response(base: &str, mut request: Value) -> Value {
    request["type"] = json!("response.create");
    let mut socket = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .upgrade()
        .send()
        .await
        .unwrap()
        .into_websocket()
        .await
        .unwrap();
    socket
        .send(Message::Text(request.to_string()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(frame) = socket.next().await {
            let Message::Text(text) = frame.unwrap() else {
                continue;
            };
            let event: Value = serde_json::from_str(&text).unwrap();
            assert_ne!(event["type"], "error", "{event}");
            if event["type"] == "response.completed" {
                return event["response"].clone();
            }
        }
        panic!("synthetic WebSocket closed before response.completed");
    })
    .await
    .expect("synthetic WebSocket did not complete")
}
