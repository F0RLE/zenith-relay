use super::*;
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message, Upgrade};

fn warmup_response() -> Value {
    json!({"id":"resp_synthetic_siwc_warmup","model":"gpt-test","status":"completed",
        "output":[],"usage":{"input_tokens":24,"output_tokens":0,"total_tokens":24,
            "input_tokens_details":{"cached_tokens":0,"cache_write_tokens":24}}})
}

#[tokio::test]
async fn siwc_http_prewarm_accepts_an_empty_completed_response_without_losing_usage() {
    for stream in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        *crate::poison::mutex(&siwc.state.generation_body) = warmup_response();
        let (base, task) = recovery::gateway(runtime(&siwc, &native).await).await;
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(
                &json!({"model":"gpt-test","input":"Synthetic warmup context",
                "stream":stream,"prompt_cache_options":{"prewarm":true,"ttl":"30m"}}),
            )
            .send()
            .await
            .unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "stream={stream}: {text}");
        assert!(text.contains("resp_synthetic_siwc_warmup"), "{text}");
        assert!(text.contains("\"cache_write_tokens\":24"), "{text}");
        let sent = crate::poison::mutex(&siwc.state.generation_bodies);
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0]["prompt_cache_options"],
            json!({"prewarm":true,"ttl":"30m"})
        );
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        task.abort();
    }
}

#[tokio::test]
async fn siwc_websocket_generate_false_warms_up_and_continues_without_an_error() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    siwc.state.stream_generations.store(true, Ordering::SeqCst);
    *crate::poison::mutex(&siwc.state.generation_body) = warmup_response();
    let (base, task) = recovery::gateway(runtime(&siwc, &native).await).await;
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
    for turn in 0..2 {
        let payload = if turn == 0 {
            json!({"type":"response.create","model":"gpt-test","generate":false,
                "input":[{"role":"developer","content":"Synthetic reusable context"}]})
        } else {
            *crate::poison::mutex(&siwc.state.generation_body) = json!({
                "id":"resp_synthetic_siwc_answer","model":"gpt-test","status":"completed",
                "output":[{"type":"message","role":"assistant","content":[{
                    "type":"output_text","text":"Synthetic generated answer"
                }]}],"usage":{"input_tokens":28,"output_tokens":4,"total_tokens":32,
                    "input_tokens_details":{"cached_tokens":24,"cache_write_tokens":4}}});
            json!({"type":"response.create","model":"gpt-test","generate":true,
                "previous_response_id":"resp_synthetic_siwc_warmup",
                "input":[{"role":"user","content":"Synthetic question"}]})
        };
        socket
            .send(Message::Text(payload.to_string()))
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
            panic!("SIWC warmup socket closed without a terminal response");
        })
        .await
        .unwrap();
        assert_eq!(
            terminal["type"], "response.completed",
            "turn={turn}: {terminal}"
        );
        if turn == 0 {
            assert_eq!(terminal["response"]["output"], json!([]));
            assert_eq!(terminal["response"]["usage"]["output_tokens"], 0);
        } else {
            assert_eq!(
                terminal["response"]["output"][0]["content"][0]["text"],
                "Synthetic generated answer"
            );
        }
    }
    let sent = crate::poison::mutex(&siwc.state.generation_bodies);
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["prompt_cache_options"]["prewarm"], true);
    assert!(sent.iter().all(|body| body.get("generate").is_none()));
    assert_ne!(
        sent[1].pointer("/prompt_cache_options/prewarm"),
        Some(&Value::Bool(true))
    );
    assert!(sent[1].get("previous_response_id").is_none());
    assert_eq!(sent[1]["input"][0], sent[0]["input"][0]);
    assert_eq!(sent[1]["input"][1]["content"], "Synthetic question");
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    task.abort();
}
