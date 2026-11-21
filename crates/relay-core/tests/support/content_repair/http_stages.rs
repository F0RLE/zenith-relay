use super::*;
use zenith_relay_core::AttemptOutcome;

#[tokio::test]
async fn http_repair_then_failover_preserves_state_across_json_and_sse_stages() {
    for streaming in [false, true] {
        let first = if streaming {
            Reply::Stream(vec![StreamChunk::Data(
                "data: {\"type\":\"response.failed\",\"response\":{\"id\":\"synthetic-rejection\",\"status\":\"failed\",\"error\":{\"code\":\"invalid_request_error\",\"message\":\"Missing required field: call_id\"},\"output\":[],\"usage\":{\"input_tokens\":3,\"output_tokens\":0,\"total_tokens\":3}}}\n\n",
            )])
        } else {
            Reply::Json(
                StatusCode::BAD_REQUEST,
                json!({
                    "error":{"message":"Missing required field: call_id"},
                    "usage":{"input_tokens":3,"output_tokens":0,"total_tokens":3}
                }),
            )
        };
        let final_reply = if streaming {
            Reply::Stream(vec![StreamChunk::Data(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"synthetic-completed\",\"status\":\"completed\",\"output\":[{\"id\":\"synthetic-message\",\"type\":\"message\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"synthetic final answer\",\"annotations\":[]}]}],\"usage\":{\"input_tokens\":5,\"output_tokens\":2,\"total_tokens\":7}}}\n\n",
            )])
        } else {
            Reply::Json(
                StatusCode::OK,
                json!({
                    "id":"synthetic-completed", "status":"completed",
                    "output":[{"id":"synthetic-message","type":"message","role":"assistant",
                        "status":"completed","content":[{"type":"output_text",
                            "text":"synthetic final answer","annotations":[]}]}],
                    "usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}
                }),
            )
        };
        let (upstream, state) = spawn_upstream(vec![
            first,
            Reply::Json(
                StatusCode::TOO_MANY_REQUESTS,
                json!({
                    "error":{"code":"rate_limit_exceeded","message":"Synthetic quota limit"},
                    "usage":{"input_tokens":4,"output_tokens":0,"total_tokens":4}
                }),
            ),
            final_reply,
        ])
        .await;
        let (gateway, events, _, _) = spawn_mixed_gateway(
            vec![
                source("synthetic-source-a", &upstream, "synthetic-a", 10),
                source("synthetic-source-b", &upstream, "synthetic-b", 10),
            ],
            Vec::new(),
            vec![mixed_key(None, None)],
            Arc::new(TokenAuthority::new(4).unwrap()),
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":MODEL,"stream":streaming,"input":[
                {"type":"function_call","name":"lookup","arguments":"{}"},
                {"type":"function_call_output","output":"synthetic result"}
            ]}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "streaming={streaming}");
        let body = response.text().await.unwrap();
        assert!(body.contains("synthetic final answer"));
        assert!(!body.contains("Missing required field"));
        tokio::time::sleep(Duration::from_millis(20)).await;

        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 3, "streaming={streaming}");
        assert!(requests[0].body["input"][0].get("call_id").is_none());
        let call_id = requests[1].body["input"][0]["call_id"].as_str().unwrap();
        assert!(!call_id.is_empty());
        for request in &requests[1..] {
            assert_eq!(request.body["input"][0]["call_id"], call_id);
            assert_eq!(request.body["input"][1]["call_id"], call_id);
            assert_eq!(request.body["input"][1]["output"], "synthetic result");
        }
        drop(requests);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 3, "streaming={streaming}");
        // Identifier repair is route-neutral; quota failover leaves the refused candidate.
        assert_ne!(events[1].candidate_id, events[2].candidate_id);
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event.request_id, events[0].request_id);
            assert_eq!(event.attempt, (index + 1) as u16);
            assert_eq!(event.input_tokens, Some((index + 3) as u64));
            assert_eq!(
                event.routing.as_ref().unwrap().attempt_outcome,
                Some(
                    [
                        AttemptOutcome::RetryRepair,
                        AttemptOutcome::RetrySelection,
                        AttemptOutcome::Completed,
                    ][index]
                )
            );
        }
        assert!(!events[0].success && !events[1].success && events[2].success);
        assert!(
            gateway
                .runtime
                .as_ref()
                .unwrap()
                .candidate_runtime_order()
                .iter()
                .all(|candidate| candidate.in_flight == 0)
        );
    }
}
