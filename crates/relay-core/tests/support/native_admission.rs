use super::*;
use zenith_relay_core::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};
use zenith_relay_core::ProtocolFeature;

#[tokio::test]
async fn reasoning_checks_exact_capabilities_and_preserves_supported_native_extensions() {
    for (client, upstream_wire_api) in WireApi::ALL
        .map(|protocol| (protocol, protocol))
        .into_iter()
        .chain([(WireApi::Responses, WireApi::Messages)])
    {
        for (reasoning_status, declared_effort) in [
            (CapabilityStatus::Unsupported, None),
            (CapabilityStatus::Declared, Some("low")),
            (CapabilityStatus::Declared, Some("high")),
        ] {
            for reference_support in [false, true] {
                let observed = Arc::new(Mutex::new(Vec::new()));
                let captured = observed.clone();
                let response_body = match upstream_wire_api {
                    WireApi::GeminiInteractions => {
                        json!({"id":"interaction_synthetic","status":"completed","outputs":[{"type":"text","text":"synthetic response"}]})
                    }
                    WireApi::Responses => json!({
                        "id":"resp_synthetic",
                        "status":"completed",
                        "output":[],
                        "model":"synthetic-model",
                        "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}
                    }),
                    WireApi::ChatCompletions => json!({
                        "id":"chatcmpl_synthetic",
                        "object":"chat.completion",
                        "model":"synthetic-model",
                        "choices":[{"index":0,"message":{"role":"assistant","content":"synthetic response"},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}
                    }),
                    WireApi::Messages => json!({
                        "id":"msg_synthetic",
                        "type":"message",
                        "role":"assistant",
                        "content":[{"type":"text","text":"synthetic response"}],
                        "stop_reason":"end_turn",
                        "usage":{"input_tokens":3,"output_tokens":2}
                    }),
                    WireApi::Gemini => json!({
                        "responseId":"gemini_synthetic",
                        "candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"synthetic response"}]},"finishReason":"STOP"}],
                        "usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2,"totalTokenCount":5}
                    }),
                };
                let upstream =
                    spawn(Router::new().fallback(post(move |Json(body): Json<Value>| {
                        captured.lock().unwrap().push(body);
                        async move { Json(response_body) }
                    })))
                    .await;
                let mut source = RuntimeSource::unrestricted(ProviderSource {
                    id: "synthetic-source".into(),
                    name: "Synthetic source".into(),
                    base_url: format!("{}/v1", upstream.base_url),
                    api_key: SOURCE_KEY.into(),
                    proxy_url: None,
                    wire_api: upstream_wire_api,
                    models: vec!["synthetic-model".into()],
                });
                source.protocol_config.endpoint_hint = Some(upstream_wire_api);
                source.protocol_config.capabilities = vec![ModelEndpointCapability {
                    model_id: "synthetic-model".into(),
                    upstream_wire_api,
                    status: CapabilityStatus::Unknown,
                    origin: CapabilityOrigin::Catalog,
                    checked_at_ms: 1,
                    features: [(ProtocolFeature::Reasoning, reasoning_status)].into(),
                    reasoning_efforts: declared_effort.into_iter().map(str::to_owned).collect(),
                    service_tiers: None,
                }];
                let runtime = GatewayRuntime::from_pool(
                vec![source],
                vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                    id: "synthetic-key".into(),
                    secret: LOCAL_KEY.into(),
                })],
                GatewayRuntimeOptions {
                    model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(
                        ModelMetadataCatalog::from_models_dev_json(&json!({
                            "vendor/synthetic-model": {"reasoning":reference_support,"reasoning_effort_levels":["low","high"]}
                        }).to_string()).unwrap()
                    )),
                    ..GatewayRuntimeOptions::default()
                },
                Arc::new(|_| {}),
            )
            .unwrap();
                let gateway = spawn(gateway::router(Arc::new(runtime))).await;
                let (path, body, field) = native_request(client);
                let response = reqwest::Client::new()
                    .post(format!("{}{path}", gateway.base_url))
                    .bearer_auth(LOCAL_KEY)
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                let supports_request = reference_support
                    && reasoning_status != CapabilityStatus::Unsupported
                    && (client == WireApi::Messages || declared_effort == Some("high"));
                if !supports_request {
                    assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{client:?}");
                    assert!(observed.lock().unwrap().is_empty());
                } else if client == upstream_wire_api {
                    assert_eq!(response.status(), StatusCode::OK, "{client:?}");
                    assert_eq!(
                        observed.lock().unwrap()[0][field],
                        body[field],
                        "{client:?}"
                    );
                } else {
                    assert_eq!(response.status(), StatusCode::OK);
                    assert_eq!(observed.lock().unwrap().len(), 1);
                }
            }
        }
    }
}

fn native_request(protocol: WireApi) -> (&'static str, Value, &'static str) {
    match protocol {
        WireApi::GeminiInteractions => (
            "/v1beta/interactions",
            json!({"model":"synthetic-model","input":"synthetic input","generation_config":{"thinking_level":"high"}}),
            "generation_config",
        ),
        WireApi::Responses => (
            "/v1/responses",
            json!({"model":"synthetic-model", "input":"synthetic input",
                "reasoning":{"effort":"high", "summary":"auto"}}),
            "reasoning",
        ),
        WireApi::ChatCompletions => (
            "/v1/chat/completions",
            json!({"model":"synthetic-model", "messages":[{"role":"user","content":"synthetic input"}],
                "reasoning_effort":"high"}),
            "reasoning_effort",
        ),
        WireApi::Messages => (
            "/v1/messages",
            json!({"model":"synthetic-model", "max_tokens":2048,
                "messages":[{"role":"user","content":"synthetic input"}],
                "thinking":{"type":"enabled","budget_tokens":1024}}),
            "thinking",
        ),
        WireApi::Gemini => (
            "/v1beta/models/synthetic-model:generateContent",
            json!({"contents":[{"role":"user","parts":[{"text":"synthetic input"}]}],
                "generationConfig":{"thinkingConfig":{"thinkingLevel":"HIGH","includeThoughts":true}}}),
            "generationConfig",
        ),
    }
}
