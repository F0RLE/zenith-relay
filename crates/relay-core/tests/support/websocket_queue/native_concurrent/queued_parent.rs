use super::*;

async fn wait_active(gateway: &TestServer, count: u32) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight != count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn websocket_native_default_retry_exhaustion_preserves_the_queued_next_request() {
    let (upstream, probe) = fixture_with_startup(false).await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, None)).await.unwrap();
    probe.wait_requests(1).await;
    probe.complete(None, "native-0", 2);
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.completed"
    );
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type":"response.create", "model":MODEL, "store":false,
                "previous_response_id":"native-0", "input":"synthetic default continuation",
            })
            .to_string(),
        ))
        .await
        .unwrap();
    probe.wait_requests(2).await;
    socket.send(create(2, None)).await.unwrap();
    reject_marker(&mut socket).await;
    let rejection = |input_tokens| {
        json!({"type":"error", "status":400,
            "error":{"type":"invalid_request_error", "code":"previous_response_not_found", "param":"previous_response_id", "message":"Synthetic exhausted default parent repair"},
            "usage":{"input_tokens":input_tokens,"output_tokens":0,"total_tokens":input_tokens}
        })
    };
    probe.emit(rejection(3));
    probe.wait_requests(3).await;
    assert!(
        probe.requests.lock().unwrap()[2]
            .get("previous_response_id")
            .is_none_or(Value::is_null)
    );
    probe.emit(rejection(4));
    let error = receive_websocket_json(&mut socket).await;
    assert_eq!(error["type"], "error");
    assert!(error.get("stream_id").is_none());
    probe.wait_requests(4).await;
    assert_eq!(probe.requests.lock().unwrap()[3]["input"], "synthetic-2");
    probe.complete(None, "native-3", 5);
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.completed"
    );
    probe.wait_settled(&gateway).await;
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 4);
    let failed = events
        .iter()
        .filter(|event| !event.success)
        .collect::<Vec<_>>();
    assert_eq!(failed.len(), 2);
    assert_eq!(failed[0].request_id, failed[1].request_id);
    assert_eq!(failed[0].attempt, 1);
    assert_eq!(failed[1].attempt, 2);
    assert_eq!(failed[0].input_tokens, Some(3));
    assert_eq!(failed[1].input_tokens, Some(4));
    assert_eq!(events.iter().filter(|event| event.success).count(), 2);
}

#[tokio::test]
async fn websocket_native_queued_fork_replays_its_evicted_parent_without_adopting_the_new_source() {
    for opaque in [false, true] {
        let (upstream, probe) = fixture_with_startup(false).await;
        let (gateway, events) = gateway_for(&upstream).await;
        let mut socket = connect(&gateway).await;
        socket
            .send(ClientWsMessage::Text(
                json!({
                    "type":"response.create", "model":MODEL, "stream_id":"source", "store":false,
                    "input":"synthetic parent input",
                })
                .to_string(),
            ))
            .await
            .unwrap();
        probe.wait_requests(1).await;
        let mut output = vec![json!({"type":"message", "role":"assistant", "content":[
            {"type":"output_text", "text":"synthetic original parent", "annotations":[]}
        ]})];
        if opaque {
            output.insert(0, json!({"type":"reasoning", "id":"synthetic-reasoning", "summary":[], "encrypted_content":"synthetic-opaque-parent"}));
        }
        probe.emit(
            json!({"type":"response.completed", "stream_id":"source", "response":{
                "id":"native-0", "status":"completed", "output":output,
                "usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}
            }}),
        );
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.completed"
        );
        continuation(&mut socket, "source", "synthetic newer source input").await;
        let lanes = (0..15)
            .map(|index| format!("busy-{index}"))
            .collect::<Vec<_>>();
        for (index, lane) in lanes.iter().enumerate() {
            socket.send(create(index + 2, Some(lane))).await.unwrap();
        }
        probe.wait_requests(17).await;
        wait_active(&gateway, 16).await;
        continuation(&mut socket, "fork", "synthetic queued fork").await;
        reject_marker(&mut socket).await;
        assert_eq!(probe.requests.lock().unwrap().len(), 17);
        probe.emit(json!({"type":"response.completed", "stream_id":"source", "response":{
            "id":"native-1", "status":"completed", "output":[
                {"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"synthetic newer source output", "annotations":[]}]}
            ], "usage":{"input_tokens":3,"output_tokens":1,"total_tokens":4}
        }}));
        assert_eq!(
            receive_websocket_json(&mut socket).await["stream_id"],
            "source"
        );
        probe.wait_requests(18).await;
        assert_eq!(
            probe.requests.lock().unwrap()[17]["previous_response_id"],
            "native-0"
        );
        probe.emit(json!({"type":"error", "stream_id":"fork", "status":400,
            "error":{"type":"invalid_request_error", "code":"previous_response_not_found", "param":"previous_response_id", "message":"Synthetic evicted queued parent"},
            "usage":{"input_tokens":7,"output_tokens":0,"total_tokens":7}
        }));
        wait_active(&gateway, 15).await;
        assert_eq!(probe.sockets.lock().unwrap().len(), 1);
        assert_eq!(probe.requests.lock().unwrap().len(), 18);
        for (index, lane) in lanes.iter().enumerate() {
            probe.complete(Some(lane), &format!("native-{}", index + 2), 1);
            assert_eq!(
                receive_websocket_json(&mut socket).await["stream_id"],
                *lane
            );
        }
        probe.wait_requests(19).await;
        assert_eq!(probe.sockets.lock().unwrap().len(), 2);
        {
            let requests = probe.requests.lock().unwrap();
            let retry = &requests[18];
            assert_eq!(retry["stream_id"], "fork");
            assert!(retry.get("previous_response_id").is_none_or(Value::is_null));
            let input = serde_json::to_string(&retry["input"]).unwrap();
            assert!(input.contains("synthetic parent input"));
            assert!(input.contains("synthetic original parent"));
            assert!(input.contains("synthetic queued fork"));
            assert!(!input.contains("synthetic newer source"));
            assert_eq!(input.contains("synthetic-opaque-parent"), opaque);
        }
        probe.complete(Some("fork"), "native-18", 9);
        let complete = receive_websocket_json(&mut socket).await;
        assert_eq!(complete["type"], "response.completed");
        assert_eq!(complete["stream_id"], "fork");
        probe.wait_settled(&gateway).await;
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 19);
        let failed = events.iter().find(|event| !event.success).unwrap();
        let repaired = events.iter().find(|event| event.attempt == 2).unwrap();
        assert_eq!(failed.attempt, 1);
        assert_eq!(failed.input_tokens, Some(7));
        assert_eq!(failed.request_id, repaired.request_id);
        assert!(repaired.success);
        assert_eq!(repaired.input_tokens, Some(9));
        assert_eq!(events.iter().filter(|event| event.success).count(), 18);
    }
}
