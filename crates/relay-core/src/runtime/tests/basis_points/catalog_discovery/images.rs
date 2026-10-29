use super::*;

#[tokio::test]
async fn resolved_images_survive_json_sse_discovery_and_warm_requests() {
    for stream in [false, true] {
        for lookup in [false, true] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            *crate::poison::mutex(&server.state.attachment_file_id) =
                Some("file-synthetic-image".to_string());
            *crate::poison::mutex(&server.state.generation_body) = final_response();
            server
                .state
                .stream_generations
                .store(stream, Ordering::SeqCst);
            server
                .state
                .strict_request_schema
                .store(true, Ordering::SeqCst);
            if lookup {
                crate::poison::mutex(&server.state.generation_replies).push_back((
                    StatusCode::OK,
                    tool_response(
                        "relay.catalog",
                        "call_image_catalog",
                        json!({"names":["lookup"]}),
                    ),
                ));
            }
            let mut request = catalog_request(stream);
            if !lookup {
                request["relay_bps_tool_catalog"] = json!("compact");
            }
            request["input"] = json!([{"role":"user","content":[
                {"type":"input_text","text":"Inspect the synthetic image"},
                {"type":"input_image","detail":"high",
                 "image_url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="}
            ]}]);
            let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
            let captured = events.clone();
            let mut runtime = runtime(&server).await;
            runtime.diagnostics_enabled = || true;
            runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
            let runtime = Arc::new(runtime);
            let (base, task) = recovery::gateway(runtime).await;
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap();
            for _ in 0..2 {
                let response = client
                    .post(format!("{base}/v1/responses"))
                    .bearer_auth("local-secret")
                    .json(&request)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = response.bytes().await.unwrap();
                let body: Value = if stream {
                    let mut pending = bytes.to_vec();
                    let mut completed = None;
                    while let Some(frame) = crate::protocol::take_sse_event(&mut pending) {
                        let data = crate::protocol::sse_data(&frame);
                        if let Ok(event) = serde_json::from_slice::<Value>(&data)
                            && event["type"] == "response.completed"
                        {
                            completed = Some(event["response"].clone());
                        }
                    }
                    completed.expect("validated response completes")
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                };
                assert_eq!(body["output"][0]["content"][0]["text"], "Ready");
            }
            assert_eq!(server.state.attachment_requests.load(Ordering::SeqCst), 1);
            let bodies = crate::poison::mutex(&server.state.generation_bodies);
            assert_eq!(bodies.len(), if lookup { 3 } else { 2 });
            for body in bodies.iter() {
                let images: Vec<_> = body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|item| item["content"].as_array())
                    .flatten()
                    .filter(|part| part["type"] == "input_image")
                    .collect();
                assert_eq!(
                    images,
                    vec![&json!({"type":"input_image","file_id":"file-synthetic-image"})]
                );
                assert!(!body.to_string().contains("data:image"));
            }
            let recorded = crate::poison::mutex(&events);
            assert_eq!(recorded.len(), bodies.len());
            for (event, body) in recorded.iter().zip(bodies.iter()) {
                let context = event.tool_use.upstream_context.as_ref().unwrap();
                assert_eq!(
                    context.body_bytes,
                    serde_json::to_vec(body).unwrap().len() as u64
                );
                let composition = context.conversation.as_ref().unwrap();
                assert_eq!(composition.attachment_count, 1);
                assert_eq!(
                    composition.attachment_bytes,
                    crate::tool_policy::serialized_bytes(
                        &json!({"type":"input_image","file_id":"file-synthetic-image"}),
                    )
                );
                assert!(
                    event
                        .tool_use
                        .basis_points_catalog
                        .as_ref()
                        .unwrap()
                        .relay_tool_instruction_bytes
                        .unwrap()
                        > 0
                );
            }
            task.abort();
        }
    }
}
