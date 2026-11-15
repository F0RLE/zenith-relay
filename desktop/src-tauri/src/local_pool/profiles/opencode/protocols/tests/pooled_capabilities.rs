use super::*;
use zenith_relay_core::protocol::{
    PoolModelConfigurationInput, SourceSummary, apply_model_metadata,
    apply_pool_model_configuration_with_context,
};

mod scoped_controls;

fn metadata() -> ModelMetadataCatalog {
    ModelMetadataCatalog::from_models_dev_json(r#"{
        "test/opaque-model":{"reasoning":true,"reasoning_effort_levels":["low","high"],
            "tool_call":true,"attachment":true,"modalities":{"input":["text","image","pdf"],"output":["text"]}},
        "test/media-model":{"reasoning":true,"reasoning_effort_levels":["low","high"],
            "tool_call":true,"attachment":true,"modalities":{"input":["text","image"],"output":["text"]}}
    }"#).unwrap()
}

fn source() -> SourceSummary {
    let ids = ["opaque-model", "media-model"];
    serde_json::from_value(json!({
        "id":"source","name":"Synthetic","enabled":true,"inPool":true,"draining":false,
        "operationalStatus":"rotation","baseUrl":"https://example.test/v1","wireApi":"responses",
        "models":ids,"allowedModels":[],"excludedModels":[],"priority":0,"weight":1,"secretAvailable":true,
        "protocolConfig":{"endpointHint":"responses","capabilities":ids.map(|id| json!({
            "modelId":id,"upstreamWireApi":"responses","status":"declared","origin":"catalog","checkedAtMs":1,
            "features":{"function_tools":"unsupported","images":"unsupported"},"reasoningEfforts":["high"]
        }))}
    })).unwrap()
}

fn projected_models(
    sources: &[SourceSummary],
    metadata: &ModelMetadataCatalog,
    policies: &BTreeMap<String, Vec<String>>,
) -> Vec<ModelSummary> {
    let ids =
        zenith_relay_core::normalize_model_ids(sources.iter().flat_map(|source| &source.models));
    let mut models = ids
        .iter()
        .map(|id| super::super::super::tests::model(id, true))
        .collect::<Vec<_>>();
    apply_model_metadata(&mut models, metadata);
    apply_pool_model_configuration_with_context(
        &mut models,
        PoolModelConfigurationInput {
            sources,
            accounts: &[],
            model_price_overrides: &BTreeMap::new(),
            model_reasoning_allowed_levels: policies,
            model_context_policies: &BTreeMap::new(),
            model_service_tier_overrides: &BTreeMap::new(),
            runtime: None,
        },
    );
    zenith_relay_core::protocol::apply_streaming_model_protocol_routes(
        &mut models,
        sources,
        &[],
        policies,
    );
    models
}

#[test]
fn pooled_profile_honors_route_numeric_limits_and_keeps_smaller_custom_windows() {
    let metadata = ModelMetadataCatalog::from_models_dev_json(
        r#"{"test/opaque-model":{"limit":{"context":10000,"input":9000,"output":1000}},
            "test/media-model":{"limit":{"context":10000,"input":9000,"output":1000}}}"#,
    )
    .unwrap();
    let mut source = source();
    for endpoint in &mut source.protocol_config.capabilities {
        endpoint.token_limits = zenith_relay_core::ModelTokenLimits {
            context: Some(2000),
            input: Some(1800),
            output: Some(200),
        };
    }
    let sources = [source];
    let mut models = sources[0]
        .models
        .iter()
        .map(|id| super::super::super::tests::model(id, true))
        .collect::<Vec<_>>();
    apply_model_metadata(&mut models, &metadata);
    apply_pool_model_configuration_with_context(
        &mut models,
        PoolModelConfigurationInput {
            sources: &sources,
            accounts: &[],
            model_price_overrides: &BTreeMap::new(),
            model_reasoning_allowed_levels: &BTreeMap::new(),
            model_context_policies: &BTreeMap::new(),
            model_service_tier_overrides: &BTreeMap::new(),
            runtime: None,
        },
    );
    let mut config = Map::new();
    apply(
        &mut config,
        "http://127.0.0.1:14998/v1",
        "synthetic",
        &models,
    )
    .unwrap();
    for id in ["opaque-model", "media-model"] {
        assert_eq!(
            config["provider"][PROVIDER_ID]["models"][id]["limit"],
            json!({
                "context":2000,"input":1800,"output":200
            })
        );
    }
    config["provider"][PROVIDER_ID]["models"]["opaque-model"]["limit"]["context"] = 1000.into();
    apply(
        &mut config,
        "http://127.0.0.1:14998/v1",
        "synthetic",
        &models,
    )
    .unwrap();
    let saved = &config["provider"][PROVIDER_ID]["models"]["opaque-model"]["limit"];
    assert_eq!(saved["context"], 1000);
    assert_eq!(saved["input"], 1000);
    assert_eq!(saved["output"], 200);
    assert_eq!(models[0].catalog_context_limit, Some(10000));
}

#[test]
fn streaming_profile_does_not_inherit_controls_or_small_windows_from_buffered_members() {
    let metadata = metadata();
    let mut buffered = source();
    buffered.id = "buffered".into();
    let mut streaming = source();
    streaming.id = "streaming".into();
    for endpoint in &mut buffered.protocol_config.capabilities {
        endpoint.features.insert(
            zenith_relay_core::ProtocolFeature::Streaming,
            zenith_relay_core::CapabilityStatus::Unsupported,
        );
        endpoint.features.insert(
            zenith_relay_core::ProtocolFeature::FunctionTools,
            zenith_relay_core::CapabilityStatus::Declared,
        );
        endpoint.reasoning_efforts = vec!["low".into()];
        endpoint.token_limits = zenith_relay_core::ModelTokenLimits {
            context: Some(1000),
            input: Some(900),
            output: Some(100),
        };
    }
    for endpoint in &mut streaming.protocol_config.capabilities {
        endpoint.features.insert(
            zenith_relay_core::ProtocolFeature::Streaming,
            zenith_relay_core::CapabilityStatus::Declared,
        );
        endpoint.token_limits = zenith_relay_core::ModelTokenLimits {
            context: Some(2000),
            input: Some(1800),
            output: Some(200),
        };
    }
    let sources = [buffered, streaming];
    let mut models = sources[0]
        .models
        .iter()
        .map(|id| super::super::super::tests::model(id, true))
        .collect::<Vec<_>>();
    apply_model_metadata(&mut models, &metadata);
    apply_pool_model_configuration_with_context(
        &mut models,
        PoolModelConfigurationInput {
            sources: &sources,
            accounts: &[],
            model_price_overrides: &BTreeMap::new(),
            model_reasoning_allowed_levels: &BTreeMap::new(),
            model_context_policies: &BTreeMap::new(),
            model_service_tier_overrides: &BTreeMap::new(),
            runtime: None,
        },
    );
    zenith_relay_core::protocol::apply_streaming_model_protocol_routes(
        &mut models,
        &sources,
        &[],
        &BTreeMap::new(),
    );
    let mut config = Map::new();
    apply(
        &mut config,
        "http://127.0.0.1:14998/v1",
        "synthetic",
        &models,
    )
    .unwrap();
    let configured = &config["provider"][PROVIDER_ID]["models"]["opaque-model"];
    assert_eq!(configured["tool_call"], false);
    assert_eq!(configured["variants"]["low"]["disabled"], true);
    assert!(configured["variants"]["high"].get("disabled").is_none());
    assert_eq!(
        configured["limit"],
        json!({"context":2000,"input":1800,"output":200})
    );
}

#[test]
fn pooled_profile_uses_endpoint_evidence_and_preserves_non_image_media() {
    let sources = [source()];
    let metadata = metadata();
    let mut models = sources[0]
        .models
        .iter()
        .map(|id| super::super::super::tests::model(id, true))
        .collect::<Vec<_>>();
    apply_model_metadata(&mut models, &metadata);
    apply_pool_model_configuration_with_context(
        &mut models,
        PoolModelConfigurationInput {
            sources: &sources,
            accounts: &[],
            model_price_overrides: &BTreeMap::new(),
            model_reasoning_allowed_levels: &BTreeMap::new(),
            model_context_policies: &BTreeMap::new(),
            model_service_tier_overrides: &BTreeMap::new(),
            runtime: None,
        },
    );
    let mut config = Map::new();
    apply(
        &mut config,
        "http://127.0.0.1:14998/v1",
        "synthetic",
        &models,
    )
    .unwrap();
    let models = &config["provider"][PROVIDER_ID]["models"];
    for id in ["opaque-model", "media-model"] {
        assert_eq!(models[id]["tool_call"], false);
        assert_eq!(models[id]["variants"]["low"]["disabled"], true);
        assert!(models[id]["variants"]["high"].get("disabled").is_none());
    }
    assert_eq!(
        models["opaque-model"]["modalities"]["input"],
        json!(["text", "pdf"])
    );
    assert_eq!(models["opaque-model"]["attachment"], true);
    assert_eq!(
        models["media-model"]["modalities"]["input"],
        json!(["text"])
    );
    assert_eq!(models["media-model"]["attachment"], false);
    assert_eq!(
        metadata.capabilities_for("opaque-model").input_modalities,
        ["text", "image", "pdf"]
    );
}

#[test]
fn direct_profile_image_exclusions_do_not_discard_other_declared_modalities() {
    let source = source();
    let configured = super::super::super::model_config_ids(
        &source.models,
        &metadata(),
        WireApi::Responses,
        &source
            .protocol_config
            .effective_capabilities(&source.base_url, &source.models),
    );
    assert_eq!(
        configured["opaque-model"]["modalities"]["input"],
        json!(["text", "pdf"])
    );
    assert_eq!(configured["opaque-model"]["attachment"], true);
    assert_eq!(
        configured["media-model"]["modalities"]["input"],
        json!(["text"])
    );
    assert_eq!(configured["media-model"]["attachment"], false);
}
