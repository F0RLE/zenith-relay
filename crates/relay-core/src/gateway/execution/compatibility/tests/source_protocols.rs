use super::super::*;
use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};
use crate::{
    CacheWriteTtl, CapabilityOrigin, GatewayRuntimeOptions, LocalGatewayKey, MessagesReasoningMode,
    ModelEndpointCapability, ProviderSource, RuntimeLocalKey, RuntimeSource, SourceAdapter,
};
use serde_json::json;
use std::sync::Arc;

fn source(declaration: Option<(CapabilityStatus, CapabilityOrigin)>) -> RuntimeSource {
    let mut source = RuntimeSource::unrestricted(ProviderSource {
        id: "synthetic-source".into(),
        name: "Synthetic source".into(),
        base_url: "https://api.zenithmarket.dev/v1".into(),
        api_key: "synthetic-upstream-key".into(),
        proxy_url: None,
        wire_api: WireApi::Responses,
        models: vec!["glm-test".into()],
    });
    if let Some((status, origin)) = declaration {
        source
            .protocol_config
            .capabilities
            .push(ModelEndpointCapability {
                model_id: "glm-test".into(),
                upstream_wire_api: WireApi::Responses,
                status,
                origin,
                checked_at_ms: 1,
                features: BTreeMap::new(),
                reasoning_efforts: Vec::new(),
                token_limits: Default::default(),
                service_tiers: None,
            });
    }
    source
}

fn runtime(declaration: Option<(CapabilityStatus, CapabilityOrigin)>) -> GatewayRuntime {
    runtime_with_source(source(declaration))
}

fn dual_protocol_source() -> RuntimeSource {
    let mut source = source(Some((
        CapabilityStatus::Declared,
        CapabilityOrigin::Catalog,
    )));
    let mut chat = source.protocol_config.capabilities[0].clone();
    chat.upstream_wire_api = WireApi::ChatCompletions;
    source.protocol_config.capabilities.push(chat);
    source
}

fn runtime_with_source(source: RuntimeSource) -> GatewayRuntime {
    runtime_with_sources(
        vec![source],
        r#"{"zhipuai/glm-test":{"name":"Synthetic model","family":"glm","reasoning":false}}"#,
    )
}

fn runtime_with_sources(sources: Vec<RuntimeSource>, metadata: &str) -> GatewayRuntime {
    let catalog = ModelMetadataCatalog::from_models_dev_json(metadata).unwrap();
    GatewayRuntime::from_pool(
        sources,
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "synthetic-key".into(),
            secret: "synthetic-secret".into(),
        })],
        GatewayRuntimeOptions {
            model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(catalog)),
            ..Default::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap()
}

#[test]
fn source_output_limits_filter_only_the_owning_route_before_execution() {
    let mut small = source(Some((
        CapabilityStatus::Declared,
        CapabilityOrigin::Catalog,
    )));
    small.protocol_config.capabilities[0].token_limits = crate::ModelTokenLimits {
        context: Some(1000),
        input: Some(900),
        output: Some(100),
    };
    let mut large = small.clone();
    large.source.id = "large-source".into();
    large.protocol_config.capabilities[0].token_limits = crate::ModelTokenLimits {
        context: Some(2000),
        input: Some(1800),
        output: Some(200),
    };
    let runtime = runtime_with_sources(
        vec![small, large],
        r#"{"zhipuai/glm-test":{"name":"Synthetic model","family":"glm","reasoning":false}}"#,
    );
    let key = runtime.authenticate_secret("synthetic-secret").unwrap();
    let projected = runtime.client_token_limits(&key, "glm-test", WireApi::Responses);
    assert_eq!(
        (projected.context, projected.input, projected.output),
        (Some(1000), Some(900), Some(100))
    );
    assert_eq!(runtime.model_capabilities("glm-test").output_limit, None);
    for stream in [false, true] {
        let request = json!({"model":"glm-test", "input":"Synthetic", "max_output_tokens":150});
        let original = request.clone();
        let policy = ServiceTierPolicy::pool_owned(&request);
        let (excluded, routes, _) = incompatible_routes(RouteCompatibility {
            runtime: &runtime,
            key: &key,
            model: "glm-test",
            client: WireApi::Responses,
            request: &request,
            stream,
            tier_policy: &policy,
            now_ms: 2,
        });
        assert_eq!(routes.len(), 1);
        assert_eq!(excluded.len(), 1);
        assert_eq!(routes.values().next().unwrap().candidate_id, "large-source");
        assert_eq!(request, original);
    }
}

#[test]
fn reasoning_conflicts_reject_controls_but_preserve_native_provider_defaults() {
    let mut source = source(Some((
        CapabilityStatus::Declared,
        CapabilityOrigin::Catalog,
    )));
    source.protocol_config.capabilities[0].reasoning_efforts = vec!["high".into()];
    let runtime = runtime_with_sources(
        vec![source],
        r#"{"zhipuai/glm-test":{"name":"Synthetic model","family":"glm","reasoning":true,"reasoning_effort_levels":["low"]}}"#,
    );
    let key = runtime.authenticate_secret("synthetic-secret").unwrap();
    for effort in ["none", "low", "high"] {
        let request = json!({"input":"Synthetic", "reasoning":{"effort":effort}});
        let policy = ServiceTierPolicy::pool_owned(&request);
        let (excluded, routes, error) = incompatible_routes(RouteCompatibility {
            runtime: &runtime,
            key: &key,
            model: "glm-test",
            client: WireApi::Responses,
            request: &request,
            stream: false,
            tier_policy: &policy,
            now_ms: 2,
        });
        assert!(routes.is_empty());
        assert!(!excluded.is_empty());
        assert_eq!(error.unwrap().code(), "adapter_metadata_conflict");
    }
    assert_eq!(
        admitted(&runtime, &json!({"input":"Synthetic"}), false).len(),
        1
    );
    assert!(
        runtime
            .client_reasoning_levels(&key, "glm-test", WireApi::Responses)
            .is_empty()
    );
}

#[test]
fn native_and_converted_output_limits_preserve_request_values() {
    let capabilities = crate::model_metadata::ModelCapabilities {
        output_limit: Some(100),
        ..Default::default()
    };
    for (wire, mut request, parameter) in [
        (
            WireApi::Responses,
            json!({"input":"Synthetic", "max_output_tokens":100}),
            "max_output_tokens",
        ),
        (
            WireApi::ChatCompletions,
            json!({"max_completion_tokens":100}),
            "max_completion_tokens",
        ),
        (
            WireApi::ChatCompletions,
            json!({"max_tokens":100}),
            "max_tokens",
        ),
        (WireApi::Messages, json!({"max_tokens":100}), "max_tokens"),
        (
            WireApi::Gemini,
            json!({"generationConfig":{"maxOutputTokens":100}}),
            "generationConfig.maxOutputTokens",
        ),
    ] {
        SourceAdapter::Native
            .validate_model_controls(wire, &request, &capabilities)
            .unwrap();
        if wire == WireApi::Gemini {
            request["generationConfig"]["maxOutputTokens"] = json!(101);
        } else {
            request[parameter] = json!(101);
        }
        let original = request.clone();
        let error = SourceAdapter::Native
            .validate_model_controls(wire, &request, &capabilities)
            .unwrap_err();
        assert_eq!(error.parameter(), Some(parameter));
        assert_eq!(request, original);
        SourceAdapter::Native
            .validate_model_controls(wire, &request, &Default::default())
            .unwrap();
    }
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
        WireApi::Gemini,
    ] {
        let adapter = SourceAdapter::between(WireApi::Responses, upstream).unwrap();
        for stream in [false, true] {
            for maximum in [100, 101] {
                let request = json!({"input":"Synthetic", "max_output_tokens":maximum});
                let prepared = adapter.prepare_request_with_capabilities(
                    AdapterRequestContext {
                        client_wire_api: WireApi::Responses,
                        request: &request,
                        model: "synthetic-model",
                        stream,
                        reasoning_mode: MessagesReasoningMode::Disabled,
                        cache_write_ttl: CacheWriteTtl::Provider,
                        previous: None,
                        response_scope: "synthetic",
                        response_id_seed: "synthetic",
                    },
                    &capabilities,
                );
                assert_eq!(
                    prepared.is_ok(),
                    maximum == 100,
                    "{upstream:?}: {prepared:?}"
                );
            }
        }
    }
}

fn admitted(runtime: &GatewayRuntime, request: &Value, stream: bool) -> Vec<ExecutorRoute> {
    let key = runtime.authenticate_secret("synthetic-secret").unwrap();
    let tier_policy = ServiceTierPolicy::pool_owned(request);
    let (excluded, admitted, error) = incompatible_routes(RouteCompatibility {
        runtime,
        key: &key,
        model: "glm-test",
        client: WireApi::Responses,
        request,
        stream,
        tier_policy: &tier_policy,
        now_ms: 2,
    });
    assert!(excluded.is_empty() || admitted.is_empty(), "{error:?}");
    admitted.into_values().collect()
}

#[test]
fn hosted_search_uses_exact_declared_native_endpoint_without_losing_controls() {
    for origin in [CapabilityOrigin::Catalog, CapabilityOrigin::Manual] {
        let runtime = runtime(Some((CapabilityStatus::Declared, origin)));
        for stream in [false, true] {
            let request = json!({
                "model":"glm-test", "input":"Synthetic search", "stream":stream,
                "tools":[{"type":"web_search","external_web_access":false,
                    "filters":{"allowed_domains":["example.invalid"]}}],
                "tool_choice":{"type":"web_search"},
                "include":["web_search_call.action.sources"]
            });
            let routes = admitted(&runtime, &request, stream);
            assert_eq!(routes.len(), 1);
            let route = &routes[0];
            assert_eq!(route.adapter, SourceAdapter::Native);
            assert!(route.upstream_url.path().ends_with("/responses"));
            let prepared = route
                .adapter
                .prepare_request(AdapterRequestContext {
                    client_wire_api: WireApi::Responses,
                    request: &request,
                    model: "glm-test",
                    stream,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: None,
                    response_scope: "synthetic-source",
                    response_id_seed: "synthetic-response",
                })
                .unwrap();
            assert_eq!(prepared.upstream_body(), &request);
        }
        let routes = admitted(&runtime, &json!({"input":"Synthetic text"}), false);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].adapter, SourceAdapter::Native);
    }
}

#[test]
fn host_profile_unknown_negative_and_probe_evidence_cannot_invent_native_search() {
    for declaration in [
        None,
        Some((CapabilityStatus::Unsupported, CapabilityOrigin::Catalog)),
        Some((CapabilityStatus::Unknown, CapabilityOrigin::Catalog)),
        Some((
            CapabilityStatus::Confirmed,
            CapabilityOrigin::GenerationProbe,
        )),
    ] {
        let runtime = runtime(declaration);
        for stream in [false, true] {
            assert!(
                admitted(
                    &runtime,
                    &json!({
                        "input":"Synthetic search", "tools":[{"type":"web_search"}]
                    }),
                    stream
                )
                .is_empty()
            );
        }
    }
}

#[test]
fn source_protocol_changes_cannot_steal_a_bound_conversation() {
    let runtime = runtime_with_source(dual_protocol_source());
    let key = runtime.authenticate_secret("synthetic-secret").unwrap();
    let routes = runtime.configured_executor_routes(&key, "glm-test", &[WireApi::Responses], false);
    assert_eq!(routes.len(), 2);
    assert_eq!(routes[0].adapter, SourceAdapter::Native);
    let first = &routes[1];
    assert_eq!(first.adapter, SourceAdapter::ResponsesToChatCompletions);
    runtime.bind_response_route_affinity(Some("resp_synthetic"), first, 2);
    let continuation = json!({"previous_response_id":"resp_synthetic",
        "input":"Synthetic search", "tools":[{"type":"web_search"}]});
    assert!(admitted(&runtime, &continuation, false).is_empty());
}

#[test]
fn exact_source_protocol_works_without_the_manufacturer_endpoint() {
    let mut source = source(Some((
        CapabilityStatus::Declared,
        CapabilityOrigin::Catalog,
    )));
    source
        .protocol_config
        .capabilities
        .push(ModelEndpointCapability {
            model_id: "glm-test".into(),
            upstream_wire_api: WireApi::ChatCompletions,
            status: CapabilityStatus::Unsupported,
            origin: CapabilityOrigin::Catalog,
            checked_at_ms: 1,
            features: BTreeMap::new(),
            reasoning_efforts: Vec::new(),
            token_limits: Default::default(),
            service_tiers: None,
        });
    let runtime = runtime_with_source(source);
    for stream in [false, true] {
        let routes = admitted(
            &runtime,
            &json!({"input":"Synthetic search",
            "tools":[{"type":"web_search"}]}),
            stream,
        );
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].adapter, SourceAdapter::Native);
    }
}

#[test]
fn configured_endpoint_pins_and_native_continuation_remain_authoritative() {
    for pinned in [WireApi::ChatCompletions, WireApi::Responses] {
        let mut source = source(Some((
            CapabilityStatus::Declared,
            CapabilityOrigin::Catalog,
        )));
        source.protocol_config.endpoint_hint = Some(pinned);
        let runtime = runtime_with_source(source);
        let request = json!({"input":"Synthetic search","tools":[{"type":"web_search"}]});
        let routes = admitted(&runtime, &request, false);
        if pinned == WireApi::ChatCompletions {
            assert!(routes.is_empty());
        } else {
            assert_eq!(routes.len(), 1);
            assert_eq!(routes[0].adapter, SourceAdapter::Native);
            runtime.bind_response_route_affinity(Some("resp_native"), &routes[0], 2);
            let next = admitted(
                &runtime,
                &json!({
                    "previous_response_id":"resp_native", "input":"Synthetic continuation"
                }),
                false,
            );
            assert_eq!(next.len(), 1);
            assert_eq!(next[0].adapter, SourceAdapter::Native);
        }
    }
}

#[test]
fn each_declared_client_protocol_uses_native_before_conversion() {
    let runtime = runtime_with_source(dual_protocol_source());
    let key = runtime.authenticate_secret("synthetic-secret").unwrap();
    for client in [WireApi::Responses, WireApi::ChatCompletions] {
        let routes = runtime.configured_executor_routes(&key, "glm-test", &[client], false);
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].adapter, SourceAdapter::Native);
        assert_eq!(routes[0].client_wire_api, client);
    }
    let routes = runtime.configured_executor_routes(&key, "glm-test", &[WireApi::Messages], false);
    assert_eq!(routes.len(), 2);
    assert_eq!(routes[0].adapter, SourceAdapter::MessagesToChatCompletions);
}

#[test]
fn converted_full_history_keeps_its_original_endpoint_after_native_is_declared() {
    let runtime = runtime_with_source(dual_protocol_source());
    let key = runtime.authenticate_secret("synthetic-secret").unwrap();
    let routes = runtime.configured_executor_routes(&key, "glm-test", &[WireApi::Responses], false);
    let original = &routes[1];
    let request = json!({"input":[{"role":"user","content":"Synthetic question"}],
        "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"},"strict":false}]});
    let prepared = original
        .adapter
        .prepare_request_with_capabilities(
            AdapterRequestContext {
                client_wire_api: WireApi::Responses,
                request: &request,
                model: "glm-test",
                stream: false,
                reasoning_mode: original.reasoning_mode,
                cache_write_ttl: original.cache_write_ttl,
                previous: None,
                response_scope: "synthetic-source",
                response_id_seed: "synthetic-response",
            },
            &crate::model_metadata::ModelCapabilities {
                reasoning: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let response = prepared
        .translate_response_bytes(
            &serde_json::to_vec(&json!({
                "id":"chat_synthetic","choices":[{"index":0,"message":{"role":"assistant",
                "content":null,"tool_calls":[{"id":"call_synthetic","type":"function",
                "function":{"name":"lookup","arguments":"{}"}}]},"finish_reason":"tool_calls"}]
            }))
            .unwrap(),
        )
        .unwrap()
        .unwrap();
    runtime.save_adapter_response_for_route(
        "synthetic-key",
        original,
        response.translated_response(),
        2,
    );
    let mut input = request["input"].as_array().unwrap().clone();
    input.extend(
        response.response_body()["output"]
            .as_array()
            .unwrap()
            .clone(),
    );
    input.push(json!({"type":"function_call_output","call_id":"call_synthetic","output":"Synthetic result"}));
    let resumed = json!({"input":input,"tools":request["tools"]});
    runtime
        .load_adapter_continuation(original, "synthetic-key", &resumed, 2)
        .expect("the retained full-history checkpoint must stay available")
        .expect("the original checkpoint must be restored");
    let next = admitted(&runtime, &resumed, false);
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].adapter, SourceAdapter::ResponsesToChatCompletions);
    let mut incompatible = resumed;
    incompatible["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"web_search"}));
    assert!(admitted(&runtime, &incompatible, false).is_empty());
}
