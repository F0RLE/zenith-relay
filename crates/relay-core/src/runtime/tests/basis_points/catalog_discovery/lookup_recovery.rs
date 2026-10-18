use super::*;

#[tokio::test]
async fn invalid_local_lookup_can_be_corrected_without_rotating_or_replaying_client_tools() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        crate::poison::mutex(&server.state.generation_replies).extend([
            (
                StatusCode::OK,
                tool_response(
                    "relay.catalog",
                    "call_bad_lookup",
                    json!({"names":["unknown"]}),
                ),
            ),
            (
                StatusCode::OK,
                tool_response(
                    "relay.catalog",
                    "call_good_lookup",
                    json!({"names":["lookup"]}),
                ),
            ),
            (
                StatusCode::OK,
                tool_response("lookup", "call_client", json!({"path":"synthetic.txt"})),
            ),
        ]);
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let mut runtime = runtime(&server).await;
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let (base, task) = recovery::gateway(Arc::new(runtime)).await;
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&catalog_request(stream))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.bytes().await.unwrap();
        let body: Value = if stream {
            let mut pending = bytes.to_vec();
            let mut completed = None;
            while let Some(frame) = crate::protocol::take_sse_event(&mut pending) {
                let event: Value =
                    serde_json::from_slice(&crate::protocol::sse_data(&frame)).unwrap();
                assert_ne!(event["type"], "error");
                if event["type"] == "response.completed" {
                    assert!(completed.is_none());
                    completed = Some(event["response"].clone());
                }
            }
            completed.unwrap()
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        assert_eq!(body["output"].as_array().unwrap().len(), 1);
        assert_eq!(body["output"][0]["name"], "lookup");
        assert_eq!(body["output"][0]["call_id"], "call_client");
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 3);
        let events = crate::poison::mutex(&events);
        assert_eq!(events.len(), 3);
        for event in events.iter() {
            assert_eq!(event.request_id, events[0].request_id);
            assert_eq!(event.candidate_id, events[0].candidate_id);
            assert_eq!(event.input_tokens, Some(100));
            assert_eq!(event.output_tokens, Some(10));
        }
        let last = events.last().unwrap();
        assert!(last.success);
        assert_eq!(
            last.tool_use
                .basis_points_catalog
                .as_ref()
                .unwrap()
                .discovery_rounds,
            2
        );
        task.abort();
    }
}
