use super::*;
use crate::usage::{CacheKeyPresence, CachePolicyMode, CachePolicyTtl, CachePrewarm};

#[tokio::test]
async fn siwc_preserves_explicit_cache_boundaries_and_configuration_updates() {
    for debug in [false, true] {
        for stream in [false, true] {
            let siwc = AccessServer::start(json!([]), false).await;
            let native = AccessServer::start(json!([]), false).await;
            siwc.state.stream_generations.store(true, Ordering::SeqCst);
            let mut runtime = runtime(&siwc, &native).await;
            let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
            let captured = events.clone();
            let mutable = Arc::get_mut(&mut runtime).unwrap();
            mutable.diagnostics_enabled = if debug { || true } else { || false };
            mutable.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
            let (base, task) = recovery::gateway(runtime).await;
            let client = retry_admission::bounded_client();
            for marked in [false, true] {
                let mut input = json!([
                    {"role":"developer","content":[{"type":"input_text","text":"Synthetic reusable context"}]},
                    {"type":"function_call","call_id":"synthetic-call","name":"lookup","arguments":"{}"},
                    {"type":"function_call_output","call_id":"synthetic-call","output":[
                        {"type":"input_text","text":"Synthetic tool result"}]},
                    {"type":"configuration_update","reasoning":{"effort":"high"}},
                    {"role":"user","content":"Synthetic follow-up"}
                ]);
                if marked {
                    input[0]["content"][0]["prompt_cache_breakpoint"] =
                        json!({"mode":"explicit","ttl":"12h"});
                    input[2]["output"][0]["prompt_cache_breakpoint"] = json!({"mode":"explicit"});
                }
                let options = json!({"mode":"explicit","ttl":"30m","prewarm":false,"comparison_response_id":null});
                let response = client.post(format!("{base}/v1/responses"))
                    .bearer_auth("local-secret")
                    .header("x-codex-session-id", "synthetic-policy-session")
                    .json(&json!({"model":"gpt-test","input":input,"stream":stream,
                        "reasoning":{"effort":"medium"},"prompt_cache_key":"synthetic-policy-key",
                        "prompt_cache_options":options,"tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]}))
                    .send().await.unwrap();
                let status = response.status();
                let text = response.text().await.unwrap();
                assert_eq!(status, StatusCode::OK, "{text}");
                let bodies = crate::poison::mutex(&siwc.state.generation_bodies);
                let sent = bodies.last().unwrap();
                assert_eq!(sent["prompt_cache_options"], options);
                assert_eq!(sent["prompt_cache_key"], "synthetic-policy-key");
                assert_eq!(sent["reasoning"], json!({"effort":"medium"}));
                assert_eq!(sent["input"][0]["type"], "additional_tools");
                assert_eq!(
                    &sent["input"].as_array().unwrap()[1..],
                    input.as_array().unwrap()
                );
            }
            assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 2);
            assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
            let events = crate::poison::mutex(&events);
            let attempts: Vec<_> = events
                .iter()
                .filter(|event| !event.is_request_completion())
                .collect();
            assert_eq!(attempts.len(), 2);
            for (turn, event) in attempts.into_iter().enumerate() {
                assert!(event.success);
                let diagnostics = event.routing.as_ref().unwrap().cache_context.as_ref();
                assert_eq!(diagnostics.is_some(), debug);
                if let Some(diagnostics) = diagnostics {
                    for policy in [&diagnostics.client_policy, &diagnostics.upstream_policy] {
                        let policy = policy.as_ref().unwrap();
                        assert_eq!(policy.mode, CachePolicyMode::Explicit);
                        assert_eq!(policy.ttl, CachePolicyTtl::ThirtyMinutes);
                        assert_eq!(policy.prewarm, CachePrewarm::Disabled);
                        assert_eq!(policy.key, CacheKeyPresence::Present);
                        assert_eq!(policy.breakpoints.explicit, (turn * 2) as u32);
                        assert_eq!(policy.breakpoints.ttl_overrides, turn as u32);
                        assert!(policy.breakpoints.complete);
                    }
                    let encoded = serde_json::to_string(diagnostics).unwrap();
                    assert!(!encoded.contains("synthetic-policy-key"));
                    assert!(!encoded.contains("Synthetic reusable context"));
                    assert!(!encoded.contains("Synthetic tool result"));
                }
            }
            task.abort();
        }
    }
}
