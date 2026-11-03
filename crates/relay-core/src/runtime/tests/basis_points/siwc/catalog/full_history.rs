use super::*;

#[tokio::test]
async fn siwc_full_history_keeps_each_sessions_prefix_without_retrying_cache_unavailability() {
    for stream in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        let mut runtime = runtime(&siwc, &native).await;
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let settings = Arc::get_mut(&mut runtime).unwrap();
        settings.diagnostics_enabled = || true;
        settings.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        {
            let mut scheduler = runtime.lock_scheduler();
            let mut excluded = scheduler.candidate("synthetic-account-1").unwrap().clone();
            excluded.model_rules.excluded.insert("gpt-test".into());
            scheduler.upsert(excluded);
        }
        let (base, task) = recovery::gateway(runtime).await;
        let client = retry_admission::bounded_client();
        let mut requests = (0..2)
            .map(|session| {
                let mut body = request(stream);
                body["prompt_cache_key"] = json!(format!("synthetic-session-{session}"));
                body["input"] = json!([
                    {"role":"developer","content":"Synthetic stable instructions. ".repeat(300)},
                    {"role":"user","content":format!("Synthetic session {session}")},
                    {"type":"function_call","call_id":"synthetic-history","name":"lookup_0","arguments":"{\"path\":\"synthetic\"}"},
                    {"type":"function_call_output","call_id":"synthetic-history","output":"Synthetic historical result"}
                ]);
                body
            })
            .collect::<Vec<_>>();
        let mut previous_inputs: [Option<Value>; 2] = [None, None];
        for turn in 0..3 {
            for (session, payload) in requests.iter_mut().enumerate() {
                let call_id = format!("synthetic-call-{session}-{turn}");
                let mut reply = response(
                    &format!("synthetic-response-{session}-{turn}"),
                    json!([
                        {"type":"reasoning","id":format!("rs_synthetic_{session}_{turn}"),"summary":[],"encrypted_content":format!("synthetic-opaque-{session}-{turn}")},
                        {"type":"function_call","id":format!("fc_synthetic_{session}_{turn}"),"call_id":call_id,"name":"lookup_0","arguments":"{\"path\":\"synthetic\"}","status":"completed"}
                    ]),
                );
                reply["usage"]["input_tokens_details"] =
                    json!({"cached_tokens":0,"cache_write_tokens":0});
                reply["prompt_cache_diagnostics"] = json!({"type":"unavailable"});
                *crate::poison::mutex(&siwc.state.generation_body) = reply;
                let result = client
                    .post(format!("{base}/v1/responses"))
                    .bearer_auth("local-secret")
                    .header("x-codex-session-id", format!("synthetic-session-{session}"))
                    .json(payload)
                    .send()
                    .await
                    .unwrap();
                let status = result.status();
                let text = result.text().await.unwrap();
                assert_eq!(status, StatusCode::OK, "{text}");
                let result = terminal(&text, stream);
                assert_eq!(result["usage"]["input_tokens_details"]["cached_tokens"], 0);
                assert_eq!(
                    result["usage"]["input_tokens_details"]["cache_write_tokens"],
                    0
                );
                let sent = crate::poison::mutex(&siwc.state.generation_bodies)
                    .last()
                    .unwrap()
                    .clone();
                assert_eq!(sent["prompt_cache_key"], payload["prompt_cache_key"]);
                assert_eq!(sent["reasoning"], payload["reasoning"]);
                assert_eq!(sent["prompt_cache_options"]["mode"], "implicit");
                assert_eq!(sent["prompt_cache_options"]["ttl"], "30m");
                if let Some(previous) = previous_inputs[session].as_ref() {
                    let previous = previous.as_array().unwrap();
                    assert_eq!(
                        &sent["input"].as_array().unwrap()[..previous.len()],
                        previous
                    );
                }
                previous_inputs[session] = Some(sent["input"].clone());
                // A real stateless client replays its own history and returned output,
                // not Relay's private tool-loading items from the upstream request.
                let input = payload["input"].as_array_mut().unwrap();
                input.extend(result["output"].as_array().unwrap().iter().cloned());
                input.push(json!({"type":"function_call_output","call_id":call_id,"output":"Synthetic next result"}));
            }
        }
        assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 6);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        let events = crate::poison::mutex(&events);
        let attempts = events
            .iter()
            .filter(|event| !event.is_request_completion())
            .collect::<Vec<_>>();
        assert_eq!(attempts.len(), 6);
        for event in attempts {
            assert!(event.success);
            assert_eq!(event.cached_input_tokens, Some(0));
            assert_eq!(event.cache_write_input_tokens, Some(0));
        }
        task.abort();
    }
}
