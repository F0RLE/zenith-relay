use super::*;

fn runtime(reference: Value) -> GatewayRuntime {
    runtime_with_features(
        reference,
        [
            json!({"images":"unsupported","function_tools":"unsupported"}),
            json!({"images":"declared","function_tools":"declared"}),
        ],
    )
}

pub(super) fn runtime_with_features(reference: Value, features: [Value; 2]) -> GatewayRuntime {
    runtime_with_endpoints(
        reference,
        features.map(|features| json!({"features":features})),
    )
}

pub(super) fn runtime_with_endpoints(reference: Value, endpoints: [Value; 2]) -> GatewayRuntime {
    let sources = ["denied", "allowed"]
        .into_iter()
        .zip(endpoints)
        .map(|(id, fields)| {
            let mut source = crate::RuntimeSource::unrestricted(crate::ProviderSource {
                id: id.into(),
                name: id.into(),
                base_url: "https://example.invalid/v1/responses".into(),
                api_key: "synthetic-key".into(),
                proxy_url: None,
                wire_api: WireApi::Responses,
                models: vec!["gpt-test".into()],
            });
            let mut endpoint = json!({
                "modelId":"gpt-test","upstreamWireApi":"responses","status":"declared",
                "origin":"catalog","checkedAtMs":1
            });
            endpoint
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            source.protocol_config.capabilities = vec![serde_json::from_value(endpoint).unwrap()];
            source
        })
        .collect();
    let keys = [
        None,
        Some(vec!["denied".into()]),
        Some(vec!["allowed".into()]),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, source_ids)| {
        let mut key = crate::RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: format!("key-{index}"),
            secret: format!("synthetic-{index}"),
        });
        key.source_ids = source_ids;
        key.model_prefix = Some("tenant".into());
        key
    })
    .collect();
    let catalog = crate::model_metadata::ModelMetadataCatalog::from_models_dev_json(
        &json!({"openai/gpt-test":reference}).to_string(),
    )
    .unwrap();
    GatewayRuntime::from_pool(
        sources,
        keys,
        GatewayRuntimeOptions {
            model_metadata_catalog: Some(crate::model_metadata::ModelMetadataCatalogHandle::new(
                catalog,
            )),
            ..Default::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap()
}

fn entry(runtime: &GatewayRuntime, key: usize) -> Value {
    let key = runtime
        .authenticate_secret(&format!("synthetic-{key}"))
        .unwrap();
    let response =
        build_codex_models_response(runtime, &key, &["tenant/gpt-test".into()], None).unwrap();
    response["models"][0].clone()
}

#[test]
fn codex_media_and_parallel_flags_respect_key_scoped_alternatives() {
    let runtime = runtime(json!({
        "tool_call":true,"parallel_tool_call":true,"attachment":true,
        "modalities":{"input":["text","image","audio"],"output":["text"]}
    }));
    for (key, allowed) in [(0, true), (1, false), (2, true)] {
        let model = entry(&runtime, key);
        let modalities = model["input_modalities"].as_array().unwrap();
        assert_eq!(modalities.contains(&json!("image")), allowed);
        assert!(modalities.contains(&json!("text")));
        assert!(modalities.contains(&json!("audio")));
        assert_eq!(model["supports_parallel_tool_calls"], allowed);
    }
    assert_eq!(
        runtime.model_capabilities("gpt-test").input_modalities,
        ["text", "image", "audio"]
    );
    assert_eq!(
        runtime.model_capabilities("gpt-test").parallel_tool_call,
        Some(true)
    );
}

#[test]
fn positive_endpoint_claims_do_not_revive_reference_exclusions() {
    let runtime = runtime(json!({
        "tool_call":false,"parallel_tool_call":true,"attachment":false,
        "modalities":{"input":["text","image"],"output":["text"]}
    }));
    let model = entry(&runtime, 2);
    assert_eq!(model["input_modalities"], json!(["text"]));
    assert_eq!(model["supports_parallel_tool_calls"], false);
}

#[test]
fn tool_support_alone_does_not_establish_parallel_call_support() {
    let runtime = runtime(json!({"tool_call":true,"attachment":true,
        "modalities":{"input":["text","image"],"output":["text"]}}));
    let model = entry(&runtime, 2);
    assert_eq!(model["input_modalities"], json!(["text", "image"]));
    assert_eq!(model["supports_parallel_tool_calls"], false);
}

#[test]
fn codex_image_and_tool_exclusions_are_independent() {
    for (features, images, parallel) in [
        (
            json!({"images":"unsupported","function_tools":"declared"}),
            false,
            true,
        ),
        (
            json!({"images":"declared","function_tools":"unsupported"}),
            true,
            false,
        ),
    ] {
        let runtime = runtime_with_features(
            json!({
                "tool_call":true,"parallel_tool_call":true,"attachment":true,
                "modalities":{"input":["text","image"],"output":["text"]}
            }),
            [features, json!({})],
        );
        let model = entry(&runtime, 1);
        assert_eq!(
            model["input_modalities"]
                .as_array()
                .unwrap()
                .contains(&json!("image")),
            images
        );
        assert_eq!(model["supports_parallel_tool_calls"], parallel);
    }
}

#[test]
fn unknown_alternatives_preserve_reference_support() {
    let runtime = runtime_with_features(
        json!({
            "tool_call":true,"parallel_tool_call":true,"attachment":true,
            "modalities":{"input":["text","image"],"output":["text"]}
        }),
        [
            json!({"images":"unsupported","function_tools":"unsupported"}),
            json!({}),
        ],
    );
    for (key, supported) in [(0, true), (1, false), (2, true)] {
        let model = entry(&runtime, key);
        assert_eq!(
            model["input_modalities"]
                .as_array()
                .unwrap()
                .contains(&json!("image")),
            supported
        );
        assert_eq!(model["supports_parallel_tool_calls"], supported);
    }
}

#[test]
fn endpoint_claims_do_not_add_unadvertised_image_or_parallel_support() {
    let runtime = runtime(json!({
        "tool_call":true,"parallel_tool_call":false,"attachment":true,
        "modalities":{"input":["text"],"output":["text"]}
    }));
    let model = entry(&runtime, 2);
    assert_eq!(model["input_modalities"], json!(["text"]));
    assert_eq!(model["supports_parallel_tool_calls"], false);
}
