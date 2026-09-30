use super::*;

#[tokio::test]
async fn chat_completions_preserves_native_tools_and_bridges_responses() {
    let chat = json!({
        "id": "chat-1",
        "object": "chat.completion",
        "created": 123,
        "model": "chat-model",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "translated"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}
    });
    let (chat_server, state) = spawn_upstream(
        "chat-key",
        vec![
            Reply::Json {
                status: StatusCode::OK,
                body: chat.clone(),
                cache_control: "chat",
                retry_after: None,
            },
            Reply::Json {
                status: StatusCode::OK,
                body: chat.clone(),
                cache_control: "chat",
                retry_after: None,
            },
            Reply::Json {
                status: StatusCode::OK,
                body: chat.clone(),
                cache_control: "chat",
                retry_after: None,
            },
            Reply::Json {
                status: StatusCode::OK,
                body: json!({
                    "id": "chat-tools", "object": "chat.completion", "model": "chat-model",
                    "choices": [{"index":0,"finish_reason":"tool_calls","message":{
                        "role":"assistant","content":null,
                        "tool_calls":[{"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{}"}}]
                    }}]
                }),
                cache_control: "chat",
                retry_after: None,
            },
            Reply::Json {
                status: StatusCode::OK,
                body: chat,
                cache_control: "chat",
                retry_after: None,
            },
        ],
    )
    .await;
    let mut chat_source = source("chat", &chat_server, "chat-key", &["chat-model"], 0);
    chat_source.source.wire_api = WireApi::ChatCompletions;
    let (gateway, events) = spawn_gateway(
        vec![chat_source],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    assert_eq!(models(&gateway, LOCAL_KEY).await, ["chat-model"]);
    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "chat-model",
            "messages": [{"role": "user", "content": "hello"}],
            "service_tier": "priority"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "translated");
    assert_eq!(body["usage"]["prompt_tokens"], 2);

    {
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests
            .iter()
            .all(|request| request.path == "/v1/chat/completions"));
        assert!(requests
            .iter()
            .all(|request| request.authorization.as_deref() == Some("Bearer chat-key")));
        assert_eq!(requests[0].body["messages"][0]["content"], "hello");
        assert_eq!(requests[0].body["service_tier"], "priority");
    }
    {
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events.iter().all(|event| event.success));
        assert!(events
            .iter()
            .all(|event| event.wire_api == WireApi::ChatCompletions));
    }

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "chat-model",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "What is shown?"},
                    {"type": "image_url", "image_url": {"url": "https://example.test/image.png"}}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": "chat-model", "input": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["object"], "response");
    assert_eq!(body["output"][0]["content"][0]["text"], "translated");

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "chat-model",
            "messages": [{"role": "user", "content": "hello"}],
            "tools": [{"type": "function", "function": {"name": "lookup", "parameters":{"type":"object"}}}],
            "tool_choice": {"type":"function","function":{"name":"lookup"}}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["choices"][0]["message"]["tool_calls"][0]["id"],
        "call_1"
    );

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "chat-model",
            "modalities": ["text", "audio"],
            "messages": [{"role": "user", "content": "hello"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "chat_feature_not_supported");

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "chat-model",
            "messages": [
                {"role":"user","content":"hello"},
                {"role":"assistant","content":null,"tool_calls":[
                    {"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "result"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "translated");
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests[1].body["messages"][0]["content"][1]["type"],
        "image_url"
    );
    assert_eq!(requests[3].body["tools"][0]["function"]["name"], "lookup");
    assert_eq!(
        requests[3].body["tool_choice"]["function"]["name"],
        "lookup"
    );
    assert_eq!(
        requests[4].body["messages"][1]["tool_calls"][0]["id"],
        "call_1"
    );
    assert_eq!(requests[4].body["messages"][2]["tool_call_id"], "call_1");
    assert_eq!(requests[4].body["messages"][2]["content"], "result");
}

#[tokio::test]
async fn chat_completions_forwards_reasoning_without_admission_gate() {
    let (chat_server, state) = spawn_upstream("chat-key", Vec::new()).await;
    let mut chat_source = source("chat", &chat_server, "chat-key", &["chat-model"], 0);
    chat_source.source.wire_api = WireApi::ChatCompletions;
    let mut options = GatewayRuntimeOptions::default();
    options
        .model_reasoning_allowed_levels
        .insert("chat-model".to_string(), vec!["high".to_string()]);
    let (gateway, _) = spawn_gateway_with_options(
        vec![chat_source],
        vec![local_key("key", LOCAL_KEY, None)],
        options,
    )
    .await;
    let client = reqwest::Client::new();

    let low = client
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "chat-model",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "low"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(low.status(), StatusCode::OK);

    let high = client
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "chat-model",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "high"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(high.status(), StatusCode::OK);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].body["reasoning_effort"], "low");
    assert_eq!(requests[1].body["reasoning_effort"], "high");
}

#[tokio::test]
async fn messages_passthrough_preserves_native_tool_use_headers_and_sse() {
    let native_message = json!({
        "id": "msg_01",
        "type": "message",
        "role": "assistant",
        "model": MODEL,
        "content": [
            {"type": "text", "text": "I will continue."},
            {
                "type": "tool_use",
                "id": "toolu_2",
                "name": "PowerShell",
                "input": {"command": "pwd"}
            }
        ],
        "stop_reason": "tool_use",
        "stop_sequence": null,
        "usage": {"input_tokens": 12, "output_tokens": 7}
    });
    let native_sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_02\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-p2\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_3\",\"name\":\"PowerShell\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":7}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );
    let (messages_server, state) = spawn_upstream(
        "messages-key",
        vec![
            Reply::Json {
                status: StatusCode::OK,
                body: native_message.clone(),
                cache_control: "messages",
                retry_after: None,
            },
            Reply::Stream {
                chunks: vec![StreamChunk::Data(native_sse)],
                cache_control: "messages",
            },
            Reply::Json {
                status: StatusCode::OK,
                body: native_message.clone(),
                cache_control: "messages",
                retry_after: None,
            },
        ],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![source_with_protocol(
            "messages",
            &messages_server,
            "messages-key",
            &[MODEL],
            0,
            WireApi::Messages,
        )],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;
    let request = json!({
        "model": MODEL,
        "max_tokens": 1024,
        "system": "Use the supplied tools.",
        "messages": [
            {"role": "user", "content": "Inspect the workspace."},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "PowerShell", "input": {"command": "ls"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "Cargo.toml"},
                {"type": "text", "text": "Continue."}
            ]}
        ],
        "tools": [{"name": "PowerShell", "input_schema": {"type": "object"}}]
    });

    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.base_url))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "authentication_error");

    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.base_url))
        .header("x-api-key", LOCAL_KEY)
        .header("anthropic-beta", "fine-grained-tool-streaming-2025-05-14")
        .header("x-claude-code-session-id", "claude-session-1")
        .header("user-agent", "claude-code-test")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body, native_message);

    let mut streaming_request = request.clone();
    streaming_request["stream"] = Value::Bool(true);
    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.base_url))
        .header("x-api-key", LOCAL_KEY)
        .header("anthropic-beta", "fine-grained-tool-streaming-2025-05-14")
        .header("x-claude-code-session-id", "claude-session-1")
        .header("user-agent", "claude-code-test")
        .json(&streaming_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "text/event-stream");
    let stream = response.text().await.unwrap();
    assert_eq!(stream, native_sse);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "messages": [{"role": "user", "content": "hello"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "I will continue.");
    assert_eq!(
        body["choices"][0]["message"]["tool_calls"][0]["id"],
        "toolu_2"
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests
        .iter()
        .all(|request| request.path == "/v1/messages"));
    assert!(requests
        .iter()
        .all(|request| request.authorization.is_none()));
    assert!(requests
        .iter()
        .all(|request| request.x_api_key.as_deref() == Some("messages-key")));
    assert!(requests[..2].iter().all(|request| {
        request.anthropic_version.as_deref() == Some("2023-06-01")
            && request.anthropic_beta.as_deref() == Some("fine-grained-tool-streaming-2025-05-14")
            && request.claude_code_session_id.as_deref() == Some("claude-session-1")
    }));
    assert_eq!(requests[0].body, request);
    assert_eq!(requests[1].body, streaming_request);
    assert_eq!(
        requests[2].body["messages"][0]["content"][0]["text"],
        "hello"
    );
    assert_eq!(requests[2].anthropic_version.as_deref(), Some("2023-06-01"));
    assert!(requests[2].anthropic_beta.is_none());
    assert!(requests[2]
        .claude_code_session_id
        .as_deref()
        .is_some_and(|session_id| !session_id.is_empty()));
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(events.iter().all(|event| event.success));
    assert!(events[..2]
        .iter()
        .all(|event| event.wire_api == WireApi::Messages));
    assert_eq!(events[2].wire_api, WireApi::ChatCompletions);
    assert!(events[..2].iter().all(|event| {
        event.tool_use.client_tool_count == 1 && event.tool_use.forwarded_tool_count == 1
    }));
}

#[tokio::test]
async fn legacy_protocol_bindings_do_not_filter_automatic_routes() {
    let (upstream, state) = spawn_upstream("source-key", Vec::new()).await;
    let mut mixed = source(
        "mixed",
        &upstream,
        "source-key",
        &["gpt-5.4", "gpt-5.4-mini", "shared-model"],
        0,
    );
    mixed.protocol_bindings = vec![
        SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gpt-5.4".into(), "shared-model".into()],
        },
        SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gpt-5.4-mini".into(), "shared-model".into()],
        },
    ];
    let (gateway, events) =
        spawn_gateway(vec![mixed], vec![local_key("key", LOCAL_KEY, None)], 3).await;

    assert_eq!(
        models(&gateway, LOCAL_KEY).await,
        ["gpt-5.4", "gpt-5.4-mini", "shared-model"]
    );

    let catalog: Value = reqwest::Client::new()
        .get(format!(
            "{}/v1/models?client_version=1.97.0",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let catalog_models = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model["slug"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    assert_eq!(catalog_models.len(), 3);
    assert!(catalog_models.contains(&"gpt-5.4".to_string()));
    assert!(catalog_models.contains(&"gpt-5.4-mini".to_string()));
    assert!(catalog_models.contains(&zenith_relay_core::codex_model_alias("shared-model")));

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": "shared-model", "input": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.base_url))
        .header("x-api-key", LOCAL_KEY)
        .json(&json!({
            "model": "shared-model",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "hello"}]
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let response_body = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "body={response_body}");

    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.base_url))
        .header("x-api-key", LOCAL_KEY)
        .json(&json!({
            "model": "gpt-5.4-mini",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "use native messages"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.base_url))
        .header("x-api-key", LOCAL_KEY)
        .json(&json!({
            "model": "gpt-5.4",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "route automatically"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let requests = state.requests.lock().unwrap();
    let paths = requests
        .iter()
        .filter(|request| request.path != "/v1/models")
        .map(|request| request.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        [
            "/v1/responses",
            "/v1/messages",
            "/v1/messages",
            "/v1/responses"
        ]
    );
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 4);
    assert!(events.iter().any(|event| {
        event.wire_api == WireApi::Responses && event.candidate_id.as_deref() == Some("mixed")
    }));
    assert!(events.iter().any(|event| {
        event.wire_api == WireApi::Messages
            && event.candidate_id.as_deref() == Some("mixed::messages_to_responses")
    }));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.wire_api == WireApi::Messages)
            .count(),
        3
    );
}

#[tokio::test]
async fn native_gpt_picker_ids_and_legacy_aliases_keep_key_scope_and_upstream_identity() {
    let (upstream, state) = spawn_upstream("source-key", Vec::new()).await;
    let mut key = local_key("key", LOCAL_KEY, None);
    key.model_prefix = Some("local".into());
    key.allowed_models = vec!["gpt-6-astra".into()];
    let (gateway, _) = spawn_gateway(
        vec![source(
            "api",
            &upstream,
            "source-key",
            &["gpt-6-astra", "gpt-5.6-sol"],
            0,
        )],
        vec![key],
        3,
    )
    .await;
    let client = reqwest::Client::new();
    let catalog: Value = client
        .get(format!(
            "{}/v1/models?client_version=1.97.0",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = catalog["models"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["slug"], "local/gpt-6-astra");
    assert_eq!(rows[0]["display_name"], "6 Astra");
    for (model, expected) in [
        ("local/gpt-6-astra".to_string(), StatusCode::OK),
        (
            zenith_relay_core::codex_model_alias("local/gpt-6-astra"),
            StatusCode::OK,
        ),
        ("local/gpt-5.6-sol".to_string(), StatusCode::NOT_FOUND),
        (
            zenith_relay_core::codex_model_alias("local/gpt-5.6-sol"),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let response = client
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model": model, "input": "synthetic"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "model: {model}");
    }
    let observed = state.requests.lock().unwrap();
    let requests = observed
        .iter()
        .filter(|request| request.path == "/v1/responses")
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .all(|request| request.body["model"] == "gpt-6-astra"));
}

#[tokio::test]
async fn legacy_single_protocol_source_keeps_its_physical_candidate_id() {
    let (upstream, _) = spawn_upstream("source-key", Vec::new()).await;
    let (gateway, events) = spawn_gateway(
        vec![source(
            "legacy-source",
            &upstream,
            "source-key",
            &[MODEL],
            0,
        )],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].candidate_id.as_deref(), Some("legacy-source"));
    assert_eq!(events[0].wire_api, WireApi::Responses);
}

#[tokio::test]
async fn round_robin_ignores_session_affinity_between_independent_requests() {
    let (source_a, state_a) =
        spawn_upstream("a-key", vec![response_reply("a-first", "a-ready")]).await;
    let (source_b, state_b) = spawn_upstream("b-key", vec![response_reply("b-first", "b")]).await;
    let (gateway, _) = spawn_gateway_with_options(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        GatewayRuntimeOptions {
            tool_policy: Default::default(),
            model_metadata_catalog: None,
            max_retry_candidates: 3,
            pool_routing: Some(zenith_relay_core::PoolRoutingPolicy {
                mode: zenith_relay_core::PoolRoutingMode::RoundRobin,
                ..Default::default()
            }),
            hidden_models: Vec::new(),
            default_service_tier: Default::default(),
            quota_stale_after_ms: zenith_relay_core::QUOTA_STALE_AFTER_MS,
            image_base_model: None,
            image_pricing_catalog: None,
            model_reasoning_allowed_levels: Default::default(),
            response_affinity_store: None,
        },
    )
    .await;

    assert_eq!(
        request_with_session(&gateway, "session-1")
            .await
            .json::<Value>()
            .await
            .unwrap()["id"],
        "a-first"
    );
    assert_eq!(
        request_with_session(&gateway, "session-1")
            .await
            .json::<Value>()
            .await
            .unwrap()["id"],
        "b-first"
    );
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert_eq!(state_b.requests.lock().unwrap().len(), 1);
}
