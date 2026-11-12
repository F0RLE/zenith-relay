use super::*;

#[tokio::test]
async fn websocket_native_forks_keep_each_lanes_latest_response_on_one_physical_socket() {
    for parent_lane in [None, Some("source")] {
        for incomplete_parent in [false, true] {
            let lanes = [parent_lane, Some("fork"), parent_lane, Some("fork")];
            let ids = ["root", "child", "source-next", "fork-next"];
            let previous = [None, Some("root"), Some("root"), Some("child")];
            let turns = lanes.iter().zip(ids).enumerate().map(|(index, (lane, id))| {
                let incomplete = index == 0 && incomplete_parent;
                let mut event = json!({
                    "type":if incomplete { "response.incomplete" } else { "response.completed" },
                    "response":{
                        "id":id, "status":if incomplete { "incomplete" } else { "completed" },
                        "output":[], "usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}
                    }
                });
                if incomplete {
                    event["response"]["incomplete_details"] = json!({"reason":"max_output_tokens"});
                }
                if let Some(lane) = lane {
                    event["stream_id"] = json!(lane);
                }
                vec![event]
            }).collect::<VecDeque<_>>();
            let (upstream, observed) = spawn_websocket_upstream_with_behavior(
                WebSocketBehavior::Sequence(Arc::new(Mutex::new(turns))),
            )
            .await;
            let (gateway, events, _, _) = spawn_mixed_gateway(
                vec![source("native-fork", &upstream, "synthetic-key", 10)],
                vec![],
                vec![mixed_key(None, None)],
                Arc::new(TokenAuthority::new(4).unwrap()),
                refresh_adapter(),
                Arc::new(PersistenceAdapter::default()),
            )
            .await;
            let mut socket = connect(&gateway).await;
            for index in 0..4 {
                let mut request = json!({
                    "type":"response.create", "model":MODEL,
                    "input":format!("synthetic-{index}"), "store":false
                });
                if let Some(lane) = lanes[index] {
                    request["stream_id"] = json!(lane);
                }
                if let Some(id) = previous[index] {
                    request["previous_response_id"] = json!(id);
                }
                socket
                    .send(ClientWsMessage::Text(request.to_string()))
                    .await
                    .unwrap();
                let event = receive_websocket_json(&mut socket).await;
                assert_eq!(event["response"]["id"], ids[index], "{event}");
                assert_eq!(event.get("stream_id").and_then(Value::as_str), lanes[index]);
            }
            let requests = observed.requests.lock().unwrap();
            assert_eq!(requests.len(), 4);
            for (index, request) in requests.iter().enumerate() {
                assert_eq!(
                    request.get("previous_response_id").and_then(Value::as_str),
                    previous[index]
                );
                assert_eq!(
                    request.get("stream_id").and_then(Value::as_str),
                    lanes[index]
                );
                assert_eq!(request["input"], format!("synthetic-{index}"));
            }
            assert_eq!(
                observed.headers.lock().unwrap().len(),
                1,
                "forks must reuse the actual upstream socket"
            );
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 4);
            assert_eq!(
                events.iter().filter(|event| event.success).count(),
                if incomplete_parent { 3 } else { 4 }
            );
            assert!(events.iter().all(|event| event.attempt == 1));
            assert_eq!(
                gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
                0
            );
        }
    }
}

#[tokio::test]
async fn websocket_native_fork_failure_keeps_parent_but_same_lane_failure_retires_it() {
    for same_lane in [false, true] {
        for after_output in [false, true] {
            let failed_lane = if same_lane { "source" } else { "fork" };
            let mut failure = vec![];
            if after_output {
                failure.push(
                    json!({"type":"response.output_text.delta", "stream_id":failed_lane,
                    "response_id":"failed", "delta":"partial"}),
                );
            }
            failure.push(json!({"type":"response.failed", "stream_id":failed_lane,
            "response":{"id":"failed", "status":"failed", "output":[],
                "error":{"code":"invalid_request_error", "message":"Synthetic rejection"},
                "usage":{"input_tokens":2,"output_tokens":if after_output { 1 } else { 0 }}
            }}));
            let turns = VecDeque::from([
                vec![
                    json!({"type":"response.incomplete", "stream_id":"source", "response":{
                        "id":"root", "status":"incomplete", "output":[],
                        "incomplete_details":{"reason":"max_output_tokens"},
                        "usage":{"input_tokens":2,"output_tokens":1}
                    }}),
                ],
                failure,
                vec![
                    json!({"type":"response.completed", "stream_id":"source", "response":{
                        "id":"continued", "status":"completed", "output":[],
                        "usage":{"input_tokens":3,"output_tokens":1}
                    }}),
                ],
            ]);
            let (upstream, observed) = spawn_websocket_upstream_with_behavior(
                WebSocketBehavior::Sequence(Arc::new(Mutex::new(turns))),
            )
            .await;
            let (gateway, events, _, _) = spawn_mixed_gateway(
                vec![source("failed-native-fork", &upstream, "synthetic-key", 10)],
                vec![],
                vec![mixed_key(None, None)],
                Arc::new(TokenAuthority::new(4).unwrap()),
                refresh_adapter(),
                Arc::new(PersistenceAdapter::default()),
            )
            .await;
            let mut socket = connect(&gateway).await;
            socket.send(create(0, Some("source"))).await.unwrap();
            assert_eq!(
                receive_websocket_json(&mut socket).await["type"],
                "response.incomplete"
            );
            for lane in [failed_lane, "source"] {
                socket.send(ClientWsMessage::Text(json!({
                    "type":"response.create", "model":MODEL, "stream_id":lane,
                    "previous_response_id":"root", "input":"synthetic continuation", "store":false
                }).to_string())).await.unwrap();
            }
            if after_output {
                let delta = receive_websocket_json(&mut socket).await;
                assert_eq!(delta["delta"], "partial");
                assert_eq!(delta["stream_id"], failed_lane);
            }
            let failed = receive_websocket_json(&mut socket).await;
            assert_eq!(failed["type"], "response.failed");
            assert_eq!(failed["stream_id"], failed_lane);
            let continued = receive_websocket_json(&mut socket).await;
            if same_lane {
                assert_eq!(continued["type"], "error");
                assert_eq!(
                    continued["error"]["code"],
                    "response_continuation_unavailable"
                );
            } else {
                assert_eq!(continued["type"], "response.completed");
                assert_eq!(continued["response"]["id"], "continued");
            }
            assert_eq!(continued["stream_id"], "source");
            assert_eq!(observed.headers.lock().unwrap().len(), 1);
            assert_eq!(
                observed.requests.lock().unwrap().len(),
                if same_lane { 2 } else { 3 }
            );
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 3);
            assert!(!events[0].success);
            assert!(!events[1].success);
            assert_eq!(events[1].input_tokens, Some(2));
            assert_eq!(
                events[1].output_tokens,
                Some(if after_output { 1 } else { 0 })
            );
            assert_eq!(events[2].success, !same_lane);
            assert_eq!(
                gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
                0
            );
        }
    }
}

#[tokio::test]
async fn websocket_native_socket_replacement_clears_all_lane_local_continuations() {
    let turns = [
        ("source", "root", "incomplete"),
        ("fork", "child", "incomplete"),
        ("replacement", "fresh", "completed"),
    ]
    .into_iter()
    .map(|(lane, id, status)| {
        let mut event = json!({"type":format!("response.{status}"), "stream_id":lane,
            "response":{"id":id, "status":status, "output":[],
                "usage":{"input_tokens":2,"output_tokens":1}}});
        if status == "incomplete" {
            event["response"]["incomplete_details"] = json!({"reason":"max_output_tokens"});
        }
        vec![event]
    })
    .collect::<VecDeque<_>>();
    let (upstream, observed) = spawn_websocket_upstream_with_behavior(WebSocketBehavior::Sequence(
        Arc::new(Mutex::new(turns)),
    ))
    .await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source(
            "replaced-native-fork",
            &upstream,
            "synthetic-key",
            10,
        )],
        vec![],
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let mut socket = connect(&gateway).await;
    for (lane, previous, response) in [
        ("source", None, "root"),
        ("fork", Some("root"), "child"),
        ("replacement", None, "fresh"),
    ] {
        let mut request = json!({"type":"response.create", "model":MODEL,
            "stream_id":lane, "input":"synthetic turn", "store":false});
        if let Some(id) = previous {
            request["previous_response_id"] = json!(id);
        }
        socket
            .send(ClientWsMessage::Text(request.to_string()))
            .await
            .unwrap();
        assert_eq!(
            receive_websocket_json(&mut socket).await["response"]["id"],
            response
        );
    }
    for (lane, previous) in [("source", "root"), ("fork", "child")] {
        socket.send(ClientWsMessage::Text(json!({"type":"response.create", "model":MODEL,
            "stream_id":lane, "input":"synthetic stale continuation", "previous_response_id":previous
        }).to_string())).await.unwrap();
        let error = receive_websocket_json(&mut socket).await;
        assert_eq!(error["type"], "error");
        assert_eq!(error["error"]["code"], "response_continuation_unavailable");
        assert_eq!(error["stream_id"], lane);
    }
    assert_eq!(observed.headers.lock().unwrap().len(), 2);
    assert_eq!(observed.requests.lock().unwrap().len(), 3);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 5);
    for event in &events[3..] {
        http_rejection::assert_local_continuation_rejection_transport(
            std::slice::from_ref(event),
            zenith_relay_core::UsageTransport::Websocket,
        );
    }
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        0
    );
}

#[tokio::test]
async fn websocket_native_named_limit_keeps_default_and_existing_lanes_available() {
    fn terminal(lane: Option<&str>, id: &str) -> Vec<Value> {
        let mut event = json!({"type":"response.completed", "response":{
            "id":id, "status":"completed", "output":[],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        }});
        if let Some(lane) = lane {
            event["stream_id"] = json!(lane);
        }
        vec![event]
    }
    let mut turns = VecDeque::from([terminal(Some("source"), "root")]);
    for index in 1..32 {
        turns.push_back(terminal(
            Some(&format!("fork-{index}")),
            &format!("child-{index}"),
        ));
    }
    turns.push_back(terminal(None, "default-child"));
    turns.push_back(terminal(Some("fork-1"), "reused-child"));
    let (upstream, observed) = spawn_websocket_upstream_with_behavior(WebSocketBehavior::Sequence(
        Arc::new(Mutex::new(turns)),
    ))
    .await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source(
            "limited-native-fork",
            &upstream,
            "synthetic-key",
            10,
        )],
        vec![],
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, Some("source"))).await.unwrap();
    assert_eq!(
        receive_websocket_completion(&mut socket).await["response"]["id"],
        "root"
    );
    let lanes = (1..32).map(|index| Some(format!("fork-{index}"))).chain([
        None,
        Some("overflow".into()),
        Some("fork-1".into()),
    ]);
    for lane in lanes {
        let mut request = json!({"type":"response.create", "model":MODEL,
            "input":"synthetic fork", "previous_response_id":"root", "store":false});
        if let Some(lane) = lane.as_deref() {
            request["stream_id"] = json!(lane);
        }
        socket
            .send(ClientWsMessage::Text(request.to_string()))
            .await
            .unwrap();
        let event = receive_websocket_json(&mut socket).await;
        assert_eq!(
            event.get("stream_id").and_then(Value::as_str),
            lane.as_deref()
        );
        if lane.as_deref() == Some("overflow") {
            assert_eq!(event["type"], "error");
            assert_eq!(event["error"]["code"], "websocket_stream_limit_reached");
        } else {
            assert_eq!(event["type"], "response.completed");
        }
    }
    assert_eq!(observed.headers.lock().unwrap().len(), 1);
    assert_eq!(observed.requests.lock().unwrap().len(), 34);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 34);
    assert!(
        events
            .iter()
            .all(|event| event.success && event.attempt == 1)
    );
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        0
    );
}
