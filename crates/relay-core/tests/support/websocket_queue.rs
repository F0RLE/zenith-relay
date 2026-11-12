use super::*;

#[path = "websocket_queue/multiplex.rs"]
mod multiplex;

#[path = "websocket_queue/native_forks.rs"]
mod native_forks;

#[path = "websocket_queue/native_concurrent.rs"]
mod native_concurrent;

#[tokio::test]
async fn websocket_queue_keeps_native_requests_fifo_without_extra_dispatches() {
    let release = Arc::new(Notify::new());
    let (upstream, observed) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Hold(release.clone())).await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("queue-source", &upstream, "synthetic-key", 10)],
        vec![],
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, None)).await.unwrap();
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.output_text.delta"
    );
    // The parent is still generating: queued continuations must prepare only at dequeue.
    for index in 1..=2 {
        if index == 2 {
            socket.send(missing_continuation()).await.unwrap();
        }
        socket
            .send(ClientWsMessage::Text(
                json!({
                    "type":"response.create", "model":MODEL,
                    "input":format!("synthetic-{index}"), "previous_response_id":"ws-held-response"
                })
                .to_string(),
            ))
            .await
            .unwrap();
    }
    // An invalid independent lane need not wait for the default lane's terminal.
    assert_missing_continuation(&mut socket).await;
    reject_marker(&mut socket).await;
    assert_eq!(observed.requests.lock().unwrap().len(), 1);
    {
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert!(events[0].input_tokens.is_none());
    }
    for index in 0..3 {
        assert_eq!(
            gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
            1
        );
        release.notify_one();
        receive_websocket_completion(&mut socket).await;
        if index < 2 {
            assert_eq!(
                receive_websocket_json(&mut socket).await["type"],
                "response.output_text.delta"
            );
        }
    }
    let requests = observed.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for (index, request) in requests.iter().enumerate() {
        assert_eq!(request["input"], format!("synthetic-{index}"));
    }
    assert_eq!(observed.headers.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events.iter().filter(|event| event.success).count(), 3);
    assert!(
        events
            .iter()
            .filter(|event| event.success)
            .all(|event| event.attempt == 1)
    );
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        0
    );
}

#[tokio::test]
async fn websocket_queue_survives_http_headers_and_body_waits() {
    for before_headers in [true, false] {
        for stream_id in [None, Some("main")] {
            let release = Arc::new(Notify::new());
            let headers_release = Arc::new(Notify::new());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let upstream = spawn(Router::new().route(
                "/v1/responses",
                post({
                    let requests = requests.clone();
                    let release = release.clone();
                    let headers_release = headers_release.clone();
                    move |Json(body): Json<Value>| {
                        let release = release.clone();
                        let headers_release = headers_release.clone();
                        let first = {
                            let mut requests = requests.lock().unwrap();
                            requests.push(body);
                            requests.len() == 1
                        };
                        async move {
                            if first && before_headers {
                                headers_release.notified().await;
                            }
                            held_stream_response(release)
                        }
                    }
                }),
            ))
            .await;
            let (gateway, events, _, _) = spawn_mixed_gateway(
                vec![source("queue-source", &upstream, "synthetic-key", 10)],
                vec![],
                vec![mixed_key(None, None)],
                Arc::new(TokenAuthority::new(4).unwrap()),
                refresh_adapter(),
                Arc::new(PersistenceAdapter::default()),
            )
            .await;
            let mut socket = connect(&gateway).await;
            socket.send(create(0, stream_id)).await.unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                while requests.lock().unwrap().is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            if !before_headers {
                assert_eq!(
                    receive_websocket_json(&mut socket).await["type"],
                    "response.output_text.delta"
                );
            }
            socket.send(create(1, stream_id)).await.unwrap();
            socket.send(missing_continuation()).await.unwrap();
            socket.send(create(2, stream_id)).await.unwrap();
            reject_marker(&mut socket).await;
            assert_eq!(requests.lock().unwrap().len(), 1);
            assert!(events.lock().unwrap().is_empty());
            if before_headers {
                headers_release.notify_one();
                assert_eq!(
                    receive_websocket_json(&mut socket).await["type"],
                    "response.output_text.delta"
                );
            }
            for index in 0..3 {
                release.notify_one();
                let terminal = receive_websocket_completion(&mut socket).await;
                assert_eq!(terminal.get("stream_id").and_then(Value::as_str), stream_id);
                if index == 1 {
                    assert_missing_continuation(&mut socket).await;
                }
                if index < 2 {
                    assert_eq!(
                        receive_websocket_json(&mut socket).await["type"],
                        "response.output_text.delta"
                    );
                }
            }
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            for (index, request) in requests.iter().enumerate() {
                assert_eq!(request["input"], format!("synthetic-{index}"));
                assert!(request.get("stream_id").is_none());
            }
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 4);
            assert_eq!(events.iter().filter(|event| event.success).count(), 3);
            assert!(
                events
                    .iter()
                    .filter(|event| event.success)
                    .all(|event| event.attempt == 1)
            );
            assert_eq!(
                gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
                0
            );
        }
    }
}

#[tokio::test]
async fn websocket_queue_overflow_and_disconnect_preserve_the_active_attempt() {
    let release = Arc::new(Notify::new());
    let (upstream, observed) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Hold(release.clone())).await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("queue-source", &upstream, "synthetic-key", 10)],
        vec![],
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, None)).await.unwrap();
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.output_text.delta"
    );
    for index in 1..=33 {
        socket.send(create(index, None)).await.unwrap();
    }
    let error = receive_websocket_json(&mut socket).await;
    assert_eq!(error["type"], "error");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("queue is full")
    );
    assert_eq!(observed.requests.lock().unwrap().len(), 1);
    assert!(events.lock().unwrap().is_empty());
    socket
        .send(ClientWsMessage::Close {
            code: reqwest_websocket::CloseCode::Normal,
            reason: String::new(),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    release.notify_one();
    assert_eq!(observed.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("client_cancelled")
    );
}

#[tokio::test]
async fn websocket_queue_continues_after_http_rejection_without_replaying_the_failed_request() {
    let release = Arc::new(Notify::new());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn(Router::new().route("/v1/responses", post({
        let requests = requests.clone();
        let release = release.clone();
        move |Json(body): Json<Value>| {
            let release = release.clone();
            let first = {
                let mut requests = requests.lock().unwrap();
                requests.push(body);
                requests.len() == 1
            };
            async move {
                if first {
                    release.notified().await;
                    Response::builder().status(StatusCode::BAD_REQUEST)
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"error":{"code":"invalid_request","message":"Synthetic rejection"}}"#)).unwrap()
                } else {
                    Response::builder().header(CONTENT_TYPE, "text/event-stream")
                        .body(Body::from("data: {\"type\":\"response.completed\",\"response\":{\"id\":\"after-rejection\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n")).unwrap()
                }
            }
        }
    }))).await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("queue-source", &upstream, "synthetic-key", 10)],
        vec![],
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, Some("main"))).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    socket.send(create(1, Some("main"))).await.unwrap();
    reject_marker(&mut socket).await;
    assert_eq!(requests.lock().unwrap().len(), 1);
    release.notify_one();
    let rejection = receive_websocket_json(&mut socket).await;
    assert_eq!(rejection["type"], "error");
    assert_eq!(rejection["status"], 400);
    assert_eq!(rejection["stream_id"], "main");
    let terminal = receive_websocket_completion(&mut socket).await;
    assert_eq!(terminal["response"]["id"], "after-rejection");
    assert_eq!(terminal["stream_id"], "main");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["input"], "synthetic-0");
    assert_eq!(requests[1]["input"], "synthetic-1");
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(!events[0].success);
    assert!(events[1].success);
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        0
    );
}

#[tokio::test]
async fn websocket_foreign_response_frames_do_not_rewrite_active_usage_or_trigger_retry() {
    for stream_id in [None, Some("main")] {
        for (foreign_lane, delta_first) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let scope = |mut event: Value| {
                if let Some(stream_id) = stream_id {
                    event["stream_id"] = json!(stream_id);
                }
                event
            };
            let foreign = |mut event: Value| {
                event = scope(event);
                if foreign_lane {
                    event["stream_id"] = json!("other-lane");
                }
                event
            };
            let wrong_response_id = if foreign_lane {
                "ws-child"
            } else {
                "ws-parent"
            };
            let first_child_event = if delta_first {
                json!({"type":"response.output_text.delta", "response_id":"ws-child", "delta":"first output"})
            } else {
                json!({"type":"response.created", "response":{"id":"ws-child"}})
            };
            let turns = VecDeque::from([
                vec![scope(json!({"type":"response.completed", "response":{
                    "id":"ws-parent", "status":"completed", "output":[],
                    "usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}
                }}))],
                vec![
                    scope(first_child_event.clone()),
                    foreign(
                        json!({"type":"error", "response_id":wrong_response_id, "status":429,
                        "error":{"code":"usage_limit_reached","message":"Synthetic stale failure"}}),
                    ),
                    foreign(
                        json!({"type":"response.output_text.delta", "response_id":wrong_response_id,
                        "delta":"stale output"}),
                    ),
                    foreign(json!({"type":"response.completed", "response":{
                        "id":wrong_response_id, "status":"completed", "output":[],
                        "usage":{"input_tokens":999,"output_tokens":999,"total_tokens":1998}
                    }})),
                    scope(
                        json!({"type":"response.output_text.delta", "response_id":"ws-child",
                        "delta":"current output"}),
                    ),
                    scope(json!({"type":"response.completed", "response":{
                        "id":"ws-child", "status":"completed", "output":[],
                        "usage":{"input_tokens":7,"output_tokens":2,"total_tokens":9}
                    }})),
                ],
            ]);
            let (upstream, observed) = spawn_websocket_upstream_with_behavior(
                WebSocketBehavior::Sequence(Arc::new(Mutex::new(turns))),
            )
            .await;
            let (gateway, events, _, _) = spawn_mixed_gateway(
                vec![source("scope-source", &upstream, "synthetic-key", 10)],
                vec![],
                vec![mixed_key(None, None)],
                Arc::new(TokenAuthority::new(4).unwrap()),
                refresh_adapter(),
                Arc::new(PersistenceAdapter::default()),
            )
            .await;
            let mut socket = connect(&gateway).await;
            socket.send(create(0, stream_id)).await.unwrap();
            assert_eq!(
                receive_websocket_completion(&mut socket).await["response"]["id"],
                "ws-parent"
            );
            socket
                .send(ClientWsMessage::Text(
                    scope(json!({
                        "type":"response.create", "model":MODEL, "previous_response_id":"ws-parent",
                        "input":"synthetic child turn"
                    }))
                    .to_string(),
                ))
                .await
                .unwrap();
            assert_eq!(
                receive_websocket_json(&mut socket).await,
                scope(first_child_event)
            );
            assert_eq!(
                receive_websocket_json(&mut socket).await["delta"],
                "current output"
            );
            assert_eq!(
                receive_websocket_completion(&mut socket).await["response"]["id"],
                "ws-child"
            );
            assert_eq!(observed.requests.lock().unwrap().len(), 2);
            assert_eq!(observed.headers.lock().unwrap().len(), 1);
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 2);
            assert!(events.iter().all(|event| event.success));
            assert_eq!(events[1].input_tokens, Some(7));
            assert_eq!(events[1].output_tokens, Some(2));
            assert_eq!(
                gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
                0
            );
        }
    }
}

#[tokio::test]
async fn websocket_initial_foreign_lane_frames_do_not_settle_or_escape_to_the_client() {
    for stream_id in [None, Some("main")] {
        let scope = |mut event: Value| {
            if let Some(stream_id) = stream_id {
                event["stream_id"] = json!(stream_id);
            }
            event
        };
        let created = scope(json!({"type":"response.created", "response":{"id":"ws-initial"}}));
        let delta = scope(
            json!({"type":"response.output_text.delta", "response_id":"ws-initial", "delta":"current output"}),
        );
        let completed = scope(json!({"type":"response.completed", "response":{
            "id":"ws-initial", "status":"completed", "output":[],
            "usage":{"input_tokens":7,"output_tokens":2,"total_tokens":9}
        }}));
        let frames = vec![
            AxumWsMessage::Text(
                json!({"type":"error", "stream_id":"other-lane", "status":429,
                "error":{"code":"usage_limit_reached", "message":"Synthetic unrelated failure"}})
                .to_string()
                .into(),
            ),
            AxumWsMessage::Text(created.to_string().into()),
            AxumWsMessage::Binary(
                serde_json::to_vec(&json!({"type":"response.output_text.delta",
                "stream_id":"other-lane", "response_id":"ws-initial", "delta":"foreign output"}))
                .unwrap()
                .into(),
            ),
            AxumWsMessage::Binary(serde_json::to_vec(&delta).unwrap().into()),
            AxumWsMessage::Text(completed.to_string().into()),
        ];
        let (upstream, observed) =
            spawn_websocket_upstream_with_behavior(WebSocketBehavior::Frames(Arc::new(frames)))
                .await;
        let (gateway, events, _, _) = spawn_mixed_gateway(
            vec![source("initial-scope", &upstream, "synthetic-key", 10)],
            vec![],
            vec![mixed_key(None, None)],
            Arc::new(TokenAuthority::new(4).unwrap()),
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        let mut socket = connect(&gateway).await;
        socket.send(create(0, stream_id)).await.unwrap();
        assert_eq!(receive_websocket_json(&mut socket).await, created);
        let binary_delta =
            receive_websocket_data_with_timeout(&mut socket, Duration::from_secs(2)).await;
        let ClientWsMessage::Binary(binary_delta) = binary_delta else {
            panic!("the current delta must retain its binary frame");
        };
        assert_eq!(binary_delta.as_ref(), serde_json::to_vec(&delta).unwrap());
        assert_eq!(receive_websocket_json(&mut socket).await, completed);
        assert_eq!(observed.requests.lock().unwrap().len(), 1);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].success);
        assert_eq!(events[0].input_tokens, Some(7));
        assert_eq!(events[0].output_tokens, Some(2));
    }
}

#[tokio::test]
async fn websocket_queue_recovers_from_invalid_initial_frames_without_reconnecting() {
    for use_native in [true, false] {
        let (native, native_state) = spawn_websocket_upstream().await;
        let (http, http_state) = spawn_upstream(vec![successful_sse_reply()]).await;
        let upstream = if use_native { &native } else { &http };
        let (gateway, events, _, _) = spawn_mixed_gateway(
            vec![source("queue-source", upstream, "synthetic-key", 10)],
            vec![],
            vec![mixed_key(None, None)],
            Arc::new(TokenAuthority::new(4).unwrap()),
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        let mut socket = connect(&gateway).await;
        socket
            .send(ClientWsMessage::Text(
                json!({"type":"response.create", "stream_id":"bad-first", "input":"missing model"})
                    .to_string(),
            ))
            .await
            .unwrap();
        socket.send(missing_continuation()).await.unwrap();
        socket.send(create(0, None)).await.unwrap();
        let first_error = receive_websocket_json(&mut socket).await;
        assert_eq!(first_error["type"], "error");
        assert_eq!(first_error["stream_id"], "bad-first");
        assert_eq!(first_error["error"]["code"], "invalid_request");
        assert_missing_continuation(&mut socket).await;
        receive_websocket_completion(&mut socket).await;
        assert_eq!(
            native_state.requests.lock().unwrap().len(),
            usize::from(use_native)
        );
        assert_eq!(
            http_state.requests.lock().unwrap().len(),
            usize::from(!use_native)
        );
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(!events[0].success);
        assert!(events[1].success);
        assert_eq!(
            gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
            0
        );
    }
}

fn missing_continuation() -> ClientWsMessage {
    ClientWsMessage::Text(
        json!({
            "type":"response.create", "stream_id":"rejected-turn", "model":MODEL,
            "previous_response_id":"missing-synthetic-response", "input":"synthetic rejected turn"
        })
        .to_string(),
    )
}

async fn assert_missing_continuation(socket: &mut reqwest_websocket::WebSocket) {
    let error = receive_websocket_json(socket).await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["stream_id"], "rejected-turn");
    assert_eq!(error["error"]["code"], "response_continuation_unavailable");
}

fn create(index: usize, stream_id: Option<&str>) -> ClientWsMessage {
    let mut body =
        json!({"type":"response.create", "model":MODEL, "input":format!("synthetic-{index}")});
    if let Some(stream_id) = stream_id {
        body["stream_id"] = json!(stream_id);
    }
    if index.is_multiple_of(2) {
        ClientWsMessage::Text(body.to_string())
    } else {
        ClientWsMessage::Binary(serde_json::to_vec(&body).unwrap().into())
    }
}

async fn connect(gateway: &TestServer) -> reqwest_websocket::WebSocket {
    reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap()
        .into_websocket()
        .await
        .unwrap()
}

async fn reject_marker(socket: &mut reqwest_websocket::WebSocket) {
    socket
        .send(ClientWsMessage::Text("invalid JSON".into()))
        .await
        .unwrap();
    assert_eq!(receive_websocket_json(socket).await["type"], "error");
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type":"response.interrupt", "stream_id":"control-only",
                "mode":"unsupported", "response_id":"synthetic-response"
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let error = receive_websocket_json(socket).await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["stream_id"], "control-only");
}
