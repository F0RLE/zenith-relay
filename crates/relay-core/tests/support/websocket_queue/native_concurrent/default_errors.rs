use super::*;

#[tokio::test]
async fn websocket_native_default_empty_incomplete_preserves_queued_work() {
    for reason in [None, Some("max_output_tokens")] {
        let (upstream, probe) = fixture_with_startup(false).await;
        let (gateway, events) = gateway_for(&upstream).await;
        let mut socket = connect(&gateway).await;
        socket.send(create(0, None)).await.unwrap();
        probe.wait_requests(1).await;
        socket.send(create(1, None)).await.unwrap();
        reject_marker(&mut socket).await;
        probe.emit(json!({"type":"response.incomplete", "response":{
            "id":"native-0", "status":"incomplete", "output":[],
            "incomplete_details":reason.map(|reason| json!({"reason":reason})),
            "usage":{"input_tokens":3,"output_tokens":0,"total_tokens":3}
        }}));
        let terminal = receive_websocket_json(&mut socket).await;
        assert_eq!(
            terminal["type"],
            if reason.is_some() {
                "response.incomplete"
            } else {
                "error"
            }
        );
        assert!(terminal.get("stream_id").is_none());
        probe.wait_requests(2).await;
        assert_eq!(probe.requests.lock().unwrap()[1]["input"], "synthetic-1");
        probe.complete(None, "native-1", 5);
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.completed"
        );
        probe.wait_settled(&gateway).await;
        assert_eq!(probe.requests.lock().unwrap().len(), 2);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| event.attempt == 1));
        assert_eq!(events.iter().filter(|event| event.success).count(), 1);
        let failed = events.iter().find(|event| !event.success).unwrap();
        assert_eq!(failed.input_tokens, Some(3));
    }
}

#[tokio::test]
async fn websocket_native_default_error_after_output_preserves_siblings_and_queued_work() {
    for sibling in [false, true] {
        for nested in [false, true] {
            let (upstream, probe) = fixture_with_startup(false).await;
            let (gateway, events) = gateway_for(&upstream).await;
            let mut socket = connect(&gateway).await;
            socket.send(create(0, None)).await.unwrap();
            probe.wait_requests(1).await;
            probe.emit(json!({"type":"response.output_text.delta", "response_id":"native-0", "delta":"synthetic default output"}));
            assert_eq!(
                receive_websocket_json(&mut socket).await["delta"],
                "synthetic default output"
            );
            if sibling {
                socket.send(create(1, Some("sibling"))).await.unwrap();
                probe.wait_requests(2).await;
            }
            socket.send(create(2, None)).await.unwrap();
            reject_marker(&mut socket).await;
            let mut error = json!({"type":"error", "status":400,
                "code":"invalid_request_error", "message":"Synthetic default rejection",
                "usage":{"input_tokens":3,"output_tokens":1,"total_tokens":4}
            });
            if nested {
                error["error"] = json!({"code":"invalid_request_error", "message":"Synthetic default rejection"});
                error.as_object_mut().unwrap().remove("code");
                error.as_object_mut().unwrap().remove("message");
            }
            probe.emit(error);
            let error = receive_websocket_json(&mut socket).await;
            assert_eq!(error["type"], "error");
            assert!(error.get("stream_id").is_none());
            let count = if sibling { 3 } else { 2 };
            probe.wait_requests(count).await;
            assert_eq!(
                probe.requests.lock().unwrap()[count - 1]["input"],
                "synthetic-2"
            );
            if sibling {
                probe.emit(json!({"type":"response.output_text.delta", "stream_id":"sibling", "response_id":"native-1", "delta":"synthetic sibling survives"}));
                let delta = receive_websocket_json(&mut socket).await;
                assert_eq!(delta["stream_id"], "sibling");
                assert_eq!(delta["delta"], "synthetic sibling survives");
                probe.complete(Some("sibling"), "native-1", 7);
                assert_eq!(
                    receive_websocket_json(&mut socket).await["type"],
                    "response.completed"
                );
            }
            probe.complete(None, &format!("native-{}", count - 1), 5);
            let complete = receive_websocket_json(&mut socket).await;
            assert_eq!(complete["type"], "response.completed");
            assert!(complete.get("stream_id").is_none());
            probe.wait_settled(&gateway).await;
            assert_eq!(
                probe.sockets.lock().unwrap().len(),
                if sibling { 1 } else { 2 }
            );
            assert_eq!(probe.requests.lock().unwrap().len(), count);
            let events = events.lock().unwrap();
            assert_eq!(events.len(), count);
            assert!(events.iter().all(|event| event.attempt == 1));
            assert_eq!(
                events.iter().filter(|event| event.success).count(),
                count - 1
            );
            let failed = events.iter().find(|event| !event.success).unwrap();
            assert!(failed.ttft_ms.is_some());
            assert_eq!(failed.input_tokens, Some(3));
            assert_eq!(failed.output_tokens, Some(1));
        }
    }
}

#[tokio::test]
async fn websocket_native_connection_limit_with_default_owner_settles_every_request() {
    for sibling in [false, true] {
        for nested in [false, true] {
            let (upstream, probe) = fixture().await;
            let (gateway, events) = gateway_for(&upstream).await;
            let mut socket = connect(&gateway).await;
            socket.send(create(0, None)).await.unwrap();
            created(&mut socket, None, 0).await;
            if sibling {
                socket.send(create(1, Some("sibling"))).await.unwrap();
                created(&mut socket, Some("sibling"), 1).await;
            }
            socket.send(create(2, None)).await.unwrap();
            reject_marker(&mut socket).await;
            let mut error = json!({"type":"error", "status":400,
                "code":"websocket_connection_limit_reached", "message":"Synthetic connection limit",
                "usage":{"input_tokens":999,"output_tokens":999,"total_tokens":1998}
            });
            if nested {
                error["error"] = json!({"code":"websocket_connection_limit_reached", "message":"Synthetic connection limit"});
                error.as_object_mut().unwrap().remove("code");
                error.as_object_mut().unwrap().remove("message");
            }
            probe.emit(error);
            assert_eq!(receive_websocket_json(&mut socket).await["type"], "error");
            probe.wait_settled(&gateway).await;
            let count = if sibling { 2 } else { 1 };
            assert_eq!(probe.sockets.lock().unwrap().len(), 1);
            assert_eq!(probe.requests.lock().unwrap().len(), count);
            let events = events.lock().unwrap();
            assert_eq!(events.len(), count);
            assert!(
                events
                    .iter()
                    .all(|event| !event.success && event.attempt == 1)
            );
            assert!(events.iter().all(|event| event.input_tokens.is_none()));
        }
    }
}
