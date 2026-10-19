use super::*;
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message, Upgrade};

async fn bps_only_runtime(bps: &AccessServer, native: &AccessServer) -> Arc<GatewayRuntime> {
    let mut runtime = retry_admission::pool_runtime(bps, 1, true).await;
    runtime
        .chatgpt_accounts
        .get_mut("synthetic-account-1")
        .unwrap()
        .responses_url = native.responses_url.clone();
    {
        let mut scheduler = runtime.lock_scheduler();
        let mut excluded = scheduler.candidate("synthetic-account-1").unwrap().clone();
        excluded.model_rules.excluded.insert("gpt-test".into());
        excluded.priority = 1_000;
        scheduler.upsert(excluded);
    }
    let affinity = runtime
        .prompt_affinity_key("key-1", "gpt-test", Some("synthetic-cache"), None)
        .unwrap();
    runtime.bind_prompt_affinity(
        Some(&affinity),
        "synthetic-account-1",
        crate::unix_time_ms(),
    );
    Arc::new(runtime)
}

#[tokio::test]
async fn responses_routes_a_bps_only_model_without_sending_to_the_excluded_native_account() {
    for (stream, lite) in [(false, false), (true, false), (false, true), (true, true)] {
        let bps = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        let native = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        bps.state.stream_generations.store(stream, Ordering::SeqCst);
        bps.state
            .strict_request_schema
            .store(true, Ordering::SeqCst);
        *crate::poison::mutex(&bps.state.generation_body) = json!({
            "id":"synthetic-bps-response","status":"completed","output":[{
                "type":"message","role":"assistant","content":[{
                    "type":"output_text","text":"Synthetic BPS answer"
                }]
            }]
        });
        let runtime = bps_only_runtime(&bps, &native).await;
        let (base, task) = recovery::gateway(runtime.clone()).await;
        let mut request = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret");
        if lite {
            request = request.header("x-openai-internal-codex-responses-lite", "true");
        }
        let response = request
            .json(&json!({
                "model":"gpt-test","input":"Synthetic request","stream":stream,
                "prompt_cache_key":"synthetic-cache","parallel_tool_calls":true
            }))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("Synthetic BPS answer"), "{body}");
        if stream {
            assert!(body.contains("response.completed"), "{body}");
        }
        assert_eq!(bps.state.generation_requests.load(Ordering::SeqCst), 1);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        assert!(
            runtime
                .lock_scheduler()
                .candidate("synthetic-account-1")
                .unwrap()
                .enabled
        );
        task.abort();
    }
}

#[tokio::test]
async fn websocket_routes_a_bps_only_model_through_http_without_trying_native_responses() {
    let bps = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let native = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    bps.state.stream_generations.store(true, Ordering::SeqCst);
    bps.state
        .strict_request_schema
        .store(true, Ordering::SeqCst);
    let runtime = bps_only_runtime(&bps, &native).await;
    let (base, task) = recovery::gateway(runtime.clone()).await;
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
    for turn in 0..3 {
        if turn > 0 {
            if turn == 2 {
                bps.reply_with(
                    StatusCode::OK,
                    json!({"allowed":true,"model_catalog":{"models":[]}}),
                );
            }
            crate::poison::write(
                &runtime.chatgpt_accounts["synthetic-account-0"].basis_points_access,
            )
            .as_mut()
            .unwrap()
            .expire();
        }
        let payload = json!({
            "type":"response.create","model":"gpt-test","input":"Synthetic request",
            "prompt_cache_key":"synthetic-cache"
        })
        .to_string();
        let message = if turn == 1 {
            Message::Binary(payload.into_bytes().into())
        } else {
            Message::Text(payload)
        };
        socket.send(message).await.unwrap();
        let terminal = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(message) = socket.next().await {
                if let Message::Text(text) = message.unwrap() {
                    let event: Value = serde_json::from_str(&text).unwrap();
                    if event["type"] == "response.completed" || event["type"] == "error" {
                        return event;
                    }
                }
            }
            panic!("socket closed without a terminal BPS response");
        })
        .await
        .expect("BPS-only WebSocket request must finish without a native upgrade");
        if turn == 2 {
            assert_eq!(terminal["type"], "error", "{terminal}");
            assert_eq!(terminal["error"]["code"], "model_not_found");
        } else {
            assert_eq!(terminal["type"], "response.completed", "{terminal}");
            assert_eq!(terminal["response"]["status"], "completed");
        }
    }
    assert_eq!(bps.state.access_requests.load(Ordering::SeqCst), 3);
    assert_eq!(bps.state.generation_requests.load(Ordering::SeqCst), 2);
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    drop(socket);
    task.abort();
}
