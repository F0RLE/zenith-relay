use super::*;
use crate::{CapabilityStatus, ProtocolFeature};
use serde_json::{Value, json};

fn reference() -> ModelMetadataCatalog {
    ModelMetadataCatalog::from_models_dev_json(
        r#"{
        "test/opaque-model":{"tool_call":true,"attachment":true,"reasoning":true,
            "reasoning_effort_levels":["low","high"]}
    }"#,
    )
    .unwrap()
}

fn source(id: &str, upstream: WireApi, features: Value, efforts: &[&str]) -> SourceSummary {
    let mut source = source_summary(id, &["opaque-model"]);
    source.wire_api = upstream;
    source.protocol_bindings = vec![SourceProtocolBinding::legacy(upstream, &source.models)];
    source.protocol_config = serde_json::from_value(json!({
        "endpointHint":upstream,"capabilities":[{
            "modelId":"opaque-model","upstreamWireApi":upstream,"status":"declared",
            "origin":"catalog","checkedAtMs":1,"features":features,"reasoningEfforts":efforts
        }]
    }))
    .unwrap();
    source
}

fn project(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    catalog: &ModelMetadataCatalog,
) -> ModelSummary {
    let mut models = pool_model_summaries(sources, accounts, &[]);
    apply_model_metadata(&mut models, catalog);
    apply_pool_model_configuration_with_context(
        &mut models,
        PoolModelConfigurationInput {
            sources,
            accounts,
            model_price_overrides: &BTreeMap::new(),
            model_reasoning_allowed_levels: &BTreeMap::new(),
            model_context_policies: &BTreeMap::new(),
            model_service_tier_overrides: &BTreeMap::new(),
            runtime: None,
        },
    );
    models
        .into_iter()
        .find(|model| model.id == "opaque-model")
        .unwrap()
}

fn route(model: &ModelSummary, upstream: WireApi) -> &ModelProtocolRoute {
    model
        .protocol_routes
        .iter()
        .find(|route| {
            route.client_wire_api == WireApi::Responses && route.upstream_wire_api == upstream
        })
        .unwrap()
}

#[test]
fn endpoint_numeric_limits_remain_bound_to_the_client_and_upstream() {
    let catalog = ModelMetadataCatalog::from_models_dev_json(
        r#"{"test/opaque-model":{"limit":{"context":100000,"input":90000,"output":10000}}}"#,
    )
    .unwrap();
    let mut responses = source("responses", WireApi::Responses, json!({}), &[]);
    responses.protocol_config.capabilities[0].token_limits = crate::ModelTokenLimits {
        context: Some(2000),
        input: Some(1800),
        output: Some(200),
    };
    let mut messages = source("messages", WireApi::Messages, json!({}), &[]);
    messages.protocol_config.capabilities[0].token_limits = crate::ModelTokenLimits {
        context: Some(50000),
        input: Some(40000),
        output: Some(5000),
    };
    let model = project(&[responses, messages], &[], &catalog);
    for (upstream, expected) in [
        (
            WireApi::Responses,
            json!({"context":2000,"input":1800,"output":200}),
        ),
        (
            WireApi::Messages,
            json!({"context":50000,"input":40000,"output":5000}),
        ),
    ] {
        let projected = serde_json::to_value(route(&model, upstream)).unwrap();
        assert_eq!(projected["tokenLimits"], expected);
    }
    assert_eq!(model.catalog_context_limit, Some(100000));
    assert_eq!(
        catalog.capabilities_for("opaque-model").context_limit,
        Some(100000)
    );
}

#[test]
fn numeric_route_limits_use_eligible_alternatives_without_reviving_disabled_members() {
    let catalog = reference();
    let mut small = source("small", WireApi::Responses, json!({}), &[]);
    small.protocol_config.capabilities[0].token_limits = crate::ModelTokenLimits {
        context: Some(1000),
        input: Some(900),
        output: Some(100),
    };
    let mut large = small.clone();
    large.id = "large".into();
    large.protocol_config.capabilities[0].token_limits = crate::ModelTokenLimits {
        context: Some(2000),
        input: Some(1800),
        output: Some(200),
    };
    for (enabled, expected) in [(true, 1000), (false, 2000)] {
        small.enabled = enabled;
        let model = project(&[small.clone(), large.clone()], &[], &catalog);
        let projected = serde_json::to_value(route(&model, WireApi::Responses)).unwrap();
        assert_eq!(projected["tokenLimits"]["context"], expected);
    }
}

#[test]
fn streaming_projection_filters_members_before_features_efforts_and_numeric_limits() {
    let mut buffered = source(
        "buffered",
        WireApi::Responses,
        json!({
            "streaming":"unsupported","function_tools":"declared","images":"declared"
        }),
        &["low"],
    );
    buffered.protocol_config.capabilities[0].token_limits = crate::ModelTokenLimits {
        context: Some(1000),
        input: Some(900),
        output: Some(100),
    };
    let mut streaming = source(
        "streaming",
        WireApi::Responses,
        json!({
            "streaming":"declared","function_tools":"unsupported","images":"unsupported"
        }),
        &["high"],
    );
    streaming.protocol_config.capabilities[0].token_limits = crate::ModelTokenLimits {
        context: Some(2000),
        input: Some(1800),
        output: Some(200),
    };
    for enabled in [true, false] {
        streaming.enabled = enabled;
        let sources = [buffered.clone(), streaming.clone()];
        let original = project(&sources, &[], &reference());
        assert_eq!(
            route(&original, WireApi::Responses).token_limits.context,
            Some(1000)
        );
        let mut projected = [original.clone()];
        apply_streaming_model_protocol_routes(&mut projected, &sources, &[], &BTreeMap::new());
        assert_eq!(projected[0].id, original.id);
        assert_eq!(
            projected[0].catalog_capabilities,
            original.catalog_capabilities
        );
        assert_eq!(
            projected[0].reasoning_allowed_levels,
            original.reasoning_allowed_levels
        );
        if enabled {
            let route = route(&projected[0], WireApi::Responses);
            assert_eq!(
                route.features[&ProtocolFeature::FunctionTools],
                CapabilityStatus::Unsupported
            );
            assert_eq!(
                route.features[&ProtocolFeature::Images],
                CapabilityStatus::Unsupported
            );
            assert_eq!(route.reasoning_efforts, ["high"]);
            assert_eq!(route.token_limits.context, Some(2000));
        } else {
            assert!(projected[0].protocol_routes.is_empty());
            assert!(!original.protocol_routes.is_empty());
        }
    }
}

#[test]
fn reference_streaming_exclusion_is_not_overridden_by_a_declared_endpoint() {
    let catalog =
        ModelMetadataCatalog::from_models_dev_json(r#"{"test/opaque-model":{"streaming":false}}"#)
            .unwrap();
    let sources = [source(
        "source",
        WireApi::Responses,
        json!({"streaming":"confirmed"}),
        &[],
    )];
    let original = project(&sources, &[], &catalog);
    let mut models = [original.clone()];
    apply_streaming_model_protocol_routes(&mut models, &sources, &[], &BTreeMap::new());
    assert!(models[0].protocol_routes.is_empty());
    assert_eq!(
        models[0].catalog_capabilities,
        original.catalog_capabilities
    );
    assert!(!original.protocol_routes.is_empty());
}

#[test]
fn endpoint_features_and_efforts_do_not_cross_upstream_protocols() {
    let sources = [
        source(
            "responses",
            WireApi::Responses,
            json!({"function_tools":"unsupported","images":"unsupported"}),
            &["high"],
        ),
        source(
            "messages",
            WireApi::Messages,
            json!({"function_tools":"declared"}),
            &["low"],
        ),
    ];
    let model = project(&sources, &[], &reference());
    let responses = route(&model, WireApi::Responses);
    assert_eq!(
        responses.features[&ProtocolFeature::FunctionTools],
        CapabilityStatus::Unsupported
    );
    assert_eq!(
        responses.features[&ProtocolFeature::Images],
        CapabilityStatus::Unsupported
    );
    assert_eq!(responses.reasoning_efforts, ["high"]);
    let messages = route(&model, WireApi::Messages);
    assert!(messages.features[&ProtocolFeature::FunctionTools].available());
    assert_eq!(messages.reasoning_efforts, ["low"]);
    assert_eq!(model.catalog_reasoning_effort_levels, ["low", "high"]);
}

#[test]
fn source_evidence_is_bound_to_the_client_protocol_not_only_the_upstream() {
    let mut converted = source(
        "converted",
        WireApi::Messages,
        json!({"function_tools":"unsupported"}),
        &["high"],
    );
    converted.resolved_protocol_bindings = Some(vec![SourceProtocolBinding {
        wire_api: WireApi::Responses,
        adapter: SourceAdapter::ResponsesToMessages,
        reasoning_mode: MessagesReasoningMode::Adaptive,
        cache_write_ttl: Default::default(),
        model_ids: converted.models.clone(),
    }]);
    let mut native = source(
        "native",
        WireApi::Messages,
        json!({"function_tools":"confirmed"}),
        &["low"],
    );
    native.resolved_protocol_bindings = Some(vec![SourceProtocolBinding::legacy(
        WireApi::Messages,
        &native.models,
    )]);
    converted.protocol_config.capabilities[0]
        .token_limits
        .context = Some(1000);
    native.protocol_config.capabilities[0].token_limits.context = Some(2000);
    let model = project(&[converted, native], &[], &reference());
    let responses = route(&model, WireApi::Messages);
    assert_eq!(responses.token_limits.context, Some(1000));
    assert_eq!(
        responses.features[&ProtocolFeature::FunctionTools],
        CapabilityStatus::Unsupported
    );
    assert_eq!(responses.reasoning_efforts, ["high"]);
    let messages = model
        .protocol_routes
        .iter()
        .find(|route| route.client_wire_api == WireApi::Messages)
        .unwrap();
    assert_eq!(
        messages.features[&ProtocolFeature::FunctionTools],
        CapabilityStatus::Confirmed
    );
    assert_eq!(messages.token_limits.context, Some(2000));
    assert_eq!(messages.reasoning_efforts, ["low"]);
}

#[test]
fn route_alternatives_union_support_but_ineligible_members_cannot_revive_it() {
    let narrow = source(
        "narrow",
        WireApi::Responses,
        json!({"function_tools":"unsupported"}),
        &["high"],
    );
    let wide = source(
        "wide",
        WireApi::Responses,
        json!({"function_tools":"confirmed"}),
        &["low"],
    );
    let both = project(&[narrow.clone(), wide.clone()], &[], &reference());
    let projected = route(&both, WireApi::Responses);
    assert_eq!(
        projected.features[&ProtocolFeature::FunctionTools],
        CapabilityStatus::Confirmed
    );
    assert_eq!(projected.reasoning_efforts, ["low", "high"]);
    for condition in 0..5 {
        let mut excluded = wide.clone();
        match condition {
            0 => excluded.enabled = false,
            1 => excluded.in_pool = false,
            2 => excluded.draining = true,
            3 => excluded.secret_available = false,
            _ => excluded.excluded_models = vec!["opaque-model".into()],
        }
        let model = project(&[narrow.clone(), excluded], &[], &reference());
        let projected = route(&model, WireApi::Responses);
        assert_eq!(
            projected.features[&ProtocolFeature::FunctionTools],
            CapabilityStatus::Unsupported
        );
        assert_eq!(projected.reasoning_efforts, ["high"]);
    }
}

#[test]
fn disjoint_endpoint_efforts_stay_empty_instead_of_falling_back_to_reference_levels() {
    let source = source("disjoint", WireApi::Responses, json!({}), &["max"]);
    let model = project(&[source], &[], &reference());
    let projected = route(&model, WireApi::Responses);
    assert!(projected.features[&ProtocolFeature::Reasoning].available());
    assert!(projected.reasoning_efforts.is_empty());
}

#[test]
fn account_routes_keep_their_own_support_without_resurrecting_disabled_accounts() {
    let denied = source(
        "denied",
        WireApi::Responses,
        json!({"function_tools":"unsupported"}),
        &["high"],
    );
    let mut account = account_summary(true, &["opaque-model"]);
    let model = project(
        std::slice::from_ref(&denied),
        &[account.clone()],
        &reference(),
    );
    let projected = route(&model, WireApi::Responses);
    assert!(projected.features[&ProtocolFeature::FunctionTools].available());
    assert_eq!(projected.reasoning_efforts, ["low", "high"]);
    account.enabled = false;
    let model = project(&[denied], &[account], &reference());
    assert_eq!(
        route(&model, WireApi::Responses).features[&ProtocolFeature::FunctionTools],
        CapabilityStatus::Unsupported
    );
}

#[test]
fn unknown_alternatives_and_reference_exclusions_retain_distinct_meaning() {
    let denied = source(
        "denied",
        WireApi::Responses,
        json!({"function_tools":"unsupported"}),
        &[],
    );
    let unknown = source("unknown", WireApi::Responses, json!({}), &[]);
    let model = project(
        &[denied.clone(), unknown],
        &[],
        &ModelMetadataCatalog::empty(),
    );
    assert_eq!(
        route(&model, WireApi::Responses).features[&ProtocolFeature::FunctionTools],
        CapabilityStatus::Unknown
    );
    let model = project(&[denied], &[], &ModelMetadataCatalog::empty());
    assert_eq!(
        route(&model, WireApi::Responses).features[&ProtocolFeature::FunctionTools],
        CapabilityStatus::Unsupported
    );
    let excluded = ModelMetadataCatalog::from_models_dev_json(
        r#"{
        "test/opaque-model":{"tool_call":false,"reasoning":false}
    }"#,
    )
    .unwrap();
    let positive = source(
        "positive",
        WireApi::Responses,
        json!({"function_tools":"confirmed","reasoning":"confirmed"}),
        &["high"],
    );
    let model = project(&[positive], &[], &excluded);
    let projected = route(&model, WireApi::Responses);
    assert_eq!(
        projected.features[&ProtocolFeature::FunctionTools],
        CapabilityStatus::Unsupported
    );
    assert_eq!(
        projected.features[&ProtocolFeature::Reasoning],
        CapabilityStatus::Unsupported
    );
    assert!(projected.reasoning_efforts.is_empty());
}
