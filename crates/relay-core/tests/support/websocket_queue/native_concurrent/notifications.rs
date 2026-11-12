use super::*;

async fn continue_on(socket: &mut reqwest_websocket::WebSocket, lane: Option<&str>) {
    let mut request = json!({
        "type":"response.create", "model":MODEL, "store":false,
        "previous_response_id":"native-0", "input":"synthetic continuation",
    });
    if let Some(lane) = lane {
        request["stream_id"] = json!(lane);
    }
    socket
        .send(ClientWsMessage::Text(request.to_string()))
        .await
        .unwrap();
}

#[tokio::test]
async fn websocket_native_timing_does_not_adopt_buffered_setup_usage() {
    for lane in [None, Some("main")] {
        let (upstream, probe) = fixture_with_startup(false).await;
        let (gateway, events) = gateway_for(&upstream).await;
        let mut socket = connect(&gateway).await;
        socket.send(create(0, lane)).await.unwrap();
        probe.wait_requests(1).await;
        let mut created = json!({"type":"response.created", "response":{
            "id":"native-0", "status":"in_progress", "output":[],
            "usage":{"input_tokens":7,"output_tokens":0}
        }});
        if let Some(lane) = lane {
            created["stream_id"] = json!(lane);
        }
        probe.emit(created);
        let notification = json!({"type":"responsesapi.websocket_timing", "response_id":"native-0",
            "usage":{"input_tokens":999,"output_tokens":0,"total_tokens":999}
        });
        probe.emit(notification.clone());
        assert_eq!(receive_websocket_json(&mut socket).await, notification);
        socket
            .send(ClientWsMessage::Close {
                code: reqwest_websocket::CloseCode::Normal,
                reason: String::new(),
            })
            .await
            .unwrap();
        probe.wait_settled(&gateway).await;
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, Some(7));
        assert_eq!(events[0].output_tokens, Some(0));
        assert!(events[0].ttft_ms.is_none());
        assert!(!events[0].success);
        assert_eq!(events[0].attempt, 1);
    }
}

#[tokio::test]
async fn websocket_native_timing_identity_cannot_replace_concurrent_response_owners() {
    let (upstream, probe) = fixture().await;
    let (gateway, events) = gateway_for(&upstream).await;
    let mut socket = connect(&gateway).await;
    socket.send(create(0, None)).await.unwrap();
    created(&mut socket, None, 0).await;
    socket.send(create(1, Some("other"))).await.unwrap();
    created(&mut socket, Some("other"), 1).await;
    for identity in [
        json!({"response_id":"native-0"}),
        json!({"response":{"id":"native-1"}}),
        json!({"response_id":"native-0","stream_id":"other"}),
        json!({"response_id":"unmatched"}),
        json!({}),
    ] {
        let mut notification = identity;
        notification["type"] = json!("responsesapi.websocket_timing");
        notification["usage"] = json!({"input_tokens":999,"output_tokens":7,"total_tokens":1006});
        probe.emit(notification.clone());
        assert_eq!(receive_websocket_json(&mut socket).await, notification);
    }
    probe.complete(Some("other"), "native-1", 9);
    assert_eq!(
        receive_websocket_json(&mut socket).await["stream_id"],
        "other"
    );
    probe.complete(None, "native-0", 7);
    assert_eq!(
        receive_websocket_json(&mut socket).await["response"]["id"],
        "native-0"
    );
    probe.wait_settled(&gateway).await;
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event.success && event.attempt == 1)
    );
    assert!(events.iter().all(|event| event.output_tokens == Some(1)));
    assert_eq!(events[0].input_tokens, Some(9));
    assert!(events[0].ttft_ms.is_none());
    assert_eq!(events[1].input_tokens, Some(7));
}

#[tokio::test]
async fn websocket_native_timing_does_not_commit_generation_or_disable_parent_repair() {
    for lane in [None, Some("main")] {
        let (upstream, probe) = fixture_with_startup(false).await;
        let (gateway, events) = gateway_for(&upstream).await;
        let mut socket = connect(&gateway).await;
        socket.send(create(0, lane)).await.unwrap();
        probe.wait_requests(1).await;
        probe.complete(lane, "native-0", 2);
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.completed"
        );
        continue_on(&mut socket, lane).await;
        probe.wait_requests(2).await;
        let notification = json!({"type":"responsesapi.websocket_timing", "response_id":"native-1",
            "usage":{"input_tokens":999,"output_tokens":7,"total_tokens":1006}
        });
        probe.emit(notification.clone());
        assert_eq!(receive_websocket_json(&mut socket).await, notification);
        let mut rejection = json!({"type":"error", "status":400,
            "error":{"type":"invalid_request_error", "code":"previous_response_not_found", "param":"previous_response_id", "message":"Synthetic lost parent after timing"},
            "usage":{"input_tokens":3,"output_tokens":0,"total_tokens":3}
        });
        if let Some(lane) = lane {
            rejection["stream_id"] = json!(lane);
        }
        probe.emit(rejection);
        probe.wait_requests(3).await;
        assert!(
            probe.requests.lock().unwrap()[2]
                .get("previous_response_id")
                .is_none_or(Value::is_null)
        );
        probe.complete(lane, "native-2", 9);
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.completed"
        );
        probe.wait_settled(&gateway).await;
        assert_eq!(probe.sockets.lock().unwrap().len(), 2);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events.iter().filter(|event| event.success).count(), 2);
        let failed = events.iter().find(|event| !event.success).unwrap();
        let repaired = events.iter().find(|event| event.attempt == 2).unwrap();
        assert_eq!(failed.request_id, repaired.request_id);
        assert_eq!(failed.input_tokens, Some(3));
        assert_eq!(repaired.input_tokens, Some(9));
        assert!(failed.ttft_ms.is_none());
    }
}
