use super::super::sqlite::Store;
use crate::state::{identity_hint, SourceRecord};
use crate::store::test_support::test_root;
use std::collections::BTreeMap;
use std::fs;
use zenith_relay_core::protocol::PresetRoutingPolicy;
use zenith_relay_core::{ApiModelPriceOverride, ApiModelPriceSources, DefaultServiceTier};

#[test]
fn quota_request_timeout_is_validated_and_persists() {
    let root = test_root("quota-policy");
    let path = root.join("relay.sqlite");
    let store = Store::open(path.clone()).unwrap();
    assert_eq!(store.quota_request_timeout_seconds().unwrap(), 20);
    store
        .set_metadata("quota_request_timeout_seconds", "9")
        .unwrap();
    assert!(store.quota_request_timeout_seconds().is_err());
    store
        .set_metadata("quota_request_timeout_seconds", "10")
        .unwrap();
    drop(store);

    let reopened = Store::open(path).unwrap();
    assert_eq!(reopened.quota_request_timeout_seconds().unwrap(), 10);
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn routing_policy_is_validated_and_persists() {
    let root = test_root("routing-policy");
    let path = root.join("relay.sqlite");
    let store = Store::open(path.clone()).unwrap();
    assert_eq!(
        store.routing_policy().unwrap(),
        PresetRoutingPolicy {
            tool_policy: Some(Default::default()),
            pool_routing: Some(Default::default()),
            basis_points_enabled: false,
            max_retry_candidates: 3,
            default_service_tier: DefaultServiceTier::Standard,
            image_base_model: None,
        }
    );
    assert!(store
        .set_routing_policy(&PresetRoutingPolicy {
            tool_policy: None,
            pool_routing: None,
            basis_points_enabled: false,
            max_retry_candidates: 0,
            default_service_tier: DefaultServiceTier::Standard,
            image_base_model: None,
        })
        .is_err());
    store
        .set_routing_policy(&PresetRoutingPolicy {
            tool_policy: None,
            pool_routing: None,
            basis_points_enabled: true,
            max_retry_candidates: 5,
            default_service_tier: DefaultServiceTier::Fast,
            image_base_model: Some("gpt-5.4-mini".into()),
        })
        .unwrap();
    drop(store);

    let reopened = Store::open(path).unwrap();
    assert_eq!(
        reopened.routing_policy().unwrap(),
        PresetRoutingPolicy {
            tool_policy: Some(Default::default()),
            pool_routing: Some(Default::default()),
            basis_points_enabled: false,
            max_retry_candidates: 5,
            default_service_tier: DefaultServiceTier::Fast,
            image_base_model: Some("gpt-5.4-mini".into()),
        }
    );
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn hidden_models_are_validated_deduplicated_and_persisted() {
    let root = test_root("hidden-models");
    let path = root.join("relay.sqlite");
    let store = Store::open(path.clone()).unwrap();
    assert!(store.hidden_models().unwrap().is_empty());
    store
        .set_hidden_models(vec![" gpt-5.4 ".into(), "GPT-5.4".into()])
        .unwrap();
    assert!(store.set_hidden_models(vec!["x\nunsafe".into()]).is_err());
    drop(store);

    let reopened = Store::open(path).unwrap();
    assert_eq!(reopened.hidden_models().unwrap(), ["gpt-5.4"]);
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn model_price_overrides_are_validated_normalized_and_persisted() {
    let root = test_root("model-prices");
    let path = root.join("relay.sqlite");
    let store = Store::open(path.clone()).unwrap();
    let price = ApiModelPriceOverride {
        input_micro_usd_per_million: 1_000_000,
        cached_input_micro_usd_per_million: Some(100_000),
        cache_write_5m_micro_usd_per_million: Some(1_250_000),
        cache_write_1h_micro_usd_per_million: Some(2_500_000),
        output_micro_usd_per_million: 2_000_000,
    };
    store
        .set_model_price_overrides(BTreeMap::from([(" GPT-Test ".into(), price)]))
        .unwrap();
    assert!(store
        .set_model_price_overrides(BTreeMap::from([("unsafe\nmodel".into(), price)]))
        .is_err());
    drop(store);

    let reopened = Store::open(path).unwrap();
    assert_eq!(
        reopened.model_price_overrides().unwrap().get("gpt-test"),
        Some(&price)
    );
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn detected_source_prices_fall_back_from_manual_overrides_and_refresh_cleanly() {
    let root = test_root("source-detected-prices");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    let detected = ApiModelPriceOverride {
        input_micro_usd_per_million: 1_000_000,
        cached_input_micro_usd_per_million: Some(100_000),
        cache_write_5m_micro_usd_per_million: None,
        cache_write_1h_micro_usd_per_million: None,
        output_micro_usd_per_million: 2_000_000,
    };
    let manual = ApiModelPriceOverride {
        input_micro_usd_per_million: 3_000_000,
        cached_input_micro_usd_per_million: Some(300_000),
        cache_write_5m_micro_usd_per_million: None,
        cache_write_1h_micro_usd_per_million: None,
        output_micro_usd_per_million: 6_000_000,
    };
    let mut source = SourceRecord {
        id: "source-detected".into(),
        name: "Detected prices".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        base_url: "https://example.test/v1".into(),
        secret_ref: "source:detected".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: zenith_relay_core::WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        models: vec!["private-model".into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::from([("private-model".into(), manual)]),
        detected_model_prices: BTreeMap::from([("private-model".into(), detected)]),
        last_error_code: None,
    };
    store.save_source(&source).unwrap();

    let price = |store: &Store| {
        store
            .source_price_overrides()
            .unwrap()
            .get(&identity_hint("source-detected"))
            .and_then(|prices| prices.get("private-model"))
            .copied()
    };
    assert_eq!(
        price(&store),
        Some(ApiModelPriceSources {
            provider: Some(detected),
            manual: Some(manual),
        })
    );

    source.model_price_overrides.clear();
    store.save_source(&source).unwrap();
    assert_eq!(
        price(&store),
        Some(ApiModelPriceSources {
            provider: Some(detected),
            manual: None,
        })
    );

    // A later catalog refresh can legitimately omit a still-available
    // model's price; it must clear the old detected value.
    source.detected_model_prices.clear();
    store.save_source(&source).unwrap();
    assert_eq!(price(&store), None);

    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn model_reasoning_allowed_levels_are_validated_normalized_and_persisted() {
    let root = test_root("model-reasoning");
    let path = root.join("relay.sqlite");
    let store = Store::open(path.clone()).unwrap();
    store
        .set_model_reasoning_allowed_levels(BTreeMap::from([(
            " GPT-Test ".into(),
            vec![" HIGH ".into(), "high".into(), "ultra".into()],
        )]))
        .unwrap();
    assert!(store
        .set_model_reasoning_allowed_levels(BTreeMap::from([(
            "unsafe\nmodel".into(),
            vec!["high".into()],
        )]))
        .is_err());
    drop(store);

    let reopened = Store::open(path).unwrap();
    assert_eq!(
        reopened
            .model_reasoning_allowed_levels()
            .unwrap()
            .get("gpt-test"),
        Some(&vec!["high".to_string(), "ultra".to_string()])
    );
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn model_speed_overrides_and_display_order_are_persisted() {
    let root = test_root("model-speed-order");
    let path = root.join("relay.sqlite");
    let store = Store::open(path.clone()).unwrap();
    store
        .set_model_service_tier_overrides(BTreeMap::from([(
            " GPT-Test ".into(),
            DefaultServiceTier::Fast,
        )]))
        .unwrap();
    store
        .set_model_display_order(vec![" gpt-test ".into(), "other-model".into()])
        .unwrap();
    assert!(store
        .set_model_service_tier_overrides(BTreeMap::from([(
            "unsafe\nmodel".into(),
            DefaultServiceTier::Fast,
        )]))
        .is_err());
    drop(store);

    let reopened = Store::open(path).unwrap();
    assert_eq!(
        reopened.model_service_tier_overrides().unwrap(),
        BTreeMap::from([("gpt-test".to_string(), DefaultServiceTier::Fast)])
    );
    assert_eq!(
        reopened.model_display_order().unwrap(),
        vec!["gpt-test".to_string(), "other-model".to_string()]
    );
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_model_reasoning_default_is_read_as_a_single_allowed_level() {
    let root = test_root("legacy-model-reasoning");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    store
        .set_metadata(
            "model_reasoning_overrides",
            r#"{"gpt-test":"HIGH","automatic":"auto"}"#,
        )
        .unwrap();

    assert_eq!(
        store.model_reasoning_allowed_levels().unwrap(),
        BTreeMap::from([("gpt-test".to_string(), vec!["high".to_string()])])
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
