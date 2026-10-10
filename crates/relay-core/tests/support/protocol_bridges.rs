use super::*;

#[tokio::test]
async fn responses_to_messages_bridge_translates_tool_turn_and_preserves_continuation() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Budget)
            .await;
    let client = reqwest::Client::new();
    let tools = json!([{
        "type": "function",
        "name": "read_file",
        "description": "Read one file",
        "parameters": {
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }
    }]);

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "Use the file tool",
            "tools": tools,
            "tool_choice": "auto",
            "reasoning": {"effort": "high"},
            "max_output_tokens": 64
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body: Value = first.json().await.unwrap();
    assert_eq!(first_body["object"], "response");
    assert!(first_body["id"]
        .as_str()
        .unwrap()
        .starts_with("resp_bridge_"));
    assert_eq!(first_body["output"][0]["type"], "function_call");
    assert_eq!(first_body["output"][0]["name"], "read_file");
    let response_id = first_body["id"].as_str().unwrap().to_string();
    let call_id = first_body["output"][0]["call_id"].as_str().unwrap();

    {
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/messages");
        assert_eq!(requests[0].authorization, None);
        assert_eq!(requests[0].x_api_key.as_deref(), Some(SOURCE_KEY));
        assert_eq!(requests[0].anthropic_version.as_deref(), Some("2023-06-01"));
    }

    {
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["model"], "claude-test");
        assert_eq!(bodies[0]["messages"][0]["role"], "user");
        assert_eq!(bodies[0]["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(bodies[0]["thinking"]["type"], "adaptive");
        assert_eq!(bodies[0]["output_config"]["effort"], "high");
    }

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "previous_response_id": response_id,
            "input": [{
                "type": "function_call_output",
                "call_id": call_id,
                "output": {"ok": true}
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second_body: Value = second.json().await.unwrap();
    assert_eq!(second_body["output"][0]["type"], "message");
    assert_eq!(
        second_body["output"][0]["content"][0]["text"],
        "Tool result received"
    );

    {
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        let messages = bodies[1]["messages"].as_array().unwrap();
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        assert_eq!(messages[1]["content"][0]["id"], call_id);
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(messages[2]["content"][0]["tool_use_id"], call_id);
    }

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn bridge_skips_incompatible_candidate_without_cooling_it() {
    let (upstream, state) = spawn_messages_upstream().await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let usage_events = events.clone();
    let source = |id: &str, priority: i32, reasoning_mode| RuntimeSource {
        source: ProviderSource {
            id: id.to_string(),
            name: format!("Synthetic {id}"),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.to_string(),
            wire_api: WireApi::Messages,
            models: vec!["claude-test".to_string()],
        },
        protocol_config: SourceProtocolConfig {
            endpoint_hint: Some(WireApi::Messages),
            ..SourceProtocolConfig::default()
        },
        protocol_bindings: vec![SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode,
            cache_write_ttl: Default::default(),
            model_ids: vec!["claude-test".to_string()],
        }],
        enabled: true,
        draining: false,
        priority,
        weight: 1,
        recovery_delay_seconds: 0,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        last_used_at_ms: None,
    };
    let runtime = Arc::new(
        GatewayRuntime::from_pool(
            vec![
                source("incompatible", 10, MessagesReasoningMode::Disabled),
                source("compatible", 0, MessagesReasoningMode::Adaptive),
            ],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "local-key-1".to_string(),
                secret: LOCAL_KEY.to_string(),
            })],
            GatewayRuntimeOptions {
                model_reasoning_allowed_levels: std::collections::BTreeMap::from([(
                    "claude-test".to_string(),
                    vec!["high".to_string()],
                )]),
                ..GatewayRuntimeOptions::default()
            },
            Arc::new(move |event| usage_events.lock().unwrap().push(event)),
        )
        .unwrap(),
    );
    let gateway = spawn(gateway::router(runtime.clone())).await;
    prime_source_metadata(&gateway).await;
    state.requests.lock().unwrap().clear();
    state.bodies.lock().unwrap().clear();

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "use reasoning",
            "reasoning": {"effort": "high"}
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    assert_eq!(events.lock().unwrap().len(), 1);
    assert!(events.lock().unwrap()[0].success);
    let incompatible = runtime
        .candidate_runtime_order()
        .into_iter()
        .find(|candidate| candidate.candidate_id.contains("incompatible"))
        .unwrap();
    assert!(incompatible.available);
    assert_eq!(incompatible.next_retry_at_ms, None);
}

#[tokio::test]
async fn responses_to_messages_bridge_translates_custom_tool_turn_and_continuation() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "List the project files.",
            "tools": [{
                "type": "custom",
                "name": "PowerShell",
                "description": "Runs one PowerShell command."
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body: Value = first.json().await.unwrap();
    assert_eq!(first_body["output"][0]["type"], "custom_tool_call");
    assert_eq!(first_body["output"][0]["name"], "PowerShell");
    assert_eq!(first_body["output"][0]["input"], "Get-ChildItem -Force");
    let response_id = first_body["id"].as_str().unwrap().to_string();

    {
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["tools"][0]["name"], "PowerShell");
        assert_eq!(
            bodies[0]["tools"][0]["input_schema"]["properties"]["input"]["type"],
            "string"
        );
    }

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "previous_response_id": response_id,
            "input": [{
                "type": "custom_tool_call_output",
                "call_id": "tool_powershell_1",
                "output": "Cargo.toml\nsrc"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second_body: Value = second.json().await.unwrap();
    assert_eq!(
        second_body["output"][0]["content"][0]["text"],
        "Tool result received"
    );

    {
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(
            bodies[1]["messages"][1]["content"][0],
            json!({
                "type": "tool_use",
                "id": "tool_powershell_1",
                "name": "PowerShell",
                "input": {"input": "Get-ChildItem -Force"}
            })
        );
        assert_eq!(
            bodies[1]["messages"][2]["content"][0],
            json!({
                "type": "tool_result",
                "tool_use_id": "tool_powershell_1",
                "content": "Cargo.toml\nsrc"
            })
        );
    }

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn source_can_mix_native_and_bridged_responses_models() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, _) = spawn_mixed_responses_gateway(&upstream.base_url).await;
    let client = reqwest::Client::new();

    let native = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "native route"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(native.status(), StatusCode::OK);
    assert_eq!(native.json::<Value>().await.unwrap()["id"], "resp_test");

    let bridged = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-oai-attestation", "must-not-reach-messages")
        .json(&json!({
            "model": "claude-test",
            "input": "bridge route"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bridged.status(), StatusCode::OK);
    let bridged_body: Value = bridged.json().await.unwrap();
    assert!(bridged_body["id"]
        .as_str()
        .is_some_and(|id| id.starts_with("resp_bridge_")));
    assert_eq!(
        bridged_body["output"][0]["content"][0]["text"],
        "Native Messages response"
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer upstream-test-key")
    );
    assert_eq!(requests[1].path, "/v1/messages");
    assert_eq!(requests[1].authorization, None);
    assert_eq!(requests[1].x_api_key.as_deref(), Some(SOURCE_KEY));
    assert_eq!(requests[1].anthropic_version.as_deref(), Some("2023-06-01"));
    assert_eq!(requests[1].x_oai_attestation, None);
}

#[tokio::test]
async fn responses_to_messages_bridge_translates_plain_response() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "Give a plain answer"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["object"], "response");
    assert_eq!(body["status"], "completed");
    assert_eq!(body["model"], "claude-test");
    assert_eq!(body["output"][0]["type"], "message");
    assert_eq!(
        body["output"][0]["content"][0]["text"],
        "Native Messages response"
    );
    assert_eq!(body["usage"]["input_tokens"], 2);
    assert_eq!(body["usage"]["output_tokens"], 2);

    {
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/messages");
        assert_eq!(requests[0].authorization, None);
        assert_eq!(requests[0].x_api_key.as_deref(), Some(SOURCE_KEY));
        assert_eq!(requests[0].anthropic_version.as_deref(), Some("2023-06-01"));
    }
    {
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["model"], "claude-test");
        assert_eq!(bodies[0]["messages"][0]["role"], "user");
        assert_eq!(
            bodies[0]["messages"][0]["content"][0]["text"],
            "Give a plain answer"
        );
    }
    assert_eq!(events.lock().unwrap().len(), 1);
    assert!(events.lock().unwrap()[0].success);
}

#[tokio::test]
async fn responses_to_gemini_bridge_uses_native_routes_for_plain_and_streaming_requests() {
    let (upstream, state) = spawn_gemini_upstream().await;
    let (gateway, events) = spawn_gemini_bridge_gateway(&upstream.base_url).await;
    let client = reqwest::Client::new();

    let plain = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-goog-api-key", "client-google-key")
        .header("x-oai-attestation", "client-attestation")
        .json(&json!({
            "model": "gemini-test",
            "input": "Give a plain answer",
            "temperature": 0.2,
            "max_output_tokens": 32,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(plain.status(), StatusCode::OK);
    let plain: Value = plain.json().await.unwrap();
    assert_eq!(
        plain["output"][0]["content"][0]["text"],
        "Native Gemini response"
    );
    assert_eq!(plain["usage"]["input_tokens"], 2);
    assert_eq!(plain["usage"]["output_tokens"], 3);

    let stream = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gemini-test",
            "input": "Stream a response",
            "stream": true,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    let stream = stream.text().await.unwrap();
    assert!(stream.contains("\"type\":\"response.output_text.delta\""));
    assert!(stream.contains("Native Gemini stream"));
    assert!(stream.contains("\"type\":\"response.completed\""));

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/v1/models/gemini-test:generateContent");
    assert_eq!(
        requests[1].path,
        "/v1/models/gemini-test:streamGenerateContent"
    );
    assert!(requests
        .iter()
        .all(|request| request.authorization.is_none()));
    assert!(requests
        .iter()
        .all(|request| request.x_goog_api_key.as_deref() == Some(SOURCE_KEY)));
    assert!(requests
        .iter()
        .all(|request| request.x_oai_attestation.is_none()));
    drop(requests);

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(
        bodies[0]["contents"][0]["parts"][0]["text"],
        "Give a plain answer"
    );
    assert_eq!(bodies[0]["generationConfig"]["temperature"], 0.2);
    assert_eq!(bodies[0]["generationConfig"]["maxOutputTokens"], 32);
    assert!(bodies.iter().all(|body| body.get("model").is_none()));
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn gemini_terminal_without_output_reaches_responses_bridge_and_native_stream() {
    for (name, upstream_body, reason) in [
        (
            "prompt_blocked",
            json!({"promptFeedback":{"blockReason":"SAFETY"},"candidates":[],"usageMetadata":{"promptTokenCount":3}}),
            "content_filter",
        ),
        (
            "candidate_filtered",
            json!({"candidates":[{"finishReason":"SAFETY"}],"usageMetadata":{"promptTokenCount":3}}),
            "content_filter",
        ),
        (
            "candidate_token_limit",
            json!({"candidates":[{"finishReason":"MAX_TOKENS"}],"usageMetadata":{"promptTokenCount":3}}),
            "max_output_tokens",
        ),
    ] {
        let frame = format!("data: {upstream_body}\n\n");
        let upstream = spawn(Router::new().route(
            "/v1/models/gemini-test:streamGenerateContent",
            post(move || {
                let frame = frame.clone();
                async move {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(frame))
                        .unwrap()
                }
            }),
        ))
        .await;
        let (gateway, events) = spawn_gemini_bridge_gateway(&upstream.base_url).await;
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":"gemini-test","input":"Synthetic request","stream":true}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{name}");
        let text = response.text().await.unwrap();
        assert!(
            text.contains("event: response.incomplete"),
            "{name}: {text}"
        );
        assert!(
            text.contains(&format!("\"reason\":\"{reason}\"")),
            "{name}: {text}"
        );
        assert!(
            !text.contains("event: response.completed"),
            "{name}: {text}"
        );
        {
            {
                let events = events.lock().unwrap();
                assert_eq!(events.len(), 1, "{name}");
                assert!(!events[0].success, "{name}");
                assert_eq!(
                    events[0].error_category.as_deref(),
                    Some("response_incomplete"),
                    "{name}"
                );
                assert_eq!(events[0].input_tokens, Some(3), "{name}");
            }
        }

        let (native, events) = spawn_native_gemini_gateway(&upstream.base_url).await;
        let response = reqwest::Client::new()
            .post(format!(
                "{}/v1beta/models/gemini-test:streamGenerateContent",
                native.base_url
            ))
            .header("x-goog-api-key", LOCAL_KEY)
            .json(&json!({"contents":[{"role":"user","parts":[{"text":"Synthetic request"}]}]}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "native {name}");
        let text = response.text().await.unwrap();
        assert!(
            text.contains(&upstream_body.to_string()),
            "native {name}: {text}"
        );
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1, "native {name}");
        assert!(!events[0].success, "native {name}");
        assert_eq!(
            events[0].error_category.as_deref(),
            Some("response_incomplete"),
            "native {name}"
        );
        assert_eq!(events[0].input_tokens, Some(3), "native {name}");
    }
}

#[tokio::test]
async fn native_gemini_truncated_stream_is_not_recorded_as_success() {
    let upstream = spawn(Router::new().route(
        "/v1/models/gemini-test:streamGenerateContent",
        post(|| async {
            Response::builder()
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from(
                    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Partial reply\"}]}}]}\n\n",
                ))
                .unwrap()
        }),
    ))
    .await;
    let (gateway, events) = spawn_native_gemini_gateway(&upstream.base_url).await;
    let response = reqwest::Client::new()
        .post(format!(
            "{}/v1beta/models/gemini-test:streamGenerateContent",
            gateway.base_url
        ))
        .header("x-goog-api-key", LOCAL_KEY)
        .json(&json!({"contents":[{"role":"user","parts":[{"text":"Synthetic request"}]}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("Partial reply"));
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("stream_incomplete")
    );
}

#[tokio::test]
async fn native_gemini_stream_forwards_media_part_and_records_terminal_usage() {
    let upstream_body = json!({
        "candidates": [{"content": {"role": "model", "parts": [
            {"inlineData": {"mimeType": "image/png", "data": "YQ=="}}
        ]}, "finishReason": "STOP"}],
        "usageMetadata": {"promptTokenCount": 2, "candidatesTokenCount": 4}
    });
    let frame = format!("data: {upstream_body}\n\n");
    let upstream = spawn(Router::new().route(
        "/v1/models/gemini-test:streamGenerateContent",
        post(move || {
            let frame = frame.clone();
            async move {
                Response::builder()
                    .header(CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(frame))
                    .unwrap()
            }
        }),
    ))
    .await;
    let (gateway, events) = spawn_native_gemini_gateway(&upstream.base_url).await;
    let response = reqwest::Client::new()
        .post(format!(
            "{}/v1beta/models/gemini-test:streamGenerateContent",
            gateway.base_url
        ))
        .header("x-goog-api-key", LOCAL_KEY)
        .json(&json!({"contents":[{"role":"user","parts":[{"text":"Synthetic request"}]}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.text().await.unwrap(),
        format!("data: {upstream_body}\n\n")
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].input_tokens, Some(2));
    assert_eq!(events[0].output_tokens, Some(4));
}

#[tokio::test]
async fn native_gemini_client_route_keeps_native_body_and_response_contract() {
    let (upstream, state) = spawn_gemini_upstream().await;
    let (gateway, events) = spawn_native_gemini_gateway(&upstream.base_url).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/v1beta/models/gemini-test:generateContent",
            gateway.base_url
        ))
        .header("x-goog-api-key", LOCAL_KEY)
        .json(&json!({
            "model": "gemini-test",
            "contents": [{"role": "user", "parts": [{"text": "Native route"}]}]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["candidates"][0]["content"]["parts"][0]["text"],
        "Native Gemini response"
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/models/gemini-test:generateContent");
    assert!(requests[0].authorization.is_none());
    assert_eq!(requests[0].x_goog_api_key.as_deref(), Some(SOURCE_KEY));
    drop(requests);

    let bodies = state.bodies.lock().unwrap();
    assert!(bodies[0].get("model").is_none());
    assert_eq!(bodies[0]["contents"][0]["parts"][0]["text"], "Native route");
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].wire_api, WireApi::Gemini);
}

#[tokio::test]
async fn responses_to_messages_bridge_preserves_effort_and_rejects_incompatible_sampling() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Adaptive)
            .await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "answer",
            "reasoning": {"effort": "high"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    {
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["thinking"]["type"], "adaptive");
        assert_eq!(bodies[0]["output_config"]["effort"], "high");
        assert!(bodies[0].get("temperature").is_none());
        assert!(bodies[0].get("top_p").is_none());
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].requested_reasoning_effort.as_deref(),
            Some("high")
        );
        assert_eq!(
            events[0].effective_reasoning_effort.as_deref(),
            Some("high")
        );
    }
    for parameter in ["temperature", "top_p"] {
        let mut request =
            json!({"model":"claude-test","input":"answer","reasoning":{"effort":"high"}});
        request[parameter] = json!(0.2);
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(state.bodies.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn legacy_disabled_reasoning_mode_is_ignored_and_unmatched_forced_tools_are_rejected() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, _) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;
    let client = reqwest::Client::new();

    let reasoning = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "reason",
            "reasoning": {"effort": "high"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reasoning.status(), StatusCode::OK);

    let opaque_tool = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "tool",
            "tools": [{"type": "web_search"}],
            "tool_choice": {"type": "function", "name": "missing"}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(opaque_tool.status(), StatusCode::BAD_REQUEST);
    let body: Value = opaque_tool.json().await.unwrap();
    assert_eq!(body["error"]["code"], "adapter_tool_unsupported");
    {
        let requests = state.requests.lock().unwrap();
        let bodies = state.bodies.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["thinking"]["type"], "adaptive");
    }

    // A hosted tool has no Messages equivalent: it is dropped, not rejected.
    let hosted_only = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "tool",
            "tools": [{"type": "web_search"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(hosted_only.status(), StatusCode::OK);
    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert!(bodies[1]
        .get("tools")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty));
}

#[tokio::test]
async fn missing_bridge_continuation_is_rejected_without_context_free_tool_output() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, _) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "previous_response_id": "resp_bridge_missing",
            "input": [{
                "type": "function_call_output",
                "call_id": "tool_missing",
                "output": "unsafe to send without context"
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "response_continuation_unavailable");
    assert!(state.requests.lock().unwrap().is_empty());
    assert!(state.bodies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn responses_to_messages_bridge_translates_sse_text_and_tool_arguments() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "stream",
            "stream": true,
            "tools": [{
                "type": "function",
                "name": "read_file",
                "parameters": {"type": "object"}
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body = response.text().await.unwrap();
    assert!(body.contains("\"type\":\"response.created\""));
    assert!(body.contains("\"type\":\"response.function_call_arguments.delta\""));
    assert!(body.contains("\"type\":\"response.function_call_arguments.done\""));
    assert!(body.contains("\"delta\":\"{\\\"path\\\":\\\"/tmp/a\\\"}\""));
    assert!(body.contains("\"type\":\"response.output_item.done\""));
    assert!(body.contains("\"type\":\"response.completed\""));
    assert_eq!(state.bodies.lock().unwrap().len(), 1);
    assert_eq!(events.lock().unwrap().len(), 1);
    assert!(events.lock().unwrap()[0].success);
}

#[tokio::test]
async fn responses_to_messages_bridge_preserves_plain_stream_context_for_http_continuation() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "Remember this turn",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_body = first.text().await.unwrap();
    let response_id = first_body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find_map(|event| {
            (event["type"] == "response.completed")
                .then(|| event["response"]["id"].as_str().map(str::to_string))
                .flatten()
        })
        .expect("the completed bridge stream exposes a response id");

    let second = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "previous_response_id": response_id,
            "input": "What did I ask you to remember?"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);

    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    let messages = bodies[1]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"][0]["text"], "Remember this turn");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"][0]["text"], "Streaming context");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(
        messages[2]["content"][0]["text"],
        "What did I ask you to remember?"
    );
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn malformed_messages_response_is_redacted_as_adapter_error() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Disabled)
            .await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "claude-test",
            "input": "malformed"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "adapter_upstream_response_invalid");
    assert_eq!(body["error"]["zenith_relay"]["origin"], "provider");
    assert!(!body.to_string().contains("provider-private-body"));
    assert_eq!(state.bodies.lock().unwrap().len(), 1);
    assert!(!events.lock().unwrap()[0].success);
    assert_eq!(
        events.lock().unwrap()[0].error_origin(),
        Some(ErrorOrigin::Provider)
    );
}
