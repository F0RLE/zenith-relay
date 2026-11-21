use super::*;

const SESSION: &str = "synthetic-discovery-replay";

async fn turn(client: &reqwest::Client, base: &str, body: &Value, session: &str) -> Value {
    let response = client
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .header("x-codex-session-id", session)
        .json(body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{text}");
    terminal(&text, body["stream"] == true)
}

#[tokio::test]
async fn siwc_catalog_rejected_tool_never_becomes_a_socket_checkpoint() {
    use crate::usage::FullRequestReason;

    for stream in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        let mut message = answer()["output"][0].clone();
        message["id"] = json!("msg_before_rejection");
        let mut call = calls()["output"][0].clone();
        call["arguments"] = json!("{}");
        let rejected = response("resp_rejected_tool", json!([message, call]));
        let recovered = response("resp_recovered", answer()["output"].clone());
        crate::poison::mutex(&siwc.state.generation_replies).extend([
            (StatusCode::OK, rejected),
            (StatusCode::OK, recovered),
            (StatusCode::OK, answer()),
        ]);
        let mut runtime = runtime(&siwc, &native).await;
        {
            let mut scheduler = runtime.lock_scheduler();
            let mut alternative = scheduler.candidate("synthetic-account-1").unwrap().clone();
            alternative.model_rules.excluded.insert("gpt-test".into());
            scheduler.upsert(alternative);
        }
        let usage = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let capture = usage.clone();
        let settings = Arc::get_mut(&mut runtime).unwrap();
        settings.diagnostics_enabled = || true;
        settings.usage = Arc::new(move |event| crate::poison::mutex(&capture).push(event));
        let (base, task) = recovery::gateway(runtime).await;
        let client = retry_admission::bounded_client();
        let mut body = request(stream);
        let declaration = json!({
            "type":"additional_tools","role":"developer","tools":[body["tools"][32]]
        });
        body["input"].as_array_mut().unwrap().push(declaration);
        let failed = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .header("x-codex-session-id", SESSION)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = failed.status();
        let text = failed.text().await.unwrap();
        assert!(text.contains("adapter_upstream_response_invalid"), "{text}");
        assert!(!text.contains("call_native_read"), "{text}");
        if stream {
            assert_eq!(status, StatusCode::OK);
            assert!(events(&text).any(|event| event["type"] == "response.output_text.delta"));
        } else {
            assert_eq!(status, StatusCode::BAD_GATEWAY);
        }
        assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 1);

        // Even a byte-identical replay of the rejected provider call is not
        // evidence that Relay emitted or accepted it.
        body["input"].as_array_mut().unwrap().extend([
            message,
            call,
            json!({"type":"function_call_output","call_id":"call_native_read","output":"synthetic"}),
        ]);
        let recovery = turn(&client, &base, &body, SESSION).await;
        assert_eq!(recovery["id"], "resp_recovered");
        let input = body["input"].as_array_mut().unwrap();
        input.extend_from_slice(recovery["output"].as_array().unwrap());
        let followup = json!({"role":"user","content":"Synthetic follow-up"});
        input.push(followup.clone());
        turn(&client, &base, &body, SESSION).await;

        let frames = crate::poison::mutex(&siwc.state.websocket_bodies);
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames[1].get("previous_response_id"),
            None,
            "stream={stream}"
        );
        assert_eq!(frames[2]["previous_response_id"], "resp_recovered");
        assert_eq!(frames[2]["input"], json!([followup]));
        assert_eq!(siwc.state.websocket_requests.load(Ordering::SeqCst), 1);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        let events = crate::poison::mutex(&usage);
        let attempts: Vec<_> = events
            .iter()
            .filter(|event| !event.is_request_completion())
            .collect();
        assert_eq!(attempts.len(), 3);
        assert!(!attempts[0].success);
        let transport = attempts[1]
            .routing
            .as_ref()
            .and_then(|routing| routing.transport.as_ref())
            .unwrap();
        assert_eq!(transport.connection_reused, Some(true));
        assert_eq!(
            transport
                .request_payload
                .as_ref()
                .unwrap()
                .full_request_reason,
            Some(FullRequestReason::SocketCheckpointUnavailable)
        );
        task.abort();
    }
}

#[tokio::test]
async fn siwc_catalog_replay_sends_only_new_results_after_private_lookup() {
    for (stream, already_loaded, repeat) in [
        (false, false, false),
        (true, false, true),
        (true, true, false),
    ] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        let mut search = lookup("resp_private_lookup");
        if repeat {
            search["output"].as_array_mut().unwrap().insert(0, json!({
                "id":"rs_public","type":"reasoning","summary":[],"encrypted_content":"synthetic-opaque",
                "metadata":{"turn_id":"12345678-1234-1234-1234-123456789abc"},
                "internal_chat_message_metadata_passthrough":{"turn_id":"12345678-1234-1234-1234-123456789abc"}
            }));
        }
        let mut replies = vec![(StatusCode::OK, search)];
        if repeat {
            replies.push((StatusCode::OK, lookup("resp_repeated_lookup")));
        }
        replies.push((StatusCode::OK, calls()));
        crate::poison::mutex(&siwc.state.generation_replies).extend(replies);
        *crate::poison::mutex(&siwc.state.generation_body) = answer();
        let (base, task) = recovery::gateway(runtime(&siwc, &native).await).await;
        let client = retry_admission::bounded_client();
        let mut body = request(stream);
        body["input"][0]["content"] = json!("Synthetic history. ".repeat(32_000));
        if already_loaded {
            let loaded = json!({"type":"additional_tools","role":"developer","tools":[
                body["tools"][32],body["tools"][33]
            ]});
            body["input"].as_array_mut().unwrap().insert(0, loaded);
        }
        let mut reply = turn(&client, &base, &body, SESSION).await;
        if repeat {
            assert_eq!(reply["output"][0]["id"], "rs_public");
            reply["output"][0]
                .as_object_mut()
                .unwrap()
                .remove("metadata");
        }
        let results = json!([
            {"type":"function_call_output","call_id":"call_native_read","output":"Synthetic read"},
            {"type":"custom_tool_call_output","call_id":"call_native_edit","output":"Synthetic edit"}
        ]);
        let input = body["input"].as_array_mut().unwrap();
        input.extend_from_slice(reply["output"].as_array().unwrap());
        input.extend_from_slice(results.as_array().unwrap());
        let next = turn(&client, &base, &body, SESSION).await;
        assert_eq!(next["output"], answer()["output"]);
        let input = body["input"].as_array_mut().unwrap();
        input.extend_from_slice(next["output"].as_array().unwrap());
        let last = json!({"role":"user","content":"Synthetic follow-up"});
        input.push(last.clone());
        turn(&client, &base, &body, SESSION).await;

        let frames = crate::poison::mutex(&siwc.state.websocket_bodies);
        let after = 2 + usize::from(repeat);
        assert_eq!(frames.len(), after + 2);
        assert_eq!(frames[1]["previous_response_id"], "resp_private_lookup");
        if repeat {
            assert_eq!(frames[2]["previous_response_id"], "resp_repeated_lookup");
        }
        let bytes = |frame: &Value| serde_json::to_vec(frame).unwrap().len();
        let sent = bytes(&frames[after]);
        assert_eq!(
            frames[after]["previous_response_id"], "resp_native_calls",
            "first turn after discovery sent {sent} JSON bytes"
        );
        assert_eq!(frames[after]["input"], results);
        assert!(sent < 2048 && bytes(&frames[0]) > 600_000);
        eprintln!("synthetic SIWC discovery replay: {sent} JSON bytes");
        assert_eq!(
            frames[after + 1]["previous_response_id"],
            "resp_native_answer"
        );
        assert_eq!(frames[after + 1]["input"], json!([last]));
        let bodies = crate::poison::mutex(&siwc.state.generation_bodies);
        let mut expected = bodies[after - 1]["input"].as_array().unwrap().clone();
        expected.extend_from_slice(calls()["output"].as_array().unwrap());
        expected.extend_from_slice(results.as_array().unwrap());
        assert!(bodies[after]["input"].as_array().unwrap() == &expected);
        assert_eq!(siwc.state.websocket_requests.load(Ordering::SeqCst), 1);
        task.abort();
    }
}

#[tokio::test]
async fn siwc_catalog_replay_keeps_full_history_for_unproven_changes() {
    for change in [
        "prefix",
        "ciphertext",
        "arguments",
        "contract",
        "later_contract",
        "client_declaration",
        "unused",
        "other_session",
        "mixed_final",
    ] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        let mut search = lookup("resp_private");
        if change == "unused" {
            search["output"][0]["arguments"] =
                json!(json!({"names":["files.read","edit","lookup_1"]}).to_string());
        }
        search["output"].as_array_mut().unwrap().insert(0, json!({
            "type":"reasoning","id":"rs_visible","summary":[],"encrypted_content":"synthetic-opaque"
        }));
        let mut final_response = calls();
        if change == "mixed_final" {
            final_response["output"]
                .as_array_mut()
                .unwrap()
                .push(lookup("resp_hidden_mixed")["output"][0].clone());
        }
        crate::poison::mutex(&siwc.state.generation_replies)
            .extend([(StatusCode::OK, search), (StatusCode::OK, final_response)]);
        *crate::poison::mutex(&siwc.state.generation_body) = answer();
        let (base, task) = recovery::gateway(runtime(&siwc, &native).await).await;
        let client = retry_admission::bounded_client();
        let mut body = request(false);
        let reply = turn(&client, &base, &body, SESSION).await;
        let input = body["input"].as_array_mut().unwrap();
        input.extend_from_slice(reply["output"].as_array().unwrap());
        input.push(json!({"type":"function_call_output","call_id":"call_native_read","output":"Synthetic read"}));
        input.push(json!({"type":"custom_tool_call_output","call_id":"call_native_edit","output":"Synthetic edit"}));
        match change {
            "prefix" => body["input"][0]["content"] = json!("Edited user input"),
            "ciphertext" => body["input"][1]["encrypted_content"] = json!("different"),
            "arguments" => body["input"][2]["arguments"] = json!("{\"path\":\"changed\"}"),
            "contract" | "later_contract" => {
                if change == "later_contract" {
                    let next = turn(&client, &base, &body, SESSION).await;
                    let input = body["input"].as_array_mut().unwrap();
                    input.extend_from_slice(next["output"].as_array().unwrap());
                    input.push(json!({"role":"user","content":"Next turn"}));
                }
                body["tools"][0]["description"] = json!("Changed deferred tool");
            }
            "client_declaration" => {
                let declaration = json!({"type":"additional_tools","role":"developer","tools":[body["tools"][32]]});
                body["input"].as_array_mut().unwrap().insert(2, declaration);
            }
            "unused" | "other_session" | "mixed_final" => {}
            _ => unreachable!(),
        }
        let session = if change == "other_session" {
            "synthetic-other"
        } else {
            SESSION
        };
        turn(&client, &base, &body, session).await;
        let frames = crate::poison::mutex(&siwc.state.websocket_bodies);
        assert_eq!(frames.len(), 3 + usize::from(change == "later_contract"));
        assert_eq!(frames[1]["previous_response_id"], "resp_private");
        if change == "later_contract" {
            assert_eq!(frames[2]["previous_response_id"], "resp_native_calls");
        }
        let frame = frames.last().unwrap();
        assert!(frame.get("previous_response_id").is_none(), "{change}");
        let full = frame["input"].as_array().unwrap();
        assert!(full.contains(&body["input"][0]));
        assert!(full.contains(&body["input"][1]));
        assert!(full.contains(body["input"].as_array().unwrap().last().unwrap()));
        assert!(
            !full
                .iter()
                .any(|i| i["type"] == "function_call" && i["name"] == "relay_catalog")
        );
        task.abort();
    }
}
