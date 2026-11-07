use super::*;

#[path = "native_concurrent/authorization.rs"]
mod authorization;
#[path = "native_concurrent/default_errors.rs"]
mod default_errors;
#[path = "native_concurrent/notifications.rs"]
mod notifications;
#[path = "native_concurrent/policy_updates.rs"]
mod policy_updates;
#[path = "native_concurrent/queued_parent.rs"]
mod queued_parent;

type EventSender = tokio::sync::mpsc::UnboundedSender<Option<Value>>;

#[derive(Clone, Default)]
struct NativeProbe {
    requests: Arc<Mutex<Vec<Value>>>,
    sockets: Arc<Mutex<Vec<EventSender>>>,
    reject_upgrades: Arc<std::sync::atomic::AtomicBool>,
    http_requests: Arc<Mutex<Vec<Value>>>,
    headers: Arc<Mutex<Vec<HeaderMap>>>,
}

impl NativeProbe {
    async fn wait_requests(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.requests.lock().unwrap().len() < count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    fn emit(&self, event: Value) {
        self.sockets
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .send(Some(event))
            .unwrap();
    }

    fn complete(&self, lane: Option<&str>, id: &str, input_tokens: u64) {
        let mut event = json!({"type":"response.completed", "response":{
            "id":id, "status":"completed", "output":[],
            "usage":{"input_tokens":input_tokens,"output_tokens":1,"total_tokens":input_tokens+1}
        }});
        if id == "native-0" {
            event["response"]["output"] = json!([{
                "type":"message", "id":"synthetic-message", "role":"assistant", "status":"completed",
                "content":[{"type":"output_text", "text":"synthetic startup", "annotations":[]}]
            }]);
        }
        if let Some(lane) = lane {
            event["stream_id"] = json!(lane);
        }
        self.emit(event);
    }

    async fn wait_settled(&self, gateway: &TestServer) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

async fn fixture() -> (TestServer, NativeProbe) {
    fixture_with_startup(true).await
}

async fn fixture_with_startup(eager_startup: bool) -> (TestServer, NativeProbe) {
    let probe = NativeProbe::default();
    let upstream = spawn(Router::new().route("/v1/responses", get({
        let probe = probe.clone();
        move |upgrade: WebSocketUpgrade, headers: HeaderMap| {
            let probe = probe.clone();
            async move {
                if probe.reject_upgrades.load(Ordering::SeqCst) {
                    return Response::builder().status(StatusCode::METHOD_NOT_ALLOWED).body(Body::empty()).unwrap();
                }
                probe.headers.lock().unwrap().push(headers);
                upgrade.on_upgrade(move |mut socket| async move {
                    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
                    probe.sockets.lock().unwrap().push(sender);
                    let mut first_request = true;
                    loop {
                        tokio::select! {
                            event = receiver.recv() => {
                                let Some(Some(event)) = event else { break; };
                                if socket.send(AxumWsMessage::Text(event.to_string().into())).await.is_err() { break; }
                            }
                            message = socket.recv() => {
                                let bytes = match message {
                                    Some(Ok(AxumWsMessage::Text(text))) => text.as_bytes().to_vec(),
                                    Some(Ok(AxumWsMessage::Binary(bytes))) => bytes.to_vec(),
                                    Some(Ok(AxumWsMessage::Ping(_) | AxumWsMessage::Pong(_))) => continue,
                                    _ => break,
                                };
                                let request: Value = serde_json::from_slice(&bytes).unwrap();
                                let index = {
                                    let mut requests = probe.requests.lock().unwrap();
                                    requests.push(request.clone());
                                    requests.len() - 1
                                };
                                if !eager_startup || request["input"] == "synthetic delayed creation" {
                                    continue;
                                }
                                let mut event = json!({"type":"response.created", "response":{
                                    "id":format!("native-{index}"), "status":"in_progress", "output":[]
                                }});
                                if let Some(lane) = request.get("stream_id") { event["stream_id"] = lane.clone(); }
                                if socket.send(AxumWsMessage::Text(event.to_string().into())).await.is_err() { break; }
                                if first_request {
                                    first_request = false;
                                    event["type"] = json!("response.output_text.delta");
                                    event["response_id"] = event["response"]["id"].clone();
                                    event.as_object_mut().unwrap().remove("response");
                                    event["delta"] = json!("synthetic startup");
                                    if socket.send(AxumWsMessage::Text(event.to_string().into())).await.is_err() { break; }
                                }
                            }
                        }
                    }
                })
            }
        }
    }).post({
        let probe = probe.clone();
        move |Json(request): Json<Value>| {
            let index = {
                let mut requests = probe.http_requests.lock().unwrap();
                requests.push(request);
                requests.len()
            };
            async move {
                let event = json!({"type":"response.completed","response":{
                    "id":format!("native-fallback-{index}"), "status":"completed", "output":[],
                    "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
                }});
                Response::builder().header(CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(format!("data: {event}\n\n"))).unwrap()
            }
        }
    }))).await;
    (upstream, probe)
}

#[tokio::test]
async fn websocket_native_bootstrap_admits_parallel_requests_before_any_upstream_event() {
    let (upstream, probe) = fixture_with_startup(false).await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    for (index, lane) in ["a", "b"].iter().enumerate() {
        socket.send(create(index, Some(lane))).await.unwrap();
    }
    probe.wait_requests(2).await;
    assert_eq!(probe.sockets.lock().unwrap().len(), 1);
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        2
    );
    assert!(events.lock().unwrap().is_empty());
    for (index, lane) in ["b", "a"].iter().enumerate() {
        let id = if index == 0 { "native-1" } else { "native-0" };
        probe.complete(Some(lane), id, 2 + index as u64);
        let event = receive_websocket_json(&mut socket).await;
        assert_eq!(event["type"], "response.completed");
        assert_eq!(event["stream_id"], *lane);
        assert_eq!(event["response"]["id"], id);
    }
    probe.wait_settled(&gateway).await;
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event.success && event.attempt == 1)
    );
}

#[tokio::test]
async fn websocket_native_bootstrap_disconnect_releases_all_unacknowledged_owners() {
    let (upstream, probe) = fixture_with_startup(false).await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    for (index, lane) in ["a", "b", "c"].iter().enumerate() {
        socket.send(create(index, Some(lane))).await.unwrap();
    }
    probe.wait_requests(3).await;
    socket
        .send(ClientWsMessage::Close {
            code: reqwest_websocket::CloseCode::Normal,
            reason: String::new(),
        })
        .await
        .unwrap();
    probe.wait_settled(&gateway).await;
    assert_eq!(probe.sockets.lock().unwrap().len(), 1);
    assert_eq!(probe.requests.lock().unwrap().len(), 3);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(
        events
            .iter()
            .all(|event| !event.success && event.attempt == 1 && event.input_tokens.is_none())
    );
}

async fn gateway_for(upstream: &TestServer) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("native-concurrent", upstream, "synthetic-key", 10)],
        vec![],
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    (gateway, events)
}

#[tokio::test]
async fn websocket_native_bootstrap_repeated_missing_parent_keeps_usage_and_stops_repair() {
    let (upstream, probe) = fixture_with_startup(false).await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, Some("source"))).await.unwrap();
    probe.wait_requests(1).await;
    probe.complete(Some("source"), "native-0", 2);
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.completed"
    );
    continuation(&mut socket, "source", "synthetic continuation").await;
    socket.send(create(2, Some("other"))).await.unwrap();
    probe.wait_requests(3).await;
    let rejection = |input_tokens| {
        json!({
            "type":"error", "stream_id":"source", "status":400,
            "error":{"type":"invalid_request_error", "code":"previous_response_not_found", "param":"previous_response_id", "message":"Synthetic missing parent"},
            "usage":{"input_tokens":input_tokens,"output_tokens":0}
        })
    };
    probe.emit(rejection(3));
    tokio::time::timeout(Duration::from_secs(3), async {
        while events.lock().unwrap().len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(probe.sockets.lock().unwrap().len(), 1);
    assert_eq!(probe.requests.lock().unwrap().len(), 3);
    probe.complete(Some("other"), "native-2", 4);
    assert_eq!(
        receive_websocket_json(&mut socket).await["stream_id"],
        "other"
    );
    probe.wait_requests(4).await;
    assert_eq!(probe.sockets.lock().unwrap().len(), 2);
    probe.emit(rejection(5));
    let error = receive_websocket_json(&mut socket).await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["stream_id"], "source");
    probe.wait_settled(&gateway).await;
    assert_eq!(probe.requests.lock().unwrap().len(), 4);
    socket.send(create(4, Some("next"))).await.unwrap();
    probe.wait_requests(5).await;
    probe.complete(Some("next"), "native-4", 6);
    assert_eq!(
        receive_websocket_json(&mut socket).await["stream_id"],
        "next"
    );
    probe.wait_settled(&gateway).await;
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 5);
    let failures = events
        .iter()
        .filter(|event| !event.success)
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 2);
    assert_eq!(failures[0].request_id, failures[1].request_id);
    assert_eq!((failures[0].attempt, failures[1].attempt), (1, 2));
    assert_eq!(
        (failures[0].input_tokens, failures[1].input_tokens),
        (Some(3), Some(5))
    );
}

#[tokio::test]
async fn websocket_native_bootstrap_unknown_forwarded_output_is_not_replayed() {
    let (upstream, probe) = fixture().await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, Some("source"))).await.unwrap();
    created(&mut socket, Some("source"), 0).await;
    socket.send(create(1, Some("other"))).await.unwrap();
    created(&mut socket, Some("other"), 1).await;
    probe.emit(json!({"type":"synthetic.future_output","stream_id":"other","value":"opaque synthetic state"}));
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "synthetic.future_output"
    );
    probe.emit(json!({"type":"error","stream_id":"other","status":503,
        "error":{"code":"server_error","message":"Synthetic rejection after unknown output"},
        "usage":{"input_tokens":2,"output_tokens":0}
    }));
    assert_eq!(receive_websocket_json(&mut socket).await["type"], "error");
    probe.complete(Some("source"), "native-0", 3);
    assert_eq!(
        receive_websocket_json(&mut socket).await["stream_id"],
        "source"
    );
    probe.wait_settled(&gateway).await;
    assert_eq!(probe.requests.lock().unwrap().len(), 2);
    assert_eq!(probe.sockets.lock().unwrap().len(), 1);
    assert_eq!(events.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn websocket_native_bootstrap_bounds_setup_frames_and_releases_all_owners() {
    let (upstream, probe) = fixture_with_startup(false).await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    for (index, lane) in ["a", "b"].iter().enumerate() {
        socket.send(create(index, Some(lane))).await.unwrap();
    }
    probe.wait_requests(2).await;
    for _ in 0..=1024 {
        probe.emit(json!({"type":"response.created","stream_id":"a","response":{"id":"native-0","status":"in_progress","output":[]}}));
    }
    let mut lanes = BTreeSet::new();
    for _ in 0..2 {
        let error = receive_websocket_json(&mut socket).await;
        assert_eq!(error["type"], "error");
        lanes.insert(error["stream_id"].as_str().unwrap().to_owned());
    }
    assert_eq!(lanes, BTreeSet::from(["a".into(), "b".into()]));
    probe.wait_settled(&gateway).await;
    assert_eq!(probe.requests.lock().unwrap().len(), 2);
    assert_eq!(events.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn websocket_native_bootstrap_disconnect_retains_reported_setup_usage() {
    let (upstream, probe) = fixture_with_startup(false).await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    for (index, lane) in ["a", "b"].iter().enumerate() {
        socket.send(create(index, Some(lane))).await.unwrap();
    }
    probe.wait_requests(2).await;
    for (index, lane, input) in [(0, "a", 7), (1, "b", 11)] {
        probe.emit(
            json!({"type":"response.created", "stream_id":lane, "response":{
                "id":format!("native-{index}"), "status":"in_progress", "output":[],
                "usage":{"input_tokens":input, "output_tokens":0}
            }}),
        );
    }
    let created = receive_websocket_json(&mut socket).await;
    assert_eq!(created["type"], "response.created");
    assert_eq!(created["stream_id"], "b");
    probe
        .sockets
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .send(None)
        .unwrap();
    for _ in 0..2 {
        assert_eq!(receive_websocket_json(&mut socket).await["type"], "error");
    }
    probe.wait_settled(&gateway).await;
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| !event.success && event.output_tokens == Some(0))
    );
    let mut inputs = events
        .iter()
        .map(|event| event.input_tokens.unwrap())
        .collect::<Vec<_>>();
    inputs.sort_unstable();
    assert_eq!(inputs, [7, 11]);
    assert_eq!(probe.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn websocket_native_bootstrap_http_handoff_preserves_deferred_retries_and_queued_work() {
    let (upstream, probe) = fixture_with_startup(false).await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, Some("root"))).await.unwrap();
    probe.wait_requests(1).await;
    probe.complete(Some("root"), "native-0", 2);
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.completed"
    );
    for lane in ["a", "b"] {
        continuation(&mut socket, lane, "synthetic fork").await;
    }
    socket.send(create(3, Some("held"))).await.unwrap();
    probe.wait_requests(4).await;
    for lane in ["a", "b"] {
        probe.emit(json!({"type":"error","stream_id":lane,"status":400,
            "error":{"type":"invalid_request_error","code":"previous_response_not_found","message":"Synthetic missing parent"},
            "usage":{"input_tokens":2,"output_tokens":0}
        }));
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while events.lock().unwrap().len() < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    socket.send(create(4, Some("b"))).await.unwrap();
    probe.reject_upgrades.store(true, Ordering::SeqCst);
    probe.complete(Some("held"), "native-3", 4);
    assert_eq!(
        receive_websocket_json(&mut socket).await["stream_id"],
        "held"
    );
    for (index, lane) in ["a", "b", "b"].iter().enumerate() {
        let event = receive_websocket_json(&mut socket).await;
        assert_eq!(event["type"], "response.completed", "{event}");
        assert_eq!(event["stream_id"], *lane);
        assert_eq!(
            event["response"]["id"],
            format!("native-fallback-{}", index + 1)
        );
    }
    probe.wait_settled(&gateway).await;
    assert_eq!(probe.requests.lock().unwrap().len(), 4);
    assert_eq!(probe.sockets.lock().unwrap().len(), 1);
    {
        let requests = probe.http_requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for request in &requests[..2] {
            assert!(
                request
                    .get("previous_response_id")
                    .is_none_or(Value::is_null)
            );
            let input = request["input"].as_array().unwrap();
            assert!(input.iter().any(|item| item["role"] == "assistant"));
            assert_eq!(input.last().unwrap()["content"], "synthetic fork");
        }
        assert_eq!(requests[2]["input"], "synthetic-4");
    }
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 7);
    let failures = events
        .iter()
        .filter(|event| !event.success)
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 2);
    for failed in failures {
        assert!(events.iter().any(|event| event.success
            && event.request_id == failed.request_id
            && event.attempt == 2));
    }
}

async fn created(socket: &mut reqwest_websocket::WebSocket, lane: Option<&str>, index: usize) {
    let event = receive_websocket_json(socket).await;
    assert_eq!(event["type"], "response.created", "{event}");
    assert_eq!(
        event.get("stream_id").and_then(Value::as_str),
        lane,
        "{event}"
    );
    assert_eq!(
        event["response"]["id"],
        format!("native-{index}"),
        "{event}"
    );
    if index == 0 {
        assert_eq!(
            receive_websocket_json(socket).await["delta"],
            "synthetic startup"
        );
    }
}

#[tokio::test]
async fn websocket_native_concurrent_lanes_share_one_socket_and_preserve_lane_fifo() {
    let (upstream, probe) = fixture().await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, Some("a"))).await.unwrap();
    created(&mut socket, Some("a"), 0).await;
    socket.send(create(1, Some("a"))).await.unwrap();
    socket.send(create(2, Some("b"))).await.unwrap();
    created(&mut socket, Some("b"), 1).await;
    socket.send(create(3, None)).await.unwrap();
    created(&mut socket, None, 2).await;
    assert_eq!(probe.sockets.lock().unwrap().len(), 1);
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        3
    );
    for (lane, id) in [
        (Some("b"), "native-1"),
        (None, "native-2"),
        (Some("a"), "native-0"),
    ] {
        let mut event =
            json!({"type":"response.output_text.delta","response_id":id,"delta":"synthetic"});
        if let Some(lane) = lane {
            event["stream_id"] = json!(lane);
        }
        probe.emit(event);
        assert_eq!(receive_websocket_json(&mut socket).await["response_id"], id);
    }
    probe.complete(Some("a"), "native-0", 11);
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.completed"
    );
    created(&mut socket, Some("a"), 3).await;
    probe.complete(None, "native-2", 7);
    assert_eq!(
        receive_websocket_json(&mut socket).await["response"]["id"],
        "native-2"
    );
    probe.complete(Some("b"), "native-1", 3);
    assert_eq!(
        receive_websocket_json(&mut socket).await["response"]["id"],
        "native-1"
    );
    probe.complete(Some("a"), "native-3", 5);
    assert_eq!(
        receive_websocket_json(&mut socket).await["response"]["id"],
        "native-3"
    );
    probe.wait_settled(&gateway).await;
    let requests = probe.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|request| request["input"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["synthetic-0", "synthetic-2", "synthetic-3", "synthetic-1"]
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 4);
    assert!(
        events
            .iter()
            .all(|event| event.success && event.attempt == 1)
    );
    let mut tokens = events
        .iter()
        .map(|event| event.input_tokens.unwrap())
        .collect::<Vec<_>>();
    tokens.sort_unstable();
    assert_eq!(tokens, [3, 5, 7, 11]);
}

#[tokio::test]
async fn websocket_native_concurrent_active_limit_includes_the_default_lane() {
    let (upstream, probe) = fixture().await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    let lanes = (1..=16)
        .map(|index| format!("lane-{index}"))
        .collect::<Vec<_>>();
    socket.send(create(0, None)).await.unwrap();
    created(&mut socket, None, 0).await;
    for index in 1..16 {
        socket
            .send(create(index, Some(&lanes[index - 1])))
            .await
            .unwrap();
        created(&mut socket, Some(&lanes[index - 1]), index).await;
    }
    socket.send(create(16, Some(&lanes[15]))).await.unwrap();
    reject_marker(&mut socket).await;
    assert_eq!(probe.requests.lock().unwrap().len(), 16);
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        16
    );
    probe.complete(None, "native-0", 2);
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.completed"
    );
    created(&mut socket, Some(&lanes[15]), 16).await;
    for index in 1..=16 {
        probe.complete(Some(&lanes[index - 1]), &format!("native-{index}"), 2);
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.completed"
        );
    }
    probe.wait_settled(&gateway).await;
    assert_eq!(probe.sockets.lock().unwrap().len(), 1);
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.success)
            .count(),
        17
    );
}

#[tokio::test]
async fn websocket_native_concurrent_disconnect_settles_every_owner_without_replay() {
    for upstream_disconnect in [false, true] {
        let (upstream, probe) = fixture().await;
        let (gateway, events) = gateway_for(&upstream).await;
        let mut socket = connect(&gateway).await;
        for (index, lane) in ["a", "b", "c"].iter().enumerate() {
            socket.send(create(index, Some(lane))).await.unwrap();
            created(&mut socket, Some(lane), index).await;
        }
        probe.emit(json!({"type":"response.output_text.delta", "stream_id":"a", "response_id":"native-0", "delta":"synthetic partial"}));
        assert_eq!(
            receive_websocket_json(&mut socket).await["delta"],
            "synthetic partial"
        );
        if upstream_disconnect {
            probe
                .sockets
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .send(None)
                .unwrap();
            let mut lanes = BTreeSet::new();
            for _ in 0..2 {
                let error = receive_websocket_json(&mut socket).await;
                assert_eq!(error["type"], "error");
                lanes.insert(error["stream_id"].as_str().unwrap().to_owned());
            }
            assert_eq!(lanes, BTreeSet::from(["b".into(), "c".into()]));
        } else {
            socket
                .send(ClientWsMessage::Close {
                    code: reqwest_websocket::CloseCode::Normal,
                    reason: String::new(),
                })
                .await
                .unwrap();
        }
        probe.wait_settled(&gateway).await;
        assert_eq!(probe.requests.lock().unwrap().len(), 3);
        assert_eq!(probe.sockets.lock().unwrap().len(), 1);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert!(
            events
                .iter()
                .all(|event| !event.success && event.attempt == 1)
        );
    }
}

#[tokio::test]
async fn websocket_native_concurrent_failed_default_and_foreign_events_do_not_end_siblings() {
    let (upstream, probe) = fixture().await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, None)).await.unwrap();
    created(&mut socket, None, 0).await;
    socket.send(create(1, Some("b"))).await.unwrap();
    created(&mut socket, Some("b"), 1).await;
    for event in [
        json!({"type":"response.output_text.delta", "stream_id":"foreign", "response_id":"native-1", "delta":"foreign"}),
        json!({"type":"response.completed", "stream_id":"b", "response":{"id":"native-0", "status":"completed","output":[],"usage":{"input_tokens":999}}}),
    ] {
        probe.emit(event);
    }
    probe.emit(json!({"type":"response.failed", "response":{
        "id":"native-0", "status":"failed", "output":[],
        "error":{"code":"invalid_request_error","message":"Synthetic rejection"},
        "usage":{"input_tokens":2,"output_tokens":0}
    }}));
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.failed"
    );
    probe.complete(Some("b"), "native-1", 4);
    assert_eq!(receive_websocket_json(&mut socket).await["stream_id"], "b");
    probe.wait_settled(&gateway).await;
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(!events[0].success);
    assert_eq!(events[0].input_tokens, Some(2));
    assert!(events[1].success);
    assert_eq!(events[1].input_tokens, Some(4));
}

async fn continuation(socket: &mut reqwest_websocket::WebSocket, lane: &str, input: &str) {
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type":"response.create", "model":MODEL, "stream_id":lane, "store":false,
                "previous_response_id":"native-0", "input":[{"role":"user","content":input}]
            })
            .to_string(),
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn websocket_native_concurrent_fork_and_source_keep_one_physical_connection() {
    for fork_fails in [false, true] {
        let (upstream, probe) = fixture().await;
        let (gateway, events) = gateway_for(&upstream).await;
        let mut socket = connect(&gateway).await;
        socket.send(create(0, Some("source"))).await.unwrap();
        created(&mut socket, Some("source"), 0).await;
        probe.complete(Some("source"), "native-0", 2);
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.completed"
        );
        continuation(&mut socket, "fork", "synthetic fork").await;
        created(&mut socket, Some("fork"), 1).await;
        probe.emit(json!({"type":"response.in_progress","stream_id":"fork","response":{"id":"native-1","status":"in_progress","output":[]}}));
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.in_progress"
        );
        continuation(&mut socket, "source", "synthetic source continuation").await;
        created(&mut socket, Some("source"), 2).await;
        assert_eq!(
            gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
            2
        );
        if fork_fails {
            probe.emit(
                json!({"type":"response.failed","stream_id":"fork","response":{
                    "id":"native-1","status":"failed","output":[],
                    "error":{"code":"invalid_request_error","message":"Synthetic rejection"},
                    "usage":{"input_tokens":3,"output_tokens":0}
                }}),
            );
        } else {
            probe.complete(Some("fork"), "native-1", 3);
        }
        let fork = receive_websocket_json(&mut socket).await;
        assert_eq!(
            fork["type"],
            if fork_fails {
                "response.failed"
            } else {
                "response.completed"
            }
        );
        probe.complete(Some("source"), "native-2", 5);
        assert_eq!(
            receive_websocket_json(&mut socket).await["response"]["id"],
            "native-2"
        );
        probe.wait_settled(&gateway).await;
        assert_eq!(probe.sockets.lock().unwrap().len(), 1);
        let requests = probe.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for request in &requests[1..] {
            assert_eq!(request["previous_response_id"], "native-0");
        }
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(
            events.iter().filter(|event| event.success).count(),
            if fork_fails { 2 } else { 3 }
        );
    }
}

#[tokio::test]
async fn websocket_native_concurrent_repair_drains_siblings_before_replacing_socket() {
    let (upstream, probe) = fixture().await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, Some("source"))).await.unwrap();
    created(&mut socket, Some("source"), 0).await;
    probe.complete(Some("source"), "native-0", 2);
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.completed"
    );
    continuation(&mut socket, "source", "synthetic continuation").await;
    created(&mut socket, Some("source"), 1).await;
    socket.send(create(2, Some("other"))).await.unwrap();
    created(&mut socket, Some("other"), 2).await;
    probe.emit(json!({"type":"error", "stream_id":"source", "status":400,
        "error":{"type":"invalid_request_error","code":"previous_response_not_found","param":"previous_response_id","message":"Synthetic missing parent"}
    }));
    reject_marker(&mut socket).await;
    assert_eq!(probe.sockets.lock().unwrap().len(), 1);
    assert_eq!(probe.requests.lock().unwrap().len(), 3);
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        1
    );
    probe.complete(Some("other"), "native-2", 4);
    assert_eq!(
        receive_websocket_json(&mut socket).await["response"]["id"],
        "native-2"
    );
    created(&mut socket, Some("source"), 3).await;
    assert_eq!(
        receive_websocket_json(&mut socket).await["delta"],
        "synthetic startup"
    );
    probe.complete(Some("source"), "native-3", 6);
    assert_eq!(
        receive_websocket_json(&mut socket).await["response"]["id"],
        "native-3"
    );
    probe.wait_settled(&gateway).await;
    assert_eq!(probe.sockets.lock().unwrap().len(), 2);
    let requests = probe.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(
        requests[3]
            .get("previous_response_id")
            .is_none_or(Value::is_null)
    );
    assert!(
        requests[3]["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["role"] == "assistant")
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events.iter().filter(|event| event.success).count(), 3);
    let failed = events.iter().find(|event| !event.success).unwrap();
    let repaired = events.iter().find(|event| event.attempt == 2).unwrap();
    assert_eq!(failed.request_id, repaired.request_id);
    assert!(repaired.success);
}

#[tokio::test]
async fn websocket_native_concurrent_connection_error_settles_all_lanes_once() {
    let (upstream, probe) = fixture().await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    for (index, lane) in ["a", "b"].iter().enumerate() {
        socket.send(create(index, Some(lane))).await.unwrap();
        created(&mut socket, Some(lane), index).await;
    }
    probe.emit(json!({"type":"responsesapi.websocket_timing", "usage":{"input_tokens":999}}));
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "responsesapi.websocket_timing"
    );
    probe.emit(json!({"type":"error","status":500,"error":{"code":"server_error","message":"Synthetic connection failure"}}));
    assert_eq!(receive_websocket_json(&mut socket).await["type"], "error");
    probe.wait_settled(&gateway).await;
    assert_eq!(probe.sockets.lock().unwrap().len(), 1);
    assert_eq!(probe.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| !event.success && event.attempt == 1)
    );
    assert!(events.iter().all(|event| event.input_tokens.is_none()));
}

#[tokio::test]
async fn websocket_native_concurrent_ambiguous_output_fails_without_guessing_a_lane() {
    let (upstream, probe) = fixture().await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    for (index, lane) in ["a", "b"].iter().enumerate() {
        socket.send(create(index, Some(lane))).await.unwrap();
        created(&mut socket, Some(lane), index).await;
    }
    probe.emit(json!({"type":"response.output_text.delta","delta":"unowned output"}));
    let error = receive_websocket_json(&mut socket).await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["stream_id"], "b");
    probe.wait_settled(&gateway).await;
    assert_eq!(probe.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| !event.success && event.input_tokens.is_none())
    );
}

#[tokio::test]
async fn websocket_native_concurrent_late_parent_cannot_claim_an_unidentified_sibling() {
    let (upstream, probe) = fixture().await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, None)).await.unwrap();
    created(&mut socket, None, 0).await;
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type":"response.create", "stream_id":"b", "model":MODEL,
                "input":"synthetic delayed creation"
            })
            .to_string(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while probe.requests.lock().unwrap().len() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    probe.complete(None, "native-0", 2);
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.completed"
    );
    probe.complete(None, "native-0", 999);
    probe.emit(
        json!({"type":"response.failed", "stream_id":"b", "response":{
            "id":"native-0", "status":"failed", "output":[],
            "error":{"code":"invalid_request_error","message":"Synthetic stale parent"},
            "usage":{"input_tokens":999,"output_tokens":0}
        }}),
    );
    probe.emit(
        json!({"type":"response.created", "stream_id":"b", "response":{
            "id":"native-1", "status":"in_progress", "output":[]
        }}),
    );
    created(&mut socket, Some("b"), 1).await;
    probe.complete(Some("b"), "native-1", 4);
    assert_eq!(
        receive_websocket_json(&mut socket).await["response"]["id"],
        "native-1"
    );
    probe.wait_settled(&gateway).await;
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].input_tokens, Some(2));
    assert_eq!(events[1].input_tokens, Some(4));
    assert!(events.iter().all(|event| event.success));
}
