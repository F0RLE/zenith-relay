use super::*;
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message, Upgrade};

#[tokio::test]
async fn siwc_http_continuation_materializes_the_owned_tool_turn_before_dispatch() {
    for stream in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        let call = json!({"type":"function_call","id":"fc_synthetic_siwc","name":"lookup",
            "call_id":"call_synthetic_siwc","arguments":"{}","status":"completed"});
        *crate::poison::mutex(&siwc.state.generation_body) = json!({
            "id":"resp_synthetic_siwc_first","status":"completed","model":"gpt-test",
            "output":[call]
        });
        let runtime = runtime(&siwc, &native).await;
        let (base, task) = recovery::gateway(runtime).await;
        let client = retry_admission::bounded_client();
        let tools = json!([{"type":"function","name":"lookup",
            "parameters":{"type":"object","properties":{},"additionalProperties":false}}]);
        let first_input = json!([{"role":"user","content":"Synthetic first turn"}]);
        let first = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","stream":stream,"input":first_input,"tools":tools}))
            .send()
            .await
            .unwrap();
        let first_status = first.status();
        let first_text = first.text().await.unwrap();
        assert_eq!(first_status, StatusCode::OK, "{first_text}");
        assert!(
            first_text.contains("resp_synthetic_siwc_first"),
            "{first_text}"
        );

        *crate::poison::mutex(&siwc.state.generation_body) = json!({
            "id":"resp_synthetic_siwc_next","status":"completed","model":"gpt-test",
            "output":[{"type":"message","role":"assistant","content":[{
                "type":"output_text","text":"Synthetic continued answer"
            }]}]
        });
        let result = json!({"type":"function_call_output","call_id":"call_synthetic_siwc",
            "output":"Synthetic lookup result"});
        let next = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","stream":stream,"tools":tools,
                "previous_response_id":"resp_synthetic_siwc_first","input":[result]}))
            .send()
            .await
            .unwrap();
        let status = next.status();
        let text = next.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "stream={stream}: {text}");
        assert!(text.contains("Synthetic continued answer"), "{text}");
        let bodies = crate::poison::mutex(&siwc.state.generation_bodies);
        assert_eq!(bodies.len(), 2);
        assert!(bodies[1].get("previous_response_id").is_none());
        assert_eq!(bodies[1]["input"][1], first_input[0]);
        assert_eq!(bodies[1]["input"][2], call);
        assert_eq!(bodies[1]["input"][3], result);
        assert_eq!(bodies[1]["store"], false);
        assert_eq!(bodies[1]["stream"], true);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        task.abort();
    }
}

#[tokio::test]
async fn siwc_websocket_continuation_replays_history_through_the_http_bridge() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    siwc.state.stream_generations.store(true, Ordering::SeqCst);
    let runtime = runtime(&siwc, &native).await;
    let (base, task) = recovery::gateway(runtime).await;
    let mut socket = retry_admission::bounded_client()
        .get(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .upgrade()
        .send()
        .await
        .unwrap()
        .into_websocket()
        .await
        .unwrap();
    let first_input = json!([{"role":"user","content":"Synthetic socket first turn"}]);
    let call = json!({"type":"custom_tool_call","id":"ctc_synthetic_edit","name":"edit","call_id":"call_synthetic_edit",
        "input":"Synthetic patch","status":"completed"});
    let result = json!({"type":"custom_tool_call_output","call_id":"call_synthetic_edit",
        "output":"Synthetic result"});
    for turn in 0..2 {
        *crate::poison::mutex(&siwc.state.generation_body) = if turn == 0 {
            json!({"id":"resp_synthetic_siwc_ws_first","status":"completed","model":"gpt-test",
                "output":[call]})
        } else {
            json!({"id":"resp_synthetic_siwc_ws_next","status":"completed","model":"gpt-test",
            "output":[{"type":"message","role":"assistant","content":[{
                "type":"output_text","text":"Synthetic socket next answer"
            }]}]})
        };
        let mut request = json!({"type":"response.create","model":"gpt-test","input":first_input,
            "tools":[{"type":"custom","name":"edit","format":{"type":"text"}}]});
        if turn == 1 {
            request["previous_response_id"] = json!("resp_synthetic_siwc_ws_first");
            request["input"] = json!([result]);
        }
        socket
            .send(Message::Text(request.to_string()))
            .await
            .unwrap();
        let terminal = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(message) = socket.next().await {
                if let Message::Text(text) = message.unwrap() {
                    let event: Value = serde_json::from_str(&text).unwrap();
                    if event["type"] == "response.completed" || event["type"] == "error" {
                        return event;
                    }
                }
            }
            panic!("SIWC socket closed without a terminal response");
        })
        .await
        .unwrap();
        assert_eq!(
            terminal["type"],
            "response.completed",
            "turn={turn}: {terminal}; SIWC requests={}; native requests={}",
            siwc.state.generation_requests.load(Ordering::SeqCst),
            native.state.generation_requests.load(Ordering::SeqCst)
        );
    }
    let bodies = crate::poison::mutex(&siwc.state.generation_bodies);
    assert_eq!(bodies.len(), 2);
    assert!(bodies[1].get("previous_response_id").is_none());
    assert_eq!(bodies[1]["input"][1], first_input[0]);
    assert_eq!(bodies[1]["input"][2], call);
    assert_eq!(bodies[1]["input"][3], result);
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    task.abort();
}

#[tokio::test]
async fn siwc_continuation_cannot_bypass_missing_foreign_or_wrong_model_replay() {
    for (stored_key, stored_model, expected) in [
        (None, "gpt-test", "adapter_continuation_missing"),
        (
            Some("other-key"),
            "gpt-test",
            "adapter_continuation_missing",
        ),
        (
            Some("key-1"),
            "other-model",
            "adapter_continuation_mismatch",
        ),
    ] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        let runtime = runtime(&siwc, &native).await;
        let now = crate::unix_time_ms();
        let route = runtime
            .executor_route(
                "synthetic-account-0",
                "gpt-test",
                &Default::default(),
                &[crate::WireApi::Responses],
                false,
            )
            .unwrap();
        runtime.bind_response_route_affinity(Some("resp_synthetic_saved"), &route, now);
        if let Some(key) = stored_key {
            runtime.capture_native_responses_replay(
                key,
                "synthetic-account-0",
                &json!({"input":[{"role":"user","content":"Synthetic original context"}]}),
                stored_model,
                &json!({"id":"resp_synthetic_saved","status":"completed",
                    "output":[]}),
                now,
            );
        }
        let (base, task) = recovery::gateway(runtime).await;
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","input":"Synthetic next turn",
                "previous_response_id":"resp_synthetic_saved"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.json::<Value>().await.unwrap();
        assert_eq!(body["error"]["code"], expected, "{body}");
        assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 0);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        task.abort();
    }
}
