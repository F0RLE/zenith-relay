use super::*;
use crate::usage::{
    CacheContextBaseline, CacheContextSection, CacheHistoryComparison, CacheInputKind,
};

fn completed_response(text: &str, stream: bool) -> Value {
    if !stream {
        return serde_json::from_str(text).unwrap();
    }
    let mut pending = text.as_bytes().to_vec();
    while let Some(frame) = crate::protocol::take_sse_event(&mut pending) {
        if let Ok(event) = serde_json::from_slice::<Value>(&crate::protocol::sse_data(&frame)) {
            if event["type"] == "response.completed" {
                return event["response"].clone();
            }
        }
    }
    panic!("synthetic BPS stream must complete: {text}");
}

#[tokio::test]
async fn bps_cache_context_observes_actual_tool_history_without_changing_requests() {
    let mut wire_baselines = std::collections::HashMap::new();
    for debug in [false, true] {
        for stream in [false, true] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            server
                .state
                .stream_generations
                .store(true, Ordering::SeqCst);
            server
                .state
                .strict_request_schema
                .store(true, Ordering::SeqCst);
            let mut runtime = retry_admission::pool_runtime(&server, 1, false).await;
            let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
            let captured = events.clone();
            runtime.diagnostics_enabled = if debug { || true } else { || false };
            runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
            let (base, task) = recovery::gateway(Arc::new(runtime)).await;
            let client = retry_admission::bounded_client();
            let tools = json!([{"type":"function","name":"lookup","parameters":{"type":"object"}}]);
            let mut input = vec![json!({"role":"user","content":"Synthetic question"})];
            for turn in 0..3 {
                let output = if turn == 0 {
                    json!([{"id":"fc_synthetic_lookup","status":"completed",
                        "type":"function_call","name":"run_officejs","call_id":"synthetic-lookup",
                        "arguments":json!({"references":["lookup"],"code":"{}"}).to_string()}])
                } else {
                    json!([{"id":format!("msg_synthetic_reply_{turn}"),"status":"completed",
                        "type":"message","role":"assistant","content":[{
                        "type":"output_text","text":"Synthetic answer"}]}])
                };
                *crate::poison::mutex(&server.state.generation_body) = json!({
                    "id":format!("synthetic-response-{turn}"),"status":"completed","model":"gpt-test",
                    "output":output,"usage":{"input_tokens":12_000,
                        "input_tokens_details":{"cached_tokens":10_000},"output_tokens":30,"total_tokens":12_030}
                });
                let instructions = if turn == 2 {
                    "Synthetic changed instructions"
                } else {
                    "Synthetic stable instructions"
                };
                let response = client
                    .post(format!("{base}/v1/responses"))
                    .bearer_auth("local-secret")
                    .header("x-codex-session-id", "synthetic-cache-session")
                    .json(
                        &json!({"model":"gpt-test","input":input,"instructions":instructions,
                        "tools":tools,"stream":stream,"prompt_cache_key":"synthetic-client-key"}),
                    )
                    .send()
                    .await
                    .unwrap();
                let status = response.status();
                let text = response.text().await.unwrap();
                assert_eq!(status, StatusCode::OK, "{text}");
                let response = completed_response(&text, stream);
                let output = response["output"].as_array().unwrap();
                input.extend(output.iter().cloned());
                if turn == 0 {
                    let call = output
                        .iter()
                        .find(|item| item["type"] == "function_call")
                        .unwrap();
                    assert_eq!(call["name"], "lookup");
                    input.push(
                        json!({"type":"function_call_output","call_id":call["call_id"],
                        "output":"Synthetic tool result"}),
                    );
                } else {
                    input.push(json!({"role":"user","content":"Synthetic follow-up"}));
                }
            }
            let sent = crate::poison::mutex(&server.state.generation_bodies).clone();
            assert_eq!(sent.len(), 3);
            for body in &sent {
                assert_eq!(body["prompt_cache_key"], "synthetic-client-key");
                assert!(body.get("prompt_cache_options").is_none());
                assert!(body.get("prompt_cache_retention").is_none());
            }
            if let Some(baseline) = wire_baselines.get(&stream) {
                assert_eq!(&sent, baseline, "debug must not change BPS wire requests");
            } else {
                wire_baselines.insert(stream, sent);
            }
            let recorded = crate::poison::mutex(&events);
            assert_eq!(recorded.len(), 3);
            for (turn, event) in recorded.iter().enumerate() {
                assert!(event.success);
                assert_eq!(event.cached_input_tokens, Some(10_000));
                let context = event.routing.as_ref().unwrap().cache_context.as_ref();
                assert_eq!(context.is_some(), debug);
                if let Some(context) = context {
                    assert_eq!(
                        context.baseline,
                        if turn == 0 {
                            CacheContextBaseline::FirstObservation
                        } else {
                            CacheContextBaseline::CompletedRequest
                        }
                    );
                    assert!(context.provider.is_none());
                    if turn > 0 {
                        assert_eq!(context.candidate_changed, Some(false));
                        assert!(context.previous_completed_age_ms.is_some());
                    }
                    if turn == 1 {
                        assert!(context.client_changes.is_empty());
                        assert!(context.upstream_changes.is_empty());
                        assert_eq!(
                            context.upstream_history.comparison,
                            CacheHistoryComparison::Appended
                        );
                    }
                    if turn == 2 {
                        assert_eq!(
                            context.client_changes,
                            vec![CacheContextSection::Instructions]
                        );
                        assert_eq!(
                            context.upstream_history.comparison,
                            CacheHistoryComparison::Rewritten
                        );
                        assert_eq!(
                            context.upstream_history.first_changed_item_kind,
                            Some(CacheInputKind::Developer)
                        );
                    }
                    let encoded = serde_json::to_string(context).unwrap();
                    for secret in ["Synthetic", "synthetic-", "lookup", "local-secret"] {
                        assert!(!encoded.contains(secret));
                    }
                }
            }
            assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 3);
            task.abort();
        }
    }
}
