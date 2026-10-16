use super::*;

fn native_catalog() -> crate::model_metadata::ModelMetadataCatalog {
    crate::model_metadata::ModelMetadataCatalog::from_models_dev_json(
        r#"{
            "openai/sol-test": {"name":"Sol Test","family":"gpt"},
            "anthropic/sonnet-test": {"name":"Sonnet Test","family":"claude"},
            "google/flash-test": {"name":"Flash Test","family":"gemini"},
            "xai/grok-test": {"name":"Grok Test","family":"grok"},
            "zhipuai/glm-test": {"name":"GLM Test","family":"glm"}
        }"#,
    )
    .unwrap()
}

fn native_runtime(source: RuntimeSource) -> GatewayRuntime {
    GatewayRuntime::from_pool(
        vec![source],
        vec![RuntimeLocalKey::unrestricted(key(
            "test-key",
            "synthetic-key",
        ))],
        GatewayRuntimeOptions {
            model_metadata_catalog: Some(crate::model_metadata::ModelMetadataCatalogHandle::new(
                native_catalog(),
            )),
            ..Default::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap()
}

#[test]
fn native_protocol_is_independent_of_the_harness_protocol() {
    let models = [
        "sol-test",
        "sonnet-test",
        "flash-test",
        "grok-test",
        "glm-test",
    ];
    let mut configured = RuntimeSource::unrestricted(source("native-source", "synthetic", &models));
    configured.source.base_url = "https://api.zenithmarket.dev/v1".into();
    let runtime = native_runtime(configured);

    for (model, expected) in [
        ("sol-test", WireApi::Responses),
        ("sonnet-test", WireApi::Messages),
        ("flash-test", WireApi::Gemini),
        ("grok-test", WireApi::ChatCompletions),
        ("glm-test", WireApi::ChatCompletions),
    ] {
        for client in WireApi::ALL {
            let routes = runtime.executor_routes(
                "native-source",
                model,
                &CandidateScope::default(),
                &[client],
                true,
            );
            assert_eq!(routes.len(), 1, "{model}: {client:?}");
            let route = &routes[0];
            assert_eq!(route.client_wire_api, client);
            assert_eq!(
                route.adapter.upstream_protocol(client).wire_api(),
                expected,
                "{model}: {client:?}"
            );
            // Broader candidate eligibility is not another client protocol.
            // Keep a single exact projection and stable binding identity.
            let allowed = std::iter::once(client)
                .chain(WireApi::ALL.into_iter().filter(|wire| *wire != client))
                .collect::<Vec<_>>();
            let expanded = runtime.executor_routes(
                "native-source",
                model,
                &CandidateScope::default(),
                &allowed,
                true,
            );
            assert_eq!(
                expanded.len(),
                1,
                "duplicate projection: {model}: {client:?}"
            );
            assert_eq!(expanded[0].binding_key(), route.binding_key());
            assert_eq!(expanded[0].upstream_url, route.upstream_url);
            let mut streaming = expanded[0].clone();
            runtime.set_route_streaming(&mut streaming, false);
            let non_streaming = runtime.executor_routes(
                "native-source",
                model,
                &CandidateScope::default(),
                &[client],
                false,
            );
            assert_eq!(streaming.upstream_url, non_streaming[0].upstream_url);
        }
    }
    assert_eq!(runtime.lock_scheduler().candidates().count(), 1);
}

#[test]
fn missing_native_protocol_cannot_fall_back_to_the_harness_protocol() {
    let configured = RuntimeSource::unrestricted(source(
        "responses-only",
        "synthetic",
        &["sonnet-test", "sol-test"],
    ));
    let runtime = native_runtime(configured);
    assert_eq!(
        runtime.visible_models_for_secret("synthetic-key", &WireApi::ALL, 1),
        ["sol-test"]
    );
    for client in WireApi::ALL {
        assert!(runtime
            .executor_routes(
                "responses-only",
                "sonnet-test",
                &CandidateScope::default(),
                &[client],
                false,
            )
            .is_empty());
    }
}

#[test]
fn explicit_native_exclusion_cannot_be_recreated_by_the_service_profile() {
    let mut configured = RuntimeSource::unrestricted(source(
        "native-source",
        "synthetic",
        &["sonnet-test", "sol-test"],
    ));
    configured.source.base_url = "https://api.zenithmarket.dev/v1".into();
    configured
        .protocol_config
        .capabilities
        .push(ModelEndpointCapability {
            model_id: "sonnet-test".into(),
            upstream_wire_api: WireApi::Messages,
            status: CapabilityStatus::Unsupported,
            origin: CapabilityOrigin::Manual,
            checked_at_ms: 1,
            features: BTreeMap::new(),
            reasoning_efforts: Vec::new(),
            service_tiers: None,
        });
    let runtime = native_runtime(configured);
    assert_eq!(
        runtime.visible_models_for_secret("synthetic-key", &WireApi::ALL, 1),
        ["sol-test"]
    );
    assert!(runtime
        .executor_routes(
            "native-source",
            "sonnet-test",
            &CandidateScope::default(),
            &[WireApi::Responses],
            false,
        )
        .is_empty());
}
