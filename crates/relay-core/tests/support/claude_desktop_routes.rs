use super::*;

fn desktop_request(alias: &str, history: &[Value], stream: bool) -> Value {
    json!({
        "model":alias, "max_tokens":128, "stream":stream,
        "metadata":{"user_id":"synthetic-user"},
        "cache_control":{"type":"ephemeral"},
        "system":[{"type":"text","text":"Use the supplied tools.","cache_control":{"type":"ephemeral","ttl":"1h"}}],
        "messages":history,
        "tools":[{"name":"lookup","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]},"cache_control":{"type":"ephemeral"}}]
    })
}

fn desktop_reply(protocol: WireApi, id: &str, text: &str, tool: bool) -> Reply {
    let body = if protocol == WireApi::Messages {
        json!({
            "id": id, "type": "message", "role": "assistant", "model": MODEL,
            "content": if tool {
                json!([{"type":"tool_use","id":"toolu_lookup","name":"lookup","input":{"path":"synthetic"}}])
            } else {
                json!([{"type":"text","text":text}])
            },
            "stop_reason": if tool { "tool_use" } else { "end_turn" },
            "stop_sequence": null,
            "usage": {"input_tokens": 2, "output_tokens": 3}
        })
    } else {
        json!({
            "id":id, "object":"response", "status":"completed", "model":MODEL,
            "output": if tool {
                json!([{"type":"function_call","id":"fc_lookup","call_id":"toolu_lookup","name":"lookup","arguments":"{\"path\":\"synthetic\"}","status":"completed"}])
            } else {
                json!([{"type":"message","id":"message_synthetic","role":"assistant","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]}])
            },
            "usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}
        })
    };
    Reply::Json {
        status: StatusCode::OK,
        body,
        cache_control: "synthetic",
        retry_after: None,
    }
}

async fn desktop_tool_cycle(protocol: WireApi) {
    let (upstream, state) = spawn_upstream(
        "synthetic-key",
        vec![
            desktop_reply(protocol, "text", "Ready.", false),
            desktop_reply(protocol, "tool", "", true),
            desktop_reply(protocol, "final", "Finished.", false),
        ],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![source_with_protocol(
            "api",
            &upstream,
            "synthetic-key",
            &[MODEL],
            0,
            protocol,
        )],
        vec![local_key("key", LOCAL_KEY, None)],
        1,
    )
    .await;
    let alias = zenith_relay_core::claude_desktop_reasoning_model_alias(MODEL);
    let client = reqwest::Client::new();
    let mut history = vec![
        json!({"role":"user","content":[{"type":"text","text":"Hello.","cache_control":{"type":"ephemeral"}}]}),
    ];
    for turn in 0..3 {
        let response = client
            .post(format!("{}/v1/messages", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&desktop_request(&alias, &history, false))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{protocol:?} turn {turn}: {text}");
        let body: Value = serde_json::from_str(&text).unwrap();
        let content = body["content"].as_array().unwrap();
        if turn == 1 {
            let tool = content
                .iter()
                .find(|block| block["type"] == "tool_use")
                .unwrap();
            assert_eq!(tool["id"], "toolu_lookup");
            assert_eq!(tool["name"], "lookup");
            assert_eq!(tool["input"], json!({"path":"synthetic"}));
            assert_eq!(body["stop_reason"], "tool_use");
        } else {
            let expected = if turn == 0 { "Ready." } else { "Finished." };
            assert!(content.iter().any(|block| block["text"] == expected));
            assert_eq!(body["stop_reason"], "end_turn");
        }
        history.push(json!({"role":"assistant","content":content}));
        history.push(if turn == 0 {
            json!({"role":"user","content":"Look up the synthetic path."})
        } else {
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_lookup","content":"synthetic result","cache_control":{"type":"ephemeral"}}]})
        });
    }
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let expected_path = if protocol == WireApi::Messages {
        "/v1/messages"
    } else {
        "/v1/responses"
    };
    assert!(requests.iter().all(|request| request.path == expected_path));
    assert!(
        requests
            .iter()
            .all(|request| request.body["model"] == MODEL)
    );
    assert!(
        requests
            .iter()
            .all(|request| request.body["metadata"]["user_id"] == "synthetic-user")
    );
    assert!(
        requests
            .iter()
            .all(|request| request.body.get("cache_control").is_some()
                == (protocol == WireApi::Messages))
    );
    if protocol == WireApi::Responses {
        assert!(
            requests
                .iter()
                .all(|request| !request.body.to_string().contains("cache_control"))
        );
    }
    if protocol == WireApi::Messages {
        assert_eq!(
            requests[2].body["messages"][4]["content"][0]["tool_use_id"],
            "toolu_lookup"
        );
        assert_eq!(
            requests[2].body["messages"][4]["content"][0]["content"],
            "synthetic result"
        );
    } else {
        let output = requests[2].body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "function_call_output")
            .unwrap();
        assert_eq!(output["call_id"], "toolu_lookup");
        assert_eq!(output["output"], "synthetic result");
    }
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(events.iter().all(|event| event.success
        && event.requested_model.as_deref() == Some(alias.as_str())
        && event.resolved_model.as_deref() == Some(MODEL)));
}

#[tokio::test]
async fn claude_desktop_native_messages_keeps_real_model_through_tool_roundtrip() {
    desktop_tool_cycle(WireApi::Messages).await;
}

#[tokio::test]
async fn claude_desktop_bridged_responses_keeps_real_model_through_tool_roundtrip() {
    desktop_tool_cycle(WireApi::Responses).await;
}

#[tokio::test]
async fn claude_desktop_effort_choices_reach_native_and_bridged_upstreams() {
    use zenith_relay_core::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};

    for protocol in [WireApi::Messages, WireApi::Responses] {
        let efforts = ["low", "medium", "high", "xhigh", "max"];
        let (upstream, state) = spawn_upstream(
            "synthetic-key",
            efforts
                .iter()
                .map(|_| desktop_reply(protocol, "selected", "Ready.", false))
                .collect(),
        )
        .await;
        let (gateway, events) = spawn_gateway_with_options(
            vec![source_with_protocol(
                "api",
                &upstream,
                "synthetic-key",
                &[MODEL],
                0,
                protocol,
            )],
            vec![local_key("key", LOCAL_KEY, None)],
            GatewayRuntimeOptions {
                model_reasoning_allowed_levels: std::collections::BTreeMap::from([(
                    MODEL.to_string(),
                    efforts.iter().map(|effort| (*effort).to_string()).collect(),
                )]),
                model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(
                    ModelMetadataCatalog::from_models_dev_json(
                        &json!({format!("vendor/{MODEL}"): {
                            "reasoning": true, "reasoning_effort_levels": efforts
                        }})
                        .to_string(),
                    )
                    .unwrap(),
                )),
                ..GatewayRuntimeOptions::default()
            },
        )
        .await;
        let alias = zenith_relay_core::claude_desktop_reasoning_model_alias(MODEL);
        let history = [json!({"role":"user","content":"Synthetic input."})];
        let client = reqwest::Client::new();
        for invalid_options in [
            json!({"output_config":{"effort":"ultra"}}),
            json!({"output_config":{"effort":"minimal"}}),
        ] {
            let mut body = desktop_request(&alias, &history, false);
            body.as_object_mut()
                .unwrap()
                .extend(invalid_options.as_object().unwrap().clone());
            let response = client
                .post(format!("{}/v1/messages", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "{protocol:?} {invalid_options}"
            );
        }
        assert!(state.requests.lock().unwrap().is_empty());
        for effort in efforts {
            let mut body = desktop_request(&alias, &history, false);
            body["output_config"] = json!({"effort":effort});
            body["thinking"] = json!({"type":"adaptive"});
            let response = client
                .post(format!("{}/v1/messages", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let body = response.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{protocol:?} {effort}: {body}");
        }
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), efforts.len());
        for (request, selected) in requests.iter().zip(efforts) {
            assert_eq!(request.body["model"], MODEL);
            let effort = if protocol == WireApi::Messages {
                request.body.pointer("/output_config/effort")
            } else {
                request.body.pointer("/reasoning/effort")
            };
            assert_eq!(effort, Some(&json!(selected)), "{protocol:?}");
        }
        let events = events.lock().unwrap();
        for selected in efforts {
            assert!(events.iter().any(|event| event.success
                && event.requested_model.as_deref() == Some(alias.as_str())
                && event.resolved_model.as_deref() == Some(MODEL)
                && event.effective_reasoning_effort.as_deref() == Some(selected)));
        }
    }
}

#[tokio::test]
async fn claude_desktop_sparse_efforts_reject_unadvertised_levels_without_remapping() {
    use zenith_relay_core::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};

    for protocol in [WireApi::Messages, WireApi::Responses] {
        for (supported, allowed, cases) in [
            (
                vec!["none", "low", "high", "max"],
                vec!["none", "low", "high", "max"],
                vec!["low", "high", "max"],
            ),
            (
                vec!["none", "low", "medium", "high", "xhigh", "max"],
                vec!["none", "high", "max"],
                vec!["high", "max"],
            ),
            (vec!["none"], vec!["none"], vec![]),
        ] {
            let (upstream, state) = spawn_upstream(
                "synthetic-key",
                (0..=cases.len())
                    .map(|_| desktop_reply(protocol, "selected", "Ready.", false))
                    .collect(),
            )
            .await;
            let (gateway, events) = spawn_gateway_with_options(
                vec![source_with_protocol(
                    "api",
                    &upstream,
                    "synthetic-key",
                    &[MODEL],
                    0,
                    protocol,
                )],
                vec![local_key("key", LOCAL_KEY, None)],
                GatewayRuntimeOptions {
                    model_reasoning_allowed_levels: std::collections::BTreeMap::from([(
                        MODEL.to_string(),
                        allowed.iter().map(|effort| (*effort).to_string()).collect(),
                    )]),
                    model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(
                        ModelMetadataCatalog::from_models_dev_json(
                            &json!({format!("vendor/{MODEL}"): {
                                "reasoning": true, "reasoning_effort_levels": supported
                            }})
                            .to_string(),
                        )
                        .unwrap(),
                    )),
                    ..GatewayRuntimeOptions::default()
                },
            )
            .await;
            let alias = zenith_relay_core::claude_desktop_reasoning_model_alias(MODEL);
            let history = [json!({"role":"user","content":"Synthetic input."})];
            let original = desktop_request(&alias, &history, false);
            let mut rejected = Vec::new();
            for effort in ["low", "medium", "high", "xhigh", "max"] {
                if !cases.contains(&effort) {
                    let mut body = original.clone();
                    body["output_config"] = json!({"effort":effort});
                    body["thinking"] = json!({"type":"adaptive"});
                    rejected.push(("/v1/messages".to_string(), body));
                }
            }
            for model in [
                MODEL.to_string(),
                zenith_relay_core::claude_desktop_model_alias(MODEL),
            ] {
                let mut body = original.clone();
                body["model"] = model.into();
                body["output_config"] = json!({"effort":"medium"});
                rejected.push(("/v1/messages".to_string(), body));
            }
            for thinking in [
                json!({"type":"disabled"}),
                json!({"type":"enabled","budget_tokens":8192}),
                json!({"type":"adaptive","budget_tokens":8192}),
            ] {
                let mut body = original.clone();
                body["output_config"] = json!({"effort":"medium"});
                body["thinking"] = thinking;
                rejected.push(("/v1/messages".to_string(), body));
            }
            for effort in ["minimal", "ultra", "unknown"] {
                let mut body = original.clone();
                body["output_config"] = json!({"effort":effort});
                body["thinking"] = json!({"type":"adaptive"});
                rejected.push(("/v1/messages".to_string(), body));
            }
            rejected.extend([
                ("/v1/responses".to_string(), json!({"model":alias,"input":"Synthetic input.","reasoning":{"effort":"medium"}})),
                ("/v1/chat/completions".to_string(), json!({"model":alias,"messages":history,"reasoning_effort":"medium"})),
                (format!("/v1beta/models/{alias}:generateContent"), json!({"contents":[{"role":"user","parts":[{"text":"Synthetic input."}]}],"generationConfig":{"thinkingConfig":{"thinkingLevel":"MEDIUM"}}})),
            ]);
            let rejected_count = rejected.len();
            for (path, body) in rejected {
                let response = reqwest::Client::new()
                    .post(format!("{}{path}", gateway.base_url))
                    .header("user-agent", "Claude Desktop")
                    .bearer_auth(LOCAL_KEY)
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(
                    response.status(),
                    StatusCode::BAD_REQUEST,
                    "{protocol:?} {path}: {body}"
                );
            }
            assert!(state.requests.lock().unwrap().is_empty());
            {
                let mut rejected_events = events.lock().unwrap();
                assert_eq!(rejected_events.len(), rejected_count);
                for event in rejected_events.iter() {
                    assert!(!event.success);
                    assert_eq!(event.http_status, 400);
                    assert_eq!(event.source_id, "relay");
                    assert!(event.error_category.is_some());
                    assert!(event.candidate_id.is_none());
                    assert!(event.account_id.is_none());
                    assert!(event.routing.is_none());
                    assert!(event.input_tokens.is_none());
                    assert!(event.output_tokens.is_none());
                    assert!(event.total_tokens.is_none());
                }
                rejected_events.clear();
            }
            let mut disabled = original.clone();
            disabled["thinking"] = json!({"type":"disabled"});
            let response = reqwest::Client::new()
                .post(format!("{}/v1/messages", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .json(&disabled)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let output = response.text().await.unwrap();
            assert_eq!(
                status,
                StatusCode::OK,
                "{protocol:?} disabled {allowed:?}: {output}"
            );
            for requested in &cases {
                let mut body = desktop_request(
                    &alias,
                    &[json!({"role":"user","content":"Synthetic input."})],
                    false,
                );
                body["output_config"] = json!({"effort":requested});
                body["thinking"] = json!({"type":"adaptive"});
                let response = reqwest::Client::new()
                    .post(format!("{}/v1/messages", gateway.base_url))
                    .bearer_auth(LOCAL_KEY)
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                let status = response.status();
                let output = response.text().await.unwrap();
                assert_eq!(status, StatusCode::OK, "{protocol:?} {requested}: {output}");
            }
            let requests = state.requests.lock().unwrap();
            assert_eq!(requests.len(), cases.len() + 1);
            if protocol == WireApi::Messages {
                assert_eq!(requests[0].body["thinking"]["type"], "disabled");
                assert!(requests[0].body.get("output_config").is_none());
            } else {
                assert_eq!(requests[0].body["reasoning"]["effort"], "none");
            }
            for (request, effort) in requests.iter().skip(1).zip(&cases) {
                assert_eq!(request.body["model"], MODEL);
                let pointer = if protocol == WireApi::Messages {
                    "/output_config/effort"
                } else {
                    "/reasoning/effort"
                };
                assert_eq!(request.body.pointer(pointer), Some(&json!(effort)));
            }
            let events = events.lock().unwrap();
            assert_eq!(events.len(), cases.len() + 1);
            assert_eq!(
                events[0].requested_reasoning_effort.as_deref(),
                Some("none")
            );
            assert_eq!(
                events[0].effective_reasoning_effort.as_deref(),
                Some("none")
            );
            for (event, effort) in events.iter().skip(1).zip(&cases) {
                assert!(event.success);
                assert_eq!(event.requested_reasoning_effort.as_deref(), Some(*effort));
                assert_eq!(event.effective_reasoning_effort.as_deref(), Some(*effort));
            }
        }
    }
}

#[tokio::test]
async fn claude_desktop_aliases_preserve_native_and_bridged_streaming() {
    const MESSAGES: &str = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_stream\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-p2\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"streamed\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
    );
    const RESPONSES: &str = concat!(
        "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_stream\",\"object\":\"response\",\"status\":\"in_progress\",\"model\":\"gpt-p2\",\"output\":[]}}\n\n",
        "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"msg_stream\",\"type\":\"message\",\"role\":\"assistant\",\"status\":\"in_progress\",\"content\":[]}}\n\n",
        "event: response.content_part.added\ndata: {\"type\":\"response.content_part.added\",\"item_id\":\"msg_stream\",\"output_index\":0,\"content_index\":0,\"part\":{\"type\":\"output_text\",\"text\":\"\",\"annotations\":[]}}\n\n",
        "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_stream\",\"output_index\":0,\"content_index\":0,\"delta\":\"streamed\"}\n\n",
        "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_stream\",\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-p2\",\"output\":[{\"id\":\"msg_stream\",\"type\":\"message\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"streamed\",\"annotations\":[]}]}],\"usage\":{\"input_tokens\":2,\"output_tokens\":3,\"total_tokens\":5}}}\n\n"
    );
    for (protocol, sse, reasoning) in [
        (WireApi::Messages, MESSAGES, false),
        (WireApi::Responses, RESPONSES, false),
        (WireApi::Messages, MESSAGES, true),
        (WireApi::Responses, RESPONSES, true),
    ] {
        let (upstream, state) = spawn_upstream(
            "synthetic-key",
            vec![Reply::Stream {
                chunks: vec![StreamChunk::Data(sse)],
                cache_control: "synthetic",
            }],
        )
        .await;
        let (gateway, events) = spawn_gateway_with_options(
            vec![source_with_protocol(
                "api",
                &upstream,
                "synthetic-key",
                &[MODEL],
                0,
                protocol,
            )],
            vec![local_key("key", LOCAL_KEY, None)],
            GatewayRuntimeOptions {
                model_reasoning_allowed_levels: std::collections::BTreeMap::from([(
                    MODEL.to_string(), vec!["low".into(), "high".into(), "max".into()],
                )]),
                model_metadata_catalog: Some(zenith_relay_core::model_metadata::ModelMetadataCatalogHandle::new(
                    zenith_relay_core::model_metadata::ModelMetadataCatalog::from_models_dev_json(
                        &json!({format!("vendor/{MODEL}"): {
                            "reasoning":true,"reasoning_effort_levels":["low","high","max"]
                        }}).to_string(),
                    ).unwrap(),
                )),
                ..GatewayRuntimeOptions::default()
            },
        )
        .await;
        let alias = if reasoning {
            zenith_relay_core::claude_desktop_reasoning_model_alias(MODEL)
        } else {
            zenith_relay_core::claude_desktop_model_alias(MODEL)
        };
        let mut body = desktop_request(
            &alias,
            &[
                json!({"role":"user","content":[{"type":"text","text":"Hello.","cache_control":{"type":"ephemeral"}}]}),
            ],
            true,
        );
        if reasoning {
            body["output_config"] = json!({"effort":"high"});
            body["thinking"] = json!({"type":"adaptive"});
        }
        let response = reqwest::Client::new()
            .post(format!("{}/v1/messages", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/event-stream");
        let output = response.text().await.unwrap();
        assert!(output.contains("streamed"), "{protocol:?}: {output}");
        assert_eq!(
            output.matches("event: message_stop\n").count(),
            1,
            "{protocol:?}: {output}"
        );
        if protocol == WireApi::Messages {
            assert_eq!(output, MESSAGES);
        }
        assert_eq!(state.requests.lock().unwrap()[0].body["model"], MODEL);
        assert_eq!(
            state.requests.lock().unwrap()[0].body["metadata"]["user_id"],
            "synthetic-user"
        );
        assert_eq!(
            state.requests.lock().unwrap()[0]
                .body
                .get("cache_control")
                .is_some(),
            protocol == WireApi::Messages
        );
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].success);
        assert_eq!(events[0].resolved_model.as_deref(), Some(MODEL));
        if reasoning {
            assert_eq!(
                events[0].requested_reasoning_effort.as_deref(),
                Some("high")
            );
            assert_eq!(
                events[0].effective_reasoning_effort.as_deref(),
                Some("high")
            );
        }
    }
}

#[tokio::test]
async fn claude_desktop_prefers_exact_effort_without_crossing_key_scope() {
    use zenith_relay_core::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};
    use zenith_relay_core::{
        CapabilityOrigin, CapabilityStatus, ModelEndpointCapability, ProtocolFeature,
    };

    let (lower, lower_state) = spawn_upstream(
        "synthetic-key",
        vec![desktop_reply(WireApi::Messages, "lower", "Ready.", false)],
    )
    .await;
    let (exact, exact_state) = spawn_upstream(
        "synthetic-key",
        vec![desktop_reply(WireApi::Messages, "exact", "Ready.", false)],
    )
    .await;
    let mut sources = vec![
        source_with_protocol(
            "lower",
            &lower,
            "synthetic-key",
            &[MODEL],
            0,
            WireApi::Messages,
        ),
        source_with_protocol(
            "exact",
            &exact,
            "synthetic-key",
            &[MODEL],
            1,
            WireApi::Messages,
        ),
    ];
    for (source, levels) in sources
        .iter_mut()
        .zip([vec!["low", "high", "max"], vec!["xhigh"]])
    {
        source.protocol_config.capabilities = vec![ModelEndpointCapability {
            model_id: MODEL.into(),
            upstream_wire_api: WireApi::Messages,
            status: CapabilityStatus::Declared,
            origin: CapabilityOrigin::Catalog,
            checked_at_ms: 1,
            features: [(ProtocolFeature::Reasoning, CapabilityStatus::Declared)].into(),
            reasoning_efforts: levels.into_iter().map(str::to_string).collect(),
            service_tiers: None,
        }];
    }
    let (gateway, events) = spawn_gateway_with_options(
        sources,
        vec![
            local_key("all", LOCAL_KEY, None),
            local_key("restricted", "restricted-local-key", Some(vec!["lower"])),
        ],
        GatewayRuntimeOptions {
            model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(
                ModelMetadataCatalog::from_models_dev_json(
                    &json!({format!("vendor/{MODEL}"): {
                        "reasoning":true,"reasoning_effort_levels":["low","high","xhigh","max"]
                    }})
                    .to_string(),
                )
                .unwrap(),
            )),
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;
    let alias = zenith_relay_core::claude_desktop_reasoning_model_alias(MODEL);
    let mut body = desktop_request(
        &alias,
        &[json!({"role":"user","content":"Synthetic input."})],
        false,
    );
    body["thinking"] = json!({"type":"adaptive"});
    body["output_config"] = json!({"effort":"xhigh"});
    for (secret, expected_status) in [
        (LOCAL_KEY, StatusCode::OK),
        ("restricted-local-key", StatusCode::BAD_REQUEST),
    ] {
        let response = reqwest::Client::new()
            .post(format!("{}/v1/messages", gateway.base_url))
            .bearer_auth(secret)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let output = response.text().await.unwrap();
        assert_eq!(status, expected_status, "{output}");
    }
    assert_eq!(exact_state.requests.lock().unwrap().len(), 1);
    assert_eq!(
        exact_state.requests.lock().unwrap()[0].body["output_config"]["effort"],
        "xhigh"
    );
    assert!(lower_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].requested_reasoning_effort.as_deref(),
        Some("xhigh")
    );
    assert_eq!(
        events[0].effective_reasoning_effort.as_deref(),
        Some("xhigh")
    );
    assert!(!events[1].success);
    assert_eq!(events[1].http_status, 400);
    assert_eq!(events[1].source_id, "relay");
    assert!(events[1].candidate_id.is_none());
    assert!(events[1].effective_reasoning_effort.is_none());
    // Pre-admission failures use the local-error event, not executed-attempt usage.
    assert!(events[1].requested_reasoning_effort.is_none());
}

#[tokio::test]
async fn claude_desktop_aliases_cannot_bypass_prefixed_key_model_restrictions() {
    let (upstream, state) = spawn_upstream("synthetic-key", Vec::new()).await;
    let mut key = local_key("key", LOCAL_KEY, None);
    key.model_prefix = Some("team".into());
    key.allowed_models = vec![MODEL.into()];
    let (gateway, _) = spawn_gateway(
        vec![source(
            "api",
            &upstream,
            "synthetic-key",
            &[MODEL, "gpt-denied"],
            0,
        )],
        vec![key],
        1,
    )
    .await;
    for (model, expected) in [
        (format!("team/{MODEL}"), StatusCode::OK),
        (MODEL.to_string(), StatusCode::NOT_FOUND),
        ("team/gpt-denied".to_string(), StatusCode::NOT_FOUND),
        ("team/unknown".to_string(), StatusCode::NOT_FOUND),
    ] {
        let response = reqwest::Client::new()
            .post(format!("{}/v1/messages", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":zenith_relay_core::claude_desktop_model_alias(&model),"max_tokens":128,"messages":[{"role":"user","content":"Hello."}]}))
            .send().await.unwrap();
        assert_eq!(response.status(), expected, "{model}");
    }
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["model"], MODEL);
}
