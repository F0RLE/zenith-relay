use super::*;
use axum::{
    Router,
    extract::ws::{Message as ServerMessage, WebSocketUpgrade},
    routing::get,
};
use std::collections::VecDeque;

struct Fixture {
    url: url::Url,
    server: tokio::task::JoinHandle<()>,
    received: Arc<Mutex<Vec<Value>>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn start(rounds: Vec<Vec<Value>>) -> Self {
        Self::start_raw(
            rounds
                .into_iter()
                .map(|events| events.into_iter().map(|event| event.to_string()).collect())
                .collect(),
        )
        .await
    }

    async fn start_raw(rounds: Vec<Vec<String>>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url: url::Url = format!("http://{}/v1/responses", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let capture = received.clone();
        let rounds = Arc::new(Mutex::new(VecDeque::from(rounds)));
        let router = Router::new().route(
            "/v1/responses",
            get(move |upgrade: WebSocketUpgrade| {
                let capture = capture.clone();
                let rounds = rounds.clone();
                async move {
                    upgrade.on_upgrade(move |mut socket| async move {
                        while let Some(Ok(message)) = socket.recv().await {
                            let text = match message {
                                ServerMessage::Text(text) => text,
                                ServerMessage::Ping(_) | ServerMessage::Pong(_) => {
                                    if socket.flush().await.is_err() {
                                        return;
                                    }
                                    continue;
                                }
                                _ => return,
                            };
                            crate::poison::mutex(&capture)
                                .push(serde_json::from_str(&text).unwrap());
                            let events = crate::poison::mutex(&rounds)
                                .pop_front()
                                .unwrap_or_default();
                            if events.is_empty() {
                                break;
                            }
                            for event in events {
                                if socket
                                    .send(ServerMessage::Text(event.into()))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }
                    })
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            url,
            server,
            received,
        }
    }

    async fn pending(&self, store: &SocketStore, body: &Value) -> PendingSocket {
        let mut lane = store
            .acquire(planner::fingerprint(&json!("conversation")))
            .unwrap();
        let reused = lane.idle.is_some();
        let connection = if let Some(connection) = lane.idle.take() {
            connection.resume().await.unwrap()
        } else {
            let socket = transport::upgrade(reqwest::Client::new().get(self.url.clone()))
                .await
                .unwrap()
                .into_websocket()
                .await
                .unwrap();
            IdleConnection {
                socket,
                opened: Instant::now(),
                checkpoint: None,
            }
        };
        let plan = plan(body, connection.checkpoint.as_ref());
        let mut payload =
            RequestPayloadDiagnostics::identity(plan.payload.as_bytes(), TransportMode::Native);
        payload.full_request_reason = plan.full_request_reason;
        payload.socket_output_mismatch = plan.output_mismatch.clone();
        let payload_diagnostics = Some(payload);
        PendingSocket {
            lane,
            connection,
            plan,
            response_id: None,
            completed_output: crate::gateway::NativeReplayCapture::default(),
            url: self.url.clone(),
            diagnostics: SocketDiagnostics {
                reused,
                handshake_ms: (!reused).then_some(1),
                reset_reason: (!reused).then_some(ConnectionResetReason::NewSession),
            },
            payload_diagnostics,
        }
    }

    fn requests(&self) -> Vec<Value> {
        crate::poison::mutex(&self.received).clone()
    }
}

fn completed(id: &str, output: Value) -> Value {
    json!({"type":"response.completed","response":{"id":id,"status":"completed","output":output,
        "usage":{"input_tokens":1000,"input_tokens_details":{"cached_tokens":900},"output_tokens":8}}})
}

#[tokio::test]
async fn wire_tool_cycle_reuses_socket_and_sends_only_new_items() {
    let call = json!({"type":"function_call","id":"fc_test","call_id":"call_test",
        "name":"lookup","arguments":"{}","status":"completed","namespace":null,
        "encrypted_function_args":null,"encrypted_content":null,"async":null});
    let fixture = Fixture::start(vec![
        vec![completed("first", json!([call]))],
        vec![completed("second", json!([]))],
    ])
    .await;
    let store = SocketStore::default();
    let first = request(json!([{"role":"user","content":"synthetic"}]));
    let response = fixture.pending(&store, &first).await.send().await.unwrap();
    assert!(
        !response
            .extensions()
            .get::<SocketDiagnostics>()
            .unwrap()
            .reused
    );
    let text = response.text().await.unwrap();
    assert!(text.contains("response.completed"));
    assert!(text.contains("cached_tokens"));
    let mut next = first.clone();
    let result = json!({"type":"function_call_output","call_id":"call_test","output":"ok"});
    let mut replay = call;
    for field in [
        "namespace",
        "encrypted_function_args",
        "encrypted_content",
        "async",
    ] {
        replay.as_object_mut().unwrap().remove(field);
    }
    next["input"]
        .as_array_mut()
        .unwrap()
        .extend([replay, result.clone()]);
    let response = fixture.pending(&store, &next).await.send().await.unwrap();
    assert!(
        response
            .extensions()
            .get::<SocketDiagnostics>()
            .unwrap()
            .reused
    );
    assert_eq!(
        response
            .extensions()
            .get::<RequestPayloadDiagnostics>()
            .unwrap()
            .mode,
        crate::usage::RequestPayloadMode::Continuation
    );
    response.text().await.unwrap();
    let sent = fixture.requests();
    assert_eq!(sent.len(), 2);
    assert!(sent[0].get("previous_response_id").is_none());
    assert_eq!(sent[1]["previous_response_id"], "first");
    assert_eq!(sent[1]["input"], json!([result]));
}

#[tokio::test]
async fn completed_item_snapshots_keep_the_socket_and_exact_continuation() {
    let reasoning = json!({"type":"reasoning","id":"rs_test","summary":[],
        "encrypted_content":"synthetic-completed-cipher","status":"completed"});
    let call = json!({"type":"function_call","id":"fc_test","call_id":"call_test",
        "name":"lookup","arguments":"{}","status":"completed"});
    for output in [
        Value::Null,
        json!([]),
        json!([{"type":"reasoning","id":"rs_test","summary":[],
            "encrypted_content":"synthetic-terminal-cipher","status":"completed"},call]),
    ] {
        let mut terminal = completed("first", output.clone());
        if output.is_null() {
            terminal["response"]
                .as_object_mut()
                .unwrap()
                .remove("output");
        }
        let fixture = Fixture::start(vec![
            vec![
                json!({"type":"response.created","response":{"id":"first"}}),
                json!({"type":"response.output_item.done","output_index":0,"item":reasoning}),
                json!({"type":"response.output_item.done","output_index":1,"item":call}),
                terminal,
            ],
            vec![completed("second", json!([]))],
        ])
        .await;
        let store = SocketStore::default();
        let mut body = request(json!([{"role":"user","content":"synthetic"}]));
        fixture
            .pending(&store, &body)
            .await
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        let result = json!({"type":"function_call_output","call_id":"call_test","output":"ok"});
        body["input"].as_array_mut().unwrap().extend([
            reasoning.clone(),
            call.clone(),
            result.clone(),
        ]);
        let response = fixture.pending(&store, &body).await.send().await.unwrap();
        assert!(
            response
                .extensions()
                .get::<SocketDiagnostics>()
                .unwrap()
                .reused,
            "completed output_item snapshots must retain the same socket"
        );
        response.text().await.unwrap();
        let sent = fixture.requests();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1]["previous_response_id"], "first");
        assert_eq!(sent[1]["input"], json!([result]));
    }
}

#[tokio::test]
async fn idle_heartbeat_is_answered_without_a_generation_or_next_client_turn() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url: url::Url = format!("http://{}/v1/responses", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let capture = received.clone();
    let (observed, observation) = tokio::sync::oneshot::channel();
    let observed = Arc::new(Mutex::new(Some(observed)));
    let router = Router::new().route("/v1/responses", get(move |upgrade: WebSocketUpgrade| {
        let capture = capture.clone();
        let observed = observed.clone();
        async move { upgrade.on_upgrade(move |mut socket| async move {
            let Some(Ok(ServerMessage::Text(text))) = socket.recv().await else { return; };
            crate::poison::mutex(&capture).push(serde_json::from_str(&text).unwrap());
            let output = json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic"}]}]);
            socket.send(ServerMessage::Text(completed("first", output).to_string().into())).await.unwrap();
            socket.send(ServerMessage::Ping(b"synthetic-heartbeat".to_vec().into())).await.unwrap();
            let alive = matches!(tokio::time::timeout(Duration::from_secs(1), socket.recv()).await,
                Ok(Some(Ok(ServerMessage::Pong(payload)))) if payload.as_ref() == b"synthetic-heartbeat");
            if let Some(observed) = crate::poison::mutex(&observed).take() { let _ = observed.send(alive); }
            if !alive { return; }
            let Some(Ok(ServerMessage::Text(text))) = socket.recv().await else { return; };
            crate::poison::mutex(&capture).push(serde_json::from_str(&text).unwrap());
            socket.send(ServerMessage::Text(completed("second", json!([])).to_string().into())).await.unwrap();
        }) }
    }));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let fixture = Fixture {
        url,
        server,
        received,
    };
    let store = SocketStore::default();
    let first = request(json!([{"role":"user","content":"synthetic"}]));
    fixture
        .pending(&store, &first)
        .await
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), observation)
            .await
            .unwrap()
            .unwrap(),
        "idle upstream heartbeats must not wait for the next client request"
    );
    assert_eq!(
        fixture.requests().len(),
        1,
        "control frames must not create a generation"
    );
    let mut next = first;
    next["input"].as_array_mut().unwrap().extend([
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic"}]}),
        json!({"role":"user","content":"next"}),
    ]);
    let response = fixture.pending(&store, &next).await.send().await.unwrap();
    assert!(
        response
            .extensions()
            .get::<SocketDiagnostics>()
            .unwrap()
            .reused
    );
    response.text().await.unwrap();
    assert_eq!(fixture.requests()[1]["previous_response_id"], "first");
}

#[tokio::test]
async fn disconnect_and_cancel_discard_connection_without_replay() {
    for cancelled in [false, true] {
        let fixture = Fixture::start(vec![vec![]]).await;
        let store = SocketStore::default();
        let response = fixture
            .pending(&store, &request(json!([])))
            .await
            .send()
            .await
            .unwrap();
        if cancelled {
            drop(response);
        } else {
            assert!(response.text().await.is_err());
        }
        assert!(
            store
                .acquire(planner::fingerprint(&json!("conversation")))
                .unwrap()
                .idle
                .is_none()
        );
        assert!(fixture.requests().len() <= 1);
    }
    assert_eq!(
        AuthorizedRequestError::WebsocketSend.execution_certainty(),
        crate::scheduler::rotation::ExecutionCertainty::Unknown
    );
}

#[tokio::test]
async fn failed_incomplete_and_foreign_terminal_never_reuse() {
    let failures = [
        vec![
            json!({"type":"error","status":429,"error":{"code":"rate_limit_exceeded","message":"synthetic"}}),
        ],
        vec![json!({"type":"response.failed","response":{"id":"r","status":"failed","output":[]}})],
        vec![
            json!({"type":"response.incomplete","response":{"id":"r","status":"incomplete","output":[]}}),
        ],
        vec![
            json!({"type":"response.created","response":{"id":"first"}}),
            completed("foreign", json!([])),
        ],
        vec![json!({"type":"response.cancelled","response":{"id":"r","status":"cancelled"}})],
        vec![
            json!({"type":"response.output_text.delta","output_index":0,"delta":"synthetic"}),
            completed("r", json!([])),
        ],
        vec![
            json!({"type":"response.output_item.done","output_index":0,"item":{
                "type":"reasoning","id":"rs_original","encrypted_content":"synthetic-cipher"}}),
            completed("r", json!([{"type":"reasoning","id":"rs_foreign"}])),
        ],
        vec![json!({"type":"response.completed","response":{"id":"r",
            "status":"completed","model":"synthetic-model","error":{
                "code":"synthetic_provider_error","message":"Synthetic rejection"},
            "output":[{"type":"message","role":"assistant","content":[{
                "type":"output_text","text":"Synthetic output"}]}]}})],
        vec![json!({"type":"response.completed","response":{"id":"r",
            "status":"completed","model":"synthetic-other-model",
            "output":[{"type":"message","role":"assistant","content":[{
                "type":"output_text","text":"Synthetic output"}]}]}})],
        vec![json!({"type":"response.completed","error":{
            "code":"synthetic_provider_error","message":"Synthetic rejection"},
            "response":{"id":"r","status":"completed","model":"synthetic-model",
            "output":[{"type":"message","role":"assistant","content":[{
                "type":"output_text","text":"Synthetic output"}]}]}})],
        vec![
            json!({"type":"response.created","response":{"id":"r","model":"synthetic-other-model"}}),
            completed(
                "r",
                json!([{"type":"message","role":"assistant","content":[{
                "type":"output_text","text":"Synthetic output"}]}]),
            ),
        ],
    ];
    for (index, events) in failures.into_iter().enumerate() {
        let fixture = Fixture::start(vec![events]).await;
        let store = SocketStore::default();
        let result = fixture
            .pending(&store, &request(json!([])))
            .await
            .send()
            .await
            .unwrap()
            .text()
            .await;
        if index == 3 {
            assert!(result.is_err());
        } else {
            assert!(result.is_ok());
        }
        assert!(
            store
                .acquire(planner::fingerprint(&json!("conversation")))
                .unwrap()
                .idle
                .is_none(),
            "invalid checkpoint case {index}"
        );
        assert_eq!(fixture.requests().len(), 1);
    }
}

#[tokio::test]
async fn sparse_output_gap_discards_checkpoint_before_the_next_turn() {
    let call = json!({"type":"function_call","id":"fc_synthetic","call_id":"call_synthetic",
        "name":"lookup","arguments":"{}","status":"completed"});
    let fixture = Fixture::start(vec![
        vec![
            json!({"type":"response.output_item.done","output_index":1,"item":call}),
            completed("first", json!([])),
        ],
        vec![completed("second", json!([]))],
    ])
    .await;
    let store = SocketStore::default();
    let first = request(json!([{"role":"user","content":"synthetic"}]));
    fixture
        .pending(&store, &first)
        .await
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let mut next = first;
    next["input"].as_array_mut().unwrap().extend([
        call,
        json!({"type":"function_call_output","call_id":"call_synthetic","output":"ok"}),
    ]);
    let response = fixture.pending(&store, &next).await.send().await.unwrap();
    assert!(
        !response
            .extensions()
            .get::<SocketDiagnostics>()
            .unwrap()
            .reused
    );
    assert_eq!(
        response
            .extensions()
            .get::<RequestPayloadDiagnostics>()
            .unwrap()
            .full_request_reason,
        Some(crate::usage::FullRequestReason::SocketCheckpointUnavailable)
    );
    response.text().await.unwrap();
    let sent = fixture.requests();
    assert_eq!(sent.len(), 2);
    assert!(sent[1].get("previous_response_id").is_none());
    assert_eq!(sent[1]["input"], next["input"]);
}

#[tokio::test]
async fn ambiguous_json_cannot_be_rewritten_into_a_retryable_error() {
    let body = r#"{"type":"error","error":{"code":"unknown","code":"rate_limit_exceeded"}}"#;
    let fixture = Fixture::start_raw(vec![vec![body.into()]]).await;
    let store = SocketStore::default();
    let response = fixture
        .pending(&store, &request(json!([])))
        .await
        .send()
        .await
        .unwrap();
    assert!(response.text().await.is_err());
    assert_eq!(fixture.requests().len(), 1);
    assert!(
        store
            .acquire(planner::fingerprint(&json!("conversation")))
            .unwrap()
            .idle
            .is_none()
    );
}
