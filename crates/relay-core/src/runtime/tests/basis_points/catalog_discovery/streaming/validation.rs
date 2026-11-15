use super::*;

#[tokio::test]
async fn hosted_search_metadata_enrichment_reaches_the_client_terminal() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        let search = json!({"type":"web_search_call","id":"ws_synthetic","status":"completed",
            "action":{"type":"search","query":"synthetic query"}});
        let mut terminal_search = search.clone();
        terminal_search["action"]["sources"] =
            json!([{"type":"url","url":"https://example.test/source"}]);
        let mut reply = final_response();
        reply["output"] = json!([terminal_search, visible_message("Search complete")]);
        let frames = [
            json!({"type":"response.created","response":{"id":reply["id"],"status":"in_progress","model":"gpt-test"}}),
            json!({"type":"response.output_item.done","output_index":0,"item":search}),
            json!({"type":"response.output_text.delta","output_index":1,"content_index":0,
                "item_id":"msg_visible","delta":"Search complete"}),
            json!({"type":"response.output_item.done","output_index":1,"item":reply["output"][1]}),
            json!({"type":"response.completed","response":reply}),
        ];
        let wire = frames
            .iter()
            .map(|frame| format!("data: {frame}\n\n"))
            .collect::<String>();
        crate::poison::mutex(&server.state.stream_bodies).push_back(wire.into_bytes());
        let (base, task) = recovery::gateway(Arc::new(runtime(&server).await)).await;
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&catalog_request(stream))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("Search complete"), "{body}");
        assert!(body.contains("https://example.test/source"), "{body}");
        assert!(!body.contains("stream_invalid"), "{body}");
        if stream {
            assert_eq!(
                body.matches("event: response.completed").count(),
                1,
                "{body}"
            );
        } else {
            assert_eq!(
                serde_json::from_str::<Value>(&body).unwrap()["status"],
                "completed"
            );
        }
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        task.abort();
    }
}

#[tokio::test]
async fn optional_completed_reasoning_status_does_not_fail_bps_text_completion() {
    for stream in [false, true] {
        for omitted_in_done in [false, true] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            server
                .state
                .stream_generations
                .store(true, Ordering::SeqCst);
            let mut done = json!({"type":"reasoning","id":"rs_synthetic","summary":[],
                "encrypted_content":"synthetic-done-cipher"});
            let mut reasoning = done.clone();
            if omitted_in_done {
                reasoning["status"] = json!("completed");
            } else {
                done["status"] = json!("completed");
            }
            let mut reply = final_response();
            reply["output"] = json!([reasoning, visible_message("Complete answer")]);
            let events = [
                json!({"type":"response.created","response":{"id":reply["id"],"status":"in_progress","model":"gpt-test"}}),
                json!({"type":"response.output_item.done","output_index":0,"item":done}),
                json!({"type":"response.output_text.delta","output_index":1,"content_index":0,
                    "item_id":"msg_visible","delta":"Complete answer"}),
                json!({"type":"response.output_item.done","output_index":1,"item":reply["output"][1]}),
                json!({"type":"response.completed","response":reply}),
            ];
            let wire = events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect::<String>();
            crate::poison::mutex(&server.state.stream_bodies).push_back(wire.into_bytes());
            let (base, task) = recovery::gateway(Arc::new(runtime(&server).await)).await;
            let response = reqwest::Client::new()
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&catalog_request(stream))
                .send()
                .await
                .unwrap();
            let status = response.status();
            let body = response.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{body}");
            assert!(body.contains("Complete answer"), "{body}");
            assert!(!body.contains("stream_invalid"), "{body}");
            if stream {
                assert!(body.contains("response.completed"), "{body}");
            } else {
                assert_eq!(
                    serde_json::from_str::<Value>(&body).unwrap()["status"],
                    "completed"
                );
            }
            assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
            task.abort();
        }
    }
}

#[tokio::test]
async fn terminal_mismatch_keeps_the_reason_usage_and_single_send_after_text() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        let message = visible_message("Visible prefix");
        let mut reply = final_response();
        reply["output"] = json!([visible_message("synthetic-private-terminal")]);
        reply["usage"] = json!({"input_tokens":120,"input_tokens_details":{"cached_tokens":100},
            "output_tokens":5,"total_tokens":125});
        let frames = [
            json!({"type":"response.created","response":{"id":reply["id"],"status":"in_progress","model":"gpt-test"}}),
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,
                "item_id":"msg_visible","delta":"Visible prefix"}),
            json!({"type":"response.output_item.done","output_index":0,"item":message}),
            json!({"type":"response.completed","response":reply}),
        ];
        let wire = frames
            .iter()
            .map(|frame| format!("data: {frame}\n\n"))
            .collect::<String>();
        crate::poison::mutex(&server.state.stream_bodies).push_back(wire.into_bytes());
        let gate = Arc::new(Semaphore::new(0));
        if stream {
            crate::poison::mutex(&server.state.stream_gates).push_back(gate.clone());
        }
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let finished = Arc::new(Notify::new());
        let notified = finished.clone();
        let mut runtime = runtime(&server).await;
        runtime.usage = Arc::new(move |event| {
            crate::poison::mutex(&captured).push(event);
            notified.notify_one();
        });
        let (base, task) = recovery::gateway(Arc::new(runtime)).await;
        let mut response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&catalog_request(stream))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if stream {
                StatusCode::OK
            } else {
                StatusCode::BAD_GATEWAY
            }
        );
        let prefix = if stream {
            let prefix = read_prefix(&mut response, "Visible prefix").await;
            gate.add_permits(1);
            prefix
        } else {
            String::new()
        };
        let body = prefix + &response.text().await.unwrap();
        assert!(body.contains("stream_invalid"), "{body}");
        assert!(body.contains("response.output_item.content"), "{body}");
        assert!(body.contains("item_kind=message"), "{body}");
        assert!(body.contains("snapshot_reason=field_changed"), "{body}");
        assert!(body.contains("before=array, after=array"), "{body}");
        assert!(!body.contains("before client output"));
        assert!(!body.contains("synthetic-private"));
        assert!(!body.contains("response.completed"));
        tokio::time::timeout(std::time::Duration::from_secs(3), finished.notified())
            .await
            .unwrap();
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        let events = crate::poison::mutex(&events);
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert!(!event.success);
        assert_eq!(event.error_category.as_deref(), Some("stream_invalid"));
        assert_eq!(event.input_tokens, Some(120));
        assert_eq!(event.cached_input_tokens, Some(100));
        assert_eq!(event.output_tokens, Some(5));
        let details = event.upstream_error.as_ref().unwrap();
        assert_eq!(
            details.parameter.as_deref(),
            Some("response.output_item.content")
        );
        assert!(
            details
                .message
                .as_ref()
                .unwrap()
                .contains("snapshot_reason=field_changed")
        );
        assert!(
            !serde_json::to_string(details)
                .unwrap()
                .contains("synthetic-private")
        );
        task.abort();
    }
}
