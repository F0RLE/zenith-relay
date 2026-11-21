use super::*;

#[tokio::test]
async fn websocket_http_multiplex_preserves_lane_fifo_and_isolates_late_failure() {
    for lane_a in [None, Some("a")] {
        for fail_first in [false, true] {
            let gates: Arc<Vec<Notify>> = Arc::new((0..3).map(|_| Notify::new()).collect());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let upstream = spawn(Router::new().route(
                "/v1/responses",
                post({
                    let gates = gates.clone();
                    let requests = requests.clone();
                    move |Json(body): Json<Value>| {
                        let input = body["input"].as_str().unwrap().to_owned();
                        let index: usize =
                            input.strip_prefix("synthetic-").unwrap().parse().unwrap();
                        requests.lock().unwrap().push(index);
                        let gates = gates.clone();
                        async move {
                            let initial = format!(
                                "data: {}\n\n",
                                json!({"type":"response.output_text.delta", "delta":input})
                            );
                            let terminal = format!(
                                "data: {}\n\n",
                                json!({"type":"response.completed", "response":{
                                    "id":format!("resp-{index}"), "status":"completed", "output":[],
                                    "usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}
                                }})
                            );
                            let chunks = futures_util::stream::once(async move {
                                Ok::<_, std::io::Error>(Bytes::from(initial))
                            })
                            .chain(futures_util::stream::once(async move {
                                gates[index].notified().await;
                                if index == 0 && fail_first {
                                    Err(std::io::Error::other("synthetic interrupted stream"))
                                } else {
                                    Ok(Bytes::from(terminal))
                                }
                            }));
                            Response::builder()
                                .header(CONTENT_TYPE, "text/event-stream")
                                .body(Body::from_stream(chunks))
                                .unwrap()
                        }
                    }
                }),
            ))
            .await;
            let (gateway, events, _, _) = spawn_mixed_gateway(
                vec![source("multiplex-source", &upstream, "synthetic-key", 10)],
                vec![],
                vec![mixed_key(None, None)],
                Arc::new(TokenAuthority::new(4).unwrap()),
                refresh_adapter(),
                Arc::new(PersistenceAdapter::default()),
            )
            .await;
            gateway
                .runtime
                .as_ref()
                .unwrap()
                .set_codex_websockets_enabled(false);
            let mut socket = connect(&gateway).await;
            socket
                .send(ClientWsMessage::Text("{".into()))
                .await
                .unwrap();
            assert_eq!(receive_websocket_json(&mut socket).await["type"], "error");
            let mut invalid = json!({"type":"response.create"});
            if let Some(lane) = lane_a {
                invalid["stream_id"] = json!(lane);
            }
            socket
                .send(ClientWsMessage::Text(invalid.to_string()))
                .await
                .unwrap();
            assert_eq!(receive_websocket_json(&mut socket).await["type"], "error");
            socket.send(create(0, lane_a)).await.unwrap();
            socket.send(create(1, Some("b"))).await.unwrap();
            socket
                .send(ClientWsMessage::Text(invalid.to_string()))
                .await
                .unwrap();
            socket.send(create(2, lane_a)).await.unwrap();
            let first = receive_websocket_json(&mut socket).await;
            let second = receive_websocket_json(&mut socket).await;
            let mut outputs = vec![
                first["delta"].as_str().unwrap(),
                second["delta"].as_str().unwrap(),
            ];
            outputs.sort();
            assert_eq!(outputs, ["synthetic-0", "synthetic-1"]);
            assert_eq!(
                requests.lock().unwrap().len(),
                2,
                "same-lane work must wait"
            );
            assert_eq!(
                gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
                2
            );
            socket
                .send(ClientWsMessage::Text(
                    json!({
                        "type":"unsupported-control", "stream_id":"b"
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            let control_error = receive_websocket_json(&mut socket).await;
            assert_eq!(control_error["type"], "error");
            assert_eq!(control_error["stream_id"], "b");
            assert_eq!(requests.lock().unwrap().len(), 2);
            gates[0].notify_one();
            let terminal = receive_websocket_json(&mut socket).await;
            assert_eq!(
                terminal["type"],
                if fail_first {
                    "response.failed"
                } else {
                    "response.completed"
                }
            );
            if fail_first {
                assert_eq!(terminal["response"]["status"], "failed");
                assert_eq!(terminal["response"]["error"]["code"], "upstream_stream");
            }
            assert_eq!(terminal.get("stream_id").and_then(Value::as_str), lane_a);
            let invalid_error = receive_websocket_json(&mut socket).await;
            assert_eq!(invalid_error["type"], "error");
            assert_eq!(
                invalid_error.get("stream_id").and_then(Value::as_str),
                lane_a
            );
            let next = receive_websocket_json(&mut socket).await;
            assert_eq!(next["delta"], "synthetic-2");
            assert_eq!(next.get("stream_id").and_then(Value::as_str), lane_a);
            assert_eq!(requests.lock().unwrap().len(), 3);
            gates[1].notify_one();
            gates[2].notify_one();
            for _ in 0..2 {
                assert_eq!(
                    receive_websocket_json(&mut socket).await["type"],
                    "response.completed"
                );
            }
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 3);
            assert_eq!(
                events.iter().filter(|event| event.success).count(),
                if fail_first { 2 } else { 3 }
            );
            assert_eq!(
                gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
                0
            );
        }
    }
}

#[tokio::test]
async fn websocket_http_multiplex_bounds_active_and_named_lanes_and_cancels_all_owners() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(Notify::new());
    let upstream = spawn(Router::new().route("/v1/responses", post({
        let requests = requests.clone();
        let gate = gate.clone();
        move |Json(body): Json<Value>| {
            requests.lock().unwrap().push(body);
            let gate = gate.clone();
            async move {
                let chunks = futures_util::stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"held\"}\n\n")) })
                    .chain(futures_util::stream::once(async move {
                        gate.notified().await;
                        Ok(Bytes::from_static(b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"held\",\"status\":\"completed\",\"output\":[]}}\n\n"))
                    }));
                Response::builder().header(CONTENT_TYPE, "text/event-stream").body(Body::from_stream(chunks)).unwrap()
            }
        }
    }))).await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("bounded-multiplex", &upstream, "synthetic-key", 10)],
        vec![],
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    gateway
        .runtime
        .as_ref()
        .unwrap()
        .set_codex_websockets_enabled(false);
    let mut socket = connect(&gateway).await;
    for index in 0..16 {
        socket
            .send(create(index, Some(&format!("lane-{index}"))))
            .await
            .unwrap();
    }
    for _ in 0..16 {
        assert_eq!(receive_websocket_json(&mut socket).await["delta"], "held");
    }
    for index in 16..32 {
        socket
            .send(create(index, Some(&format!("lane-{index}"))))
            .await
            .unwrap();
    }
    socket.send(create(32, None)).await.unwrap();
    socket.send(create(33, Some("lane-33"))).await.unwrap();
    let error = receive_websocket_json(&mut socket).await;
    assert_eq!(error["error"]["code"], "websocket_stream_limit_reached");
    assert_eq!(error["stream_id"], "lane-33");
    assert_eq!(requests.lock().unwrap().len(), 16);
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        16
    );
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
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 16);
    assert!(events.iter().all(|event| !event.success));
    assert_eq!(
        requests.lock().unwrap().len(),
        16,
        "queued work must not start after disconnect"
    );
}

#[tokio::test]
async fn websocket_http_multiplex_forks_completed_state_and_keeps_transport_after_toggle() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn(Router::new().route(
        "/v1/responses",
        post({
            let requests = requests.clone();
            move |Json(body): Json<Value>| {
                let index = {
                    let mut requests = requests.lock().unwrap();
                    requests.push(body);
                    requests.len()
                };
                async move {
                    Response::builder()
                        .header(CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(format!(
                            "data: {}\n\n",
                            json!({"type":"response.completed", "response":{
                                "id":format!("fork-{index}"), "status":"completed", "output":[],
                                "usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}
                            }})
                        )))
                        .unwrap()
                }
            }
        }),
    ))
    .await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("fork-multiplex", &upstream, "synthetic-key", 10)],
        vec![],
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap();
    runtime.set_codex_websockets_enabled(false);
    let mut socket = connect(&gateway).await;
    socket.send(create(0, Some("parent"))).await.unwrap();
    assert_eq!(
        receive_websocket_completion(&mut socket).await["response"]["id"],
        "fork-1"
    );
    runtime.set_codex_websockets_enabled(true);
    for lane in ["child-a", "child-b"] {
        socket
            .send(ClientWsMessage::Text(
                json!({
                    "type":"response.create", "model":MODEL, "stream_id":lane,
                    "previous_response_id":"fork-1", "input":format!("synthetic-{lane}")
                })
                .to_string(),
            ))
            .await
            .unwrap();
    }
    let mut lanes = Vec::new();
    for _ in 0..2 {
        let completed = receive_websocket_completion(&mut socket).await;
        lanes.push(completed["stream_id"].as_str().unwrap().to_owned());
    }
    lanes.sort();
    assert_eq!(lanes, ["child-a", "child-b"]);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for request in &requests[1..] {
        assert_eq!(request["previous_response_id"], "fork-1");
        assert!(request.get("stream_id").is_none());
    }
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(
        events
            .iter()
            .all(|event| event.success && event.attempt == 1)
    );
    assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 0);
}
