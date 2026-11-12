use super::*;
use crate::usage::{CacheContextBaseline, CacheHistoryComparison};
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message, Upgrade};

#[tokio::test]
async fn siwc_defaults_missing_cache_keys_without_replacing_client_policy() {
    for stream in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        let runtime = runtime(&siwc, &native).await;
        {
            let mut scheduler = runtime.lock_scheduler();
            let mut excluded = scheduler.candidate("synthetic-account-1").unwrap().clone();
            excluded.model_rules.excluded.insert("gpt-test".into());
            scheduler.upsert(excluded);
        }
        let (base, task) = recovery::gateway(runtime).await;
        let client = retry_admission::bounded_client();
        let mut generated_key = None;
        for (turn, session) in ["synthetic-session", "synthetic-session", "other-session"]
            .into_iter()
            .enumerate()
        {
            let response = client
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .header("x-codex-session-id", session)
                .json(&json!({"model":"gpt-test","stream":stream,"input":[{
                    "role":"user","content":"Synthetic input"
                }],"prompt_cache_options":{"mode":"implicit","ttl":"30m"}}))
                .send()
                .await
                .unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{text}");
            let sent = crate::poison::mutex(&siwc.state.generation_bodies)[turn].clone();
            let key = sent["prompt_cache_key"]
                .as_str()
                .expect("SIWC must receive the same scoped key as other account transports");
            assert!(key.starts_with("relay_"));
            assert!(!key.contains(session));
            if turn == 0 {
                generated_key = Some(key.to_owned());
            } else if turn == 1 {
                assert_eq!(Some(key), generated_key.as_deref());
            } else {
                assert_ne!(Some(key), generated_key.as_deref());
            }
            assert_eq!(
                sent["prompt_cache_options"],
                json!({"mode":"implicit","ttl":"30m"})
            );
            assert_eq!(sent["store"], false);
            assert_eq!(sent["stream"], true);
        }
        task.abort();
    }
}

#[tokio::test]
async fn siwc_preserves_append_only_tool_prefix_and_reports_cache_context_without_prompts() {
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
            let lookup = json!({"type":"function","name":"lookup","parameters":{"type":"object"}});
            let edit = json!({"type":"custom","name":"edit","format":{"type":"text"}});
            let mut input = json!([
                {"role":"developer","content":"Synthetic stable instructions"},
                {"role":"user","content":"Synthetic first turn"}
            ]);
            for turn in 0..3 {
                let output = match turn {
                    0 => {
                        json!([{"type":"function_call","name":"lookup","call_id":"call_lookup","arguments":"{}"}])
                    }
                    1 => {
                        json!([{"type":"custom_tool_call","name":"edit","call_id":"call_edit","input":"Synthetic patch"}])
                    }
                    _ => {
                        json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Synthetic final reply"}]}])
                    }
                };
                *crate::poison::mutex(&siwc.state.generation_body) = json!({
                    "id":format!("synthetic-siwc-cache-{turn}"),"status":"completed","model":"gpt-test",
                    "output":output,
                    "prompt_cache_diagnostics":{"type":"cache_miss","reason":"input_changed","cache_missed_tokens":2048},
                    "usage":{"input_tokens":12_000,"input_tokens_details":{"cached_tokens":10_000,"cache_write_tokens":2_000},"output_tokens":30,"total_tokens":12_030}
                });
                let tools = if turn == 0 {
                    json!([lookup])
                } else {
                    json!([lookup, edit])
                };
                let response = client.post(format!("{base}/v1/responses"))
                    .bearer_auth("local-secret")
                    .header("x-codex-session-id", "synthetic-siwc-cache-session")
                    .json(&json!({"model":"gpt-test","stream":stream,"store":false,"input":input,"tools":tools,
                        "prompt_cache_key":"synthetic-client-key","prompt_cache_options":{"mode":"implicit","ttl":"30m"}}))
                    .send().await.unwrap();
                let status = response.status();
                let text = response.text().await.unwrap();
                assert_eq!(status, StatusCode::OK, "{text}");
                let sent = crate::poison::mutex(&siwc.state.generation_bodies)[turn].clone();
                assert_eq!(sent["store"], false);
                assert_eq!(sent["stream"], true);
                assert_eq!(sent["prompt_cache_key"], "synthetic-client-key");
                let expected_options = if debug && turn > 0 {
                    json!({"mode":"implicit","ttl":"30m","comparison_response_id":format!("synthetic-siwc-cache-{}", turn - 1)})
                } else {
                    json!({"mode":"implicit","ttl":"30m"})
                };
                assert_eq!(sent["prompt_cache_options"], expected_options);
                if turn == 0 {
                    assert_eq!(sent["input"][0]["tools"], json!([lookup]));
                } else if turn == 1 {
                    let sent_input = sent["input"].as_array().unwrap();
                    let input = input.as_array().unwrap();
                    assert_eq!(&sent_input[..input.len()], input);
                    assert_eq!(sent_input.last().unwrap()["tools"], json!([edit]));
                } else {
                    assert_eq!(sent["input"], input);
                }
                input = sent["input"].clone();
                input
                    .as_array_mut()
                    .unwrap()
                    .extend(output.as_array().unwrap().iter().cloned());
                if turn < 2 {
                    input.as_array_mut().unwrap().push(if turn == 0 {
                        json!({"type":"function_call_output","call_id":"call_lookup","output":"Synthetic lookup result"})
                    } else {
                        json!({"type":"custom_tool_call_output","call_id":"call_edit","output":"Synthetic edit result"})
                    });
                    input
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"role":"user","content":"Synthetic next turn"}));
                }
            }
            let events = crate::poison::mutex(&events);
            assert_eq!(events.len(), 3);
            assert_eq!(
                siwc.state.websocket_requests.load(Ordering::SeqCst),
                1,
                "one upstream connection must serve the identified conversation: stream={stream}, debug={debug}"
            );
            for event in events.iter() {
                assert!(event.success);
                assert_eq!(event.input_tokens, Some(12_000));
                assert_eq!(event.cached_input_tokens, Some(10_000));
                assert_eq!(event.cache_write_input_tokens, Some(2_000));
                assert_eq!(
                    event.routing.as_ref().unwrap().cache_context.is_some(),
                    debug
                );
            }
            if debug {
                for event in events.iter() {
                    let transport = event.routing.as_ref().unwrap().transport.as_ref().unwrap();
                    let payload = transport.request_payload.as_ref().unwrap();
                    assert_eq!(
                        payload.content_encoding,
                        crate::usage::RequestContentEncoding::Identity
                    );
                    assert_eq!(payload.compression_us, None);
                    assert_eq!(transport.upstream, crate::UsageTransport::Websocket);
                    assert_eq!(payload.http_send_to_headers_ms, None);
                    assert_eq!(transport.response_headers_ms, None);
                    assert_eq!(transport.upstream_http_status, None);
                }
                let diagnostics = |index: usize| {
                    events[index]
                        .routing
                        .as_ref()
                        .unwrap()
                        .cache_context
                        .as_ref()
                        .unwrap()
                };
                assert_eq!(
                    diagnostics(0).baseline,
                    CacheContextBaseline::FirstObservation
                );
                for turn in [1, 2] {
                    assert_eq!(
                        diagnostics(turn).baseline,
                        CacheContextBaseline::CompletedRequest
                    );
                    assert_eq!(
                        diagnostics(turn).upstream_history.comparison,
                        CacheHistoryComparison::Appended
                    );
                    assert_eq!(diagnostics(turn).candidate_changed, Some(false));
                }
                assert_eq!(
                    diagnostics(2).relay_history.comparison,
                    CacheHistoryComparison::Unchanged
                );
                let encoded = serde_json::to_string(diagnostics(2)).unwrap();
                assert!(encoded.contains("input_changed"));
                assert!(!encoded.contains("synthetic-siwc-cache"));
                assert!(!encoded.contains("Synthetic"));
                assert!(!encoded.contains("synthetic-client-key"));
                assert!(!encoded.contains("call_lookup"));
            }
            assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 3);
            assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
            task.abort();
        }
    }
}

#[tokio::test]
async fn siwc_completed_item_snapshots_reuse_pool_socket_with_delta_history() {
    for stream in [false, true] {
        for sparse in [false, true] {
            let siwc = AccessServer::start(json!([]), false).await;
            let native = AccessServer::start(json!([]), false).await;
            siwc.state.stream_generations.store(true, Ordering::SeqCst);
            let mut runtime = runtime(&siwc, &native).await;
            let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
            let captured = events.clone();
            let mutable = Arc::get_mut(&mut runtime).unwrap();
            mutable.diagnostics_enabled = || true;
            mutable.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
            let reasoning = json!({"type":"reasoning","id":"rs_test","summary":[],"content":[],
                "encrypted_content":"synthetic-completed-cipher","status":"completed",
                "internal_chat_message_metadata_passthrough":{"turn_id":"synthetic-provider-turn"}});
            let call = json!({"type":"function_call","id":"fc_test","call_id":"call_test",
                "name":"lookup","arguments":"{}","status":"completed","caller":null});
            let mut terminal_reasoning = reasoning.clone();
            terminal_reasoning["encrypted_content"] = json!("synthetic-terminal-cipher");
            let output = if sparse {
                json!([])
            } else {
                json!([terminal_reasoning, call])
            };
            let frames = [
                json!({"type":"response.created","response":{"id":"synthetic-first","status":"in_progress","model":"gpt-test"}}),
                json!({"type":"response.output_item.done","output_index":0,"item":reasoning}),
                json!({"type":"response.output_item.done","output_index":1,"item":call}),
                json!({"type":"response.completed","response":{"id":"synthetic-first","status":"completed","model":"gpt-test","output":output}}),
            ];
            let wire = frames
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect::<String>();
            crate::poison::mutex(&siwc.state.stream_bodies).push_back(wire.into_bytes());
            let answer = json!({"type":"message","id":"msg_test","status":"completed",
                "role":"assistant","phase":null,"content":[{"type":"output_text",
                "text":"Synthetic final reply","annotations":[],"logprobs":[]}]});
            *crate::poison::mutex(&siwc.state.generation_body) = json!({
                "id":"synthetic-second","status":"completed","model":"gpt-test",
                "output":[answer]
            });
            let (base, task) = recovery::gateway(runtime).await;
            let client = retry_admission::bounded_client();
            let tools = json!([{"type":"function","name":"lookup","parameters":{"type":"object"}}]);
            let mut input = json!([{"role":"user","content":"Synthetic first turn"}]);
            let result = json!({"type":"function_call_output","call_id":"call_test","output":"Synthetic result"});
            let followup = json!({"role":"user","content":"Synthetic next turn"});
            for turn in 0..3 {
                for item in input.as_array_mut().unwrap() {
                    if matches!(
                        item.get("type").and_then(Value::as_str),
                        Some("reasoning" | "message" | "function_call")
                    ) {
                        item["internal_chat_message_metadata_passthrough"] = json!({
                            "turn_id":format!("synthetic-client-turn-{turn}"),
                            "create_time":turn,"content_token_count":10,
                        });
                    }
                }
                if turn == 2 {
                    crate::poison::mutex(&siwc.state.generation_body)["id"] =
                        json!("synthetic-third");
                }
                let response = client.post(format!("{base}/v1/responses"))
                    .bearer_auth("local-secret")
                    .header("x-codex-session-id", "synthetic-snapshot-session")
                    .json(&json!({"model":"gpt-test","stream":stream,"store":false,"input":input,"tools":tools,
                        "client_metadata":{"turn":format!("synthetic-turn-{turn}")}}))
                    .send().await.unwrap();
                let status = response.status();
                let text = response.text().await.unwrap();
                assert_eq!(status, StatusCode::OK, "{text}");
                if turn == 0 {
                    input = crate::poison::mutex(&siwc.state.generation_bodies)[0]["input"].clone();
                    let mut replayed_reasoning = reasoning.clone();
                    replayed_reasoning.as_object_mut().unwrap().remove("status");
                    replayed_reasoning["content"] = Value::Null;
                    let mut replayed_call = call.clone();
                    replayed_call.as_object_mut().unwrap().remove("caller");
                    input.as_array_mut().unwrap().extend([
                        replayed_reasoning,
                        replayed_call,
                        result.clone(),
                    ]);
                } else if turn == 1 {
                    let mut replayed_answer = answer.clone();
                    replayed_answer.as_object_mut().unwrap().remove("phase");
                    input
                        .as_array_mut()
                        .unwrap()
                        .extend([replayed_answer, followup.clone()]);
                }
            }
            assert_eq!(siwc.state.websocket_requests.load(Ordering::SeqCst), 1);
            assert_eq!(
                siwc.state.http_generation_requests.load(Ordering::SeqCst),
                0
            );
            assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
            let sent = crate::poison::mutex(&siwc.state.websocket_bodies);
            assert_eq!(sent.len(), 3);
            assert_eq!(
                sent[1]["previous_response_id"], "synthetic-first",
                "stream={stream}, sparse={sparse}; synthetic requests={sent:#?}"
            );
            assert_eq!(sent[1]["input"], json!([result]));
            assert_eq!(sent[2]["previous_response_id"], "synthetic-second");
            assert_eq!(sent[2]["input"], json!([followup]));
            let events = crate::poison::mutex(&events);
            assert_eq!(events.len(), 3);
            assert!(events.iter().all(|event| event.success));
            for event in &events[1..] {
                let transport = event.routing.as_ref().unwrap().transport.as_ref().unwrap();
                assert_eq!(transport.connection_reused, Some(true));
                assert_eq!(transport.connection_reset_reason, None);
                assert_eq!(
                    transport.request_payload.as_ref().unwrap().mode,
                    crate::usage::RequestPayloadMode::Continuation
                );
            }
            task.abort();
        }
    }
}

#[tokio::test]
async fn siwc_websocket_http_bridge_keeps_cache_context_across_tool_history() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    siwc.state.stream_generations.store(true, Ordering::SeqCst);
    *crate::poison::mutex(&siwc.state.generation_body) = json!({
        "id":"synthetic-siwc-ws-cache","status":"completed","model":"gpt-test",
        "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Synthetic reply"}]}]
    });
    let mut runtime = runtime(&siwc, &native).await;
    {
        let mut scheduler = runtime.lock_scheduler();
        let mut excluded = scheduler.candidate("synthetic-account-1").unwrap().clone();
        excluded.model_rules.excluded.insert("gpt-test".into());
        scheduler.upsert(excluded);
    }
    let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
    let captured = events.clone();
    let mutable = Arc::get_mut(&mut runtime).unwrap();
    mutable.diagnostics_enabled = || true;
    mutable.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
    let (base, task) = recovery::gateway(runtime).await;
    let mut socket = retry_admission::bounded_client()
        .get(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .header("x-codex-session-id", "synthetic-siwc-ws-cache")
        .upgrade()
        .send()
        .await
        .unwrap()
        .into_websocket()
        .await
        .unwrap();
    let mut input = json!([{"role":"user","content":"Synthetic first turn"}]);
    let mut generated_key = None;
    for turn in 0..2 {
        socket
            .send(Message::Text(
                json!({
                    "type":"response.create","model":"gpt-test","input":input,
                    "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]
                })
                .to_string(),
            ))
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
        .expect("SIWC HTTP bridge must finish without a native upstream upgrade");
        assert_eq!(
            terminal["type"],
            "response.completed",
            "turn={turn}: {terminal}; categories={:?}; SIWC dispatches={}; native dispatches={}",
            crate::poison::mutex(&events)
                .iter()
                .map(|event| &event.error_category)
                .collect::<Vec<_>>(),
            siwc.state.generation_requests.load(Ordering::SeqCst),
            native.state.generation_requests.load(Ordering::SeqCst)
        );
        let sent = crate::poison::mutex(&siwc.state.generation_bodies)[turn].clone();
        let key = sent["prompt_cache_key"]
            .as_str()
            .expect("the SIWC WebSocket bridge needs a stable scoped cache key");
        if turn == 0 {
            generated_key = Some(key.to_owned());
        } else {
            assert_eq!(Some(key), generated_key.as_deref());
        }
        if turn == 0 {
            assert!(sent.get("prompt_cache_options").is_none());
        } else {
            assert_eq!(
                sent["prompt_cache_options"],
                json!({"comparison_response_id":"synthetic-siwc-ws-cache"})
            );
        }
        if turn == 1 {
            assert_eq!(sent["input"], input);
        }
        input = sent["input"].clone();
        input.as_array_mut().unwrap().extend(
            terminal["response"]["output"]
                .as_array()
                .unwrap()
                .iter()
                .cloned(),
        );
        input
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":"Synthetic next turn"}));
    }
    let events = crate::poison::mutex(&events);
    assert_eq!(events.len(), 2);
    for event in events.iter() {
        assert_eq!(event.transport, crate::UsageTransport::Websocket);
        let routing = event.routing.as_ref().unwrap();
        assert!(routing.cache_context.is_some());
        assert_eq!(
            routing.transport.as_ref().unwrap().upstream,
            crate::UsageTransport::Websocket
        );
    }
    let next = events[1]
        .routing
        .as_ref()
        .unwrap()
        .cache_context
        .as_ref()
        .unwrap();
    assert_eq!(next.baseline, CacheContextBaseline::CompletedRequest);
    assert_eq!(
        next.upstream_history.comparison,
        CacheHistoryComparison::Appended
    );
    assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 2);
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    task.abort();
}
