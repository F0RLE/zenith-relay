use super::*;
use zenith_relay_core::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};

#[tokio::test]
async fn parallel_permission_keeps_a_sequential_model_eligible_and_preserves_single_call_limits() {
    for (parallel, call_count, expected_status) in [
        (true, 1, StatusCode::OK),
        (false, 1, StatusCode::OK),
        (false, 2, StatusCode::BAD_GATEWAY),
    ] {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let captured = observed.clone();
        let parts = (0..call_count)
            .map(|index| {
                json!({"functionCall": {
                    "id":format!("synthetic-call-{index}"), "name":"lookup", "args":{}
                }})
            })
            .collect::<Vec<_>>();
        let response_body = json!({
            "responseId":"synthetic-response",
            "candidates":[{"content":{"role":"model","parts":parts},"finishReason":"STOP"}]
        });
        let upstream = spawn(Router::new().fallback(post(move |Json(body): Json<Value>| {
            captured.lock().unwrap().push(body);
            async move { Json(response_body) }
        })))
        .await;
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "synthetic-source".into(),
                name: "Synthetic source".into(),
                base_url: format!("{}/v1beta", upstream.base_url),
                api_key: SOURCE_KEY.into(),
                proxy_url: None,
                wire_api: WireApi::Gemini,
                models: vec!["sequential-tools".into()],
            })],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "synthetic-key".into(),
                secret: LOCAL_KEY.into(),
            })],
            GatewayRuntimeOptions {
                model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(
                    ModelMetadataCatalog::from_models_dev_json(
                        &json!({"vendor/sequential-tools": {
                            "tool_call":true,"parallel_tool_call":false,"reasoning":false
                        }})
                        .to_string(),
                    )
                    .unwrap(),
                )),
                ..GatewayRuntimeOptions::default()
            },
            Arc::new(|_| {}),
        )
        .unwrap();
        let gateway = spawn(gateway::router(Arc::new(runtime))).await;
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({
                "model":"sequential-tools", "input":"Look up a value",
                "parallel_tool_calls":parallel,
                "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            expected_status,
            "{parallel}: {call_count}"
        );
        let requests = observed.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].get("parallel_tool_calls").is_none());
        assert_eq!(
            requests[0]["tools"][0]["functionDeclarations"][0]["name"],
            "lookup"
        );
    }
}
