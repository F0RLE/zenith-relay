use super::fetch::{merge_route_model_price, parse_upstream_models};
use super::*;
use crate::UpstreamProtocol;
use crate::WireApi;
use crate::{CapabilityOrigin, CapabilityStatus};
use axum::{routing::get, Json, Router};
use tokio::net::TcpListener;

#[test]
fn merges_generic_and_messages_prices_for_one_model() {
    let generic = ApiModelPriceOverride {
        input_micro_usd_per_million: 1,
        cached_input_micro_usd_per_million: Some(2),
        cache_write_5m_micro_usd_per_million: None,
        cache_write_1h_micro_usd_per_million: None,
        output_micro_usd_per_million: 3,
    };
    let messages = ApiModelPriceOverride {
        cache_write_5m_micro_usd_per_million: Some(4),
        cache_write_1h_micro_usd_per_million: Some(5),
        ..generic
    };
    assert_eq!(merge_route_model_price(generic, messages), Some(messages));
    assert!(merge_route_model_price(
        generic,
        ApiModelPriceOverride {
            input_micro_usd_per_million: 99,
            ..messages
        }
    )
    .is_none());
}

#[test]
fn generic_model_catalog_retains_explicit_cache_write_windows() {
    let models = parse_upstream_models(
        UpstreamProtocol::Responses,
        &serde_json::json!({"data": [{
            "id": "claude-test",
            "inputCostMicrousdPerMillion": 1_000_000,
            "outputCostMicrousdPerMillion": 2_000_000,
            "promptCacheWriteCostsByTtl": { "5m": 1_250_000, "1h": 2_500_000 }
        }]}),
    )
    .unwrap();
    let price = models[0].1.unwrap();
    assert_eq!(price.cache_write_5m_micro_usd_per_million, Some(1_250_000));
    assert_eq!(price.cache_write_1h_micro_usd_per_million, Some(2_500_000));
}

#[test]
fn native_gemini_catalog_requires_generate_content_capability() {
    let models = parse_upstream_models(
        UpstreamProtocol::GeminiGenerateContent,
        &serde_json::json!({"models": [
            {"name": "models/gemini-usable", "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/gemini-unsupported", "supportedGenerationMethods": ["countTokens"]}
        ]}),
    )
    .unwrap();
    assert_eq!(models[0].0, "gemini-usable");
    assert_eq!(models.len(), 1);
}

#[tokio::test]
async fn valid_empty_catalog_is_successful() {
    let app = Router::new().route(
        "/v1/models",
        get(|| async { Json(serde_json::json!({"data": []})) }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let source = ProviderSource {
        id: "source-empty".into(),
        name: "Empty source".into(),
        base_url: format!("http://{address}/v1"),
        api_key: "secret".into(),
        wire_api: WireApi::Responses,
        models: Vec::new(),
    };

    let discovery = discover_source_models_and_protocol_bindings(&source, &[])
        .await
        .unwrap();
    assert!(discovery.models.is_empty());
    assert_eq!(discovery.protocol_bindings.len(), 1);
    assert!(discovery.protocol_bindings[0].model_ids.is_empty());
    server.abort();
}

#[test]
fn apply_catalog_replaces_models_and_only_a_resolved_url() {
    let price = ApiModelPriceOverride {
        input_micro_usd_per_million: 1,
        cached_input_micro_usd_per_million: None,
        cache_write_5m_micro_usd_per_million: None,
        cache_write_1h_micro_usd_per_million: None,
        output_micro_usd_per_million: 2,
    };
    let catalog = ModelEndpointCapability {
        model_id: "model-a".into(),
        upstream_wire_api: WireApi::Responses,
        status: CapabilityStatus::Declared,
        origin: CapabilityOrigin::Catalog,
        checked_at_ms: 10,
        features: BTreeMap::new(),
        reasoning_efforts: Vec::new(),
    };
    let probe = ModelEndpointCapability {
        model_id: "old".into(),
        upstream_wire_api: WireApi::Responses,
        status: CapabilityStatus::Confirmed,
        origin: CapabilityOrigin::GenerationProbe,
        checked_at_ms: 1,
        features: BTreeMap::new(),
        reasoning_efforts: Vec::new(),
    };
    let mut base_url = "https://kept.example/v1".to_string();
    let mut models = vec!["old".into()];
    let mut bindings = vec![SourceProtocolBinding::legacy(
        WireApi::ChatCompletions,
        &["old".into()],
    )];
    let mut config = SourceProtocolConfig {
        revision: 4,
        capabilities: vec![
            ModelEndpointCapability {
                model_id: "stale".into(),
                ..catalog.clone()
            },
            probe.clone(),
        ],
        endpoint_hint: Some(WireApi::ChatCompletions),
    };
    let mut prices = BTreeMap::from([("old".into(), price)]);
    let resolved = SourceDiscovery {
        models: vec!["model-a".into()],
        protocol_bindings: vec![SourceProtocolBinding::legacy(
            WireApi::Responses,
            &["model-a".into()],
        )],
        resolved_base_url: Some("https://resolved.example/v1".into()),
        detected_model_prices: BTreeMap::from([("model-a".into(), price)]),
        capabilities: vec![catalog.clone()],
    };

    resolved.apply_catalog(
        &mut base_url,
        &mut models,
        &mut bindings,
        &mut config,
        &mut prices,
    );

    assert_eq!(base_url, "https://resolved.example/v1");
    assert_eq!(models, ["model-a"]);
    assert_eq!(bindings, resolved.protocol_bindings);
    assert_eq!(prices, resolved.detected_model_prices);
    assert_eq!(config.revision, 4);
    assert_eq!(config.endpoint_hint, Some(WireApi::ChatCompletions));
    assert_eq!(config.capabilities, vec![probe.clone(), catalog]);

    let unresolved = SourceDiscovery {
        models: vec!["model-b".into()],
        protocol_bindings: vec![SourceProtocolBinding::legacy(
            WireApi::Responses,
            &["model-b".into()],
        )],
        resolved_base_url: None,
        detected_model_prices: BTreeMap::new(),
        capabilities: Vec::new(),
    };
    unresolved.apply_catalog(
        &mut base_url,
        &mut models,
        &mut bindings,
        &mut config,
        &mut prices,
    );

    assert_eq!(base_url, "https://resolved.example/v1");
    assert_eq!(models, ["model-b"]);
    assert_eq!(bindings, unresolved.protocol_bindings);
    assert!(prices.is_empty());
    assert_eq!(config.capabilities, vec![probe]);
}

#[test]
fn source_catalog_changed_follows_apply_catalog() {
    let price = ApiModelPriceOverride {
        input_micro_usd_per_million: 1,
        cached_input_micro_usd_per_million: None,
        cache_write_5m_micro_usd_per_million: None,
        cache_write_1h_micro_usd_per_million: None,
        output_micro_usd_per_million: 2,
    };
    let mut base_url = "https://kept.example/v1".to_string();
    let mut models = vec!["model-a".into()];
    let mut bindings = vec![SourceProtocolBinding::legacy(
        WireApi::Responses,
        &["model-a".into()],
    )];
    let mut config = SourceProtocolConfig::default();
    let mut prices = BTreeMap::from([("model-a".into(), price)]);
    let before = SourceCatalogEvidence {
        base_url: &base_url,
        models: &models,
        protocol_bindings: &bindings,
        protocol_config: &config,
        detected_model_prices: &prices,
    };
    assert!(!source_catalog_changed(&before, &before));

    let same = SourceDiscovery {
        models: models.clone(),
        protocol_bindings: bindings.clone(),
        resolved_base_url: None,
        detected_model_prices: prices.clone(),
        capabilities: Vec::new(),
    };
    let previous_base = base_url.clone();
    let previous_models = models.clone();
    let previous_bindings = bindings.clone();
    let previous_config = config.clone();
    let previous_prices = prices.clone();
    same.apply_catalog(
        &mut base_url,
        &mut models,
        &mut bindings,
        &mut config,
        &mut prices,
    );
    assert!(!source_catalog_changed(
        &SourceCatalogEvidence {
            base_url: &previous_base,
            models: &previous_models,
            protocol_bindings: &previous_bindings,
            protocol_config: &previous_config,
            detected_model_prices: &previous_prices,
        },
        &SourceCatalogEvidence {
            base_url: &base_url,
            models: &models,
            protocol_bindings: &bindings,
            protocol_config: &config,
            detected_model_prices: &prices,
        },
    ));

    let changed = SourceDiscovery {
        models: vec!["model-b".into()],
        resolved_base_url: Some("https://resolved.example/v1".into()),
        detected_model_prices: BTreeMap::new(),
        ..same
    };
    changed.apply_catalog(
        &mut base_url,
        &mut models,
        &mut bindings,
        &mut config,
        &mut prices,
    );
    assert!(source_catalog_changed(
        &SourceCatalogEvidence {
            base_url: &previous_base,
            models: &previous_models,
            protocol_bindings: &previous_bindings,
            protocol_config: &previous_config,
            detected_model_prices: &previous_prices,
        },
        &SourceCatalogEvidence {
            base_url: &base_url,
            models: &models,
            protocol_bindings: &bindings,
            protocol_config: &config,
            detected_model_prices: &prices,
        },
    ));
    assert_eq!(base_url, "https://resolved.example/v1");
    assert_eq!(models, ["model-b"]);
    assert!(prices.is_empty());
}
