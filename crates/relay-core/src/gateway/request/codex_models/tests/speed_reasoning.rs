use super::*;
use serde_json::json;

#[test]
fn relay_speed_policy_overrides_missing_empty_and_conflicting_account_fields() {
    use crate::gateway::request::normalization::ServiceTierPolicy;

    let models = ["gpt-future", "gpt-another-synthetic"];
    let runtime =
        native_catalog_test_runtime_with_accounts(None, None, &["native-account"], &models);
    let key = runtime.authenticate_secret("secret").unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
    for fields in [
        json!({}),
        json!({"service_tiers": [], "additional_speed_tiers": []}),
        json!({"service_tiers": null, "additional_speed_tiers": "invalid"}),
        json!({"service_tiers": [{"id":"unrelated"}], "default_service_tier": "unrelated"}),
    ] {
        let sparse = json!({"models": models.iter().map(|id| {
            let mut row = fields.clone();
            row["slug"] = json!(id);
            row
        }).collect::<Vec<_>>()});
        runtime.remember_codex_model_manifest("native-account", sparse.clone(), now_ms());
        let response = build_codex_models_response_from_manifests(
            &runtime,
            &key,
            &visible,
            [("native-account".into(), sparse)],
        )
        .unwrap();
        assert_eq!(response["models"].as_array().unwrap().len(), models.len());
        for model in response["models"].as_array().unwrap() {
            let id = model["slug"].as_str().unwrap();
            assert!(models.contains(&id));
            assert_eq!(model["service_tiers"][0]["id"], "priority");
            assert_eq!(model["service_tiers"][1]["id"], "ultrafast");
            assert_eq!(
                model["additional_speed_tiers"],
                json!(["fast", "ultrafast"])
            );
            assert!(model.get("default_service_tier").is_none());
            let mut request = json!({"model": id, "service_tier": "ultrafast"});
            let policy = ServiceTierPolicy::pool_owned(&request);
            let selected = policy.select_for_model(&runtime, id);
            assert_eq!(selected, DefaultServiceTier::Ultrafast);
            policy.prepare_for_candidate(&mut request, selected, WireApi::Responses);
            assert_eq!(request["service_tier"], "ultrafast");
        }
    }
}

#[test]
fn basis_points_account_still_publishes_model_speed_tiers() {
    let models = ["gpt-future"];
    let runtime =
        native_catalog_test_runtime_with_accounts(None, None, &["basis-account"], &models);
    runtime.set_basis_points_enabled(true);
    let key = runtime.authenticate_secret("secret").unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
    let manifest = json!({
        "models": [{
            "slug": "gpt-future",
            "service_tiers": [{"id": "priority"}],
            "additional_speed_tiers": ["fast"]
        }]
    });
    let response = build_codex_models_response_from_manifests(
        &runtime,
        &key,
        &visible,
        [("basis-account".into(), manifest)],
    )
    .unwrap();
    let model = &response["models"][0];
    assert_eq!(model["service_tiers"][0]["id"], "priority");
    assert_eq!(model["service_tiers"][1]["id"], "ultrafast");
    assert_eq!(
        model["additional_speed_tiers"],
        json!(["fast", "ultrafast"])
    );
}

#[test]
fn installed_codex_ultra_reaches_the_live_catalog_without_an_account_card() {
    use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};
    use std::collections::BTreeMap;

    let catalog = ModelMetadataCatalog::from_models_dev_json(
        r#"{"gpt-native":{"reasoning":true,"reasoning_effort_levels":["low","xhigh","max"]}}"#,
    )
    .unwrap();
    let runtime = native_catalog_test_runtime_with_accounts(
        None,
        Some(ModelMetadataCatalogHandle::new(catalog)),
        &["native-account"],
        &["gpt-native"],
    );
    runtime.set_official_codex_ultra_models(BTreeMap::from([(
        "gpt-native".to_string(),
        json!({
            "slug": "gpt-native",
            "supported_reasoning_levels": [{"effort": "ultra"}],
            "multi_agent_version": "v2",
            "multi_agent_reasoning_effort": "xhigh"
        }),
    )]));
    let key = runtime.authenticate_secret("secret").unwrap();
    let visible = runtime.visible_models(&key, &[WireApi::Responses], 0);
    let response = build_codex_models_response_from_manifests(
        &runtime,
        &key,
        &visible,
        [(
            "native-account".into(),
            json!({"models": [{"slug": "gpt-native"}]}),
        )],
    )
    .unwrap();
    let levels = response["models"][0]["supported_reasoning_levels"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|level| level["effort"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(levels, ["low", "xhigh", "max", "ultra"]);
    assert_eq!(response["models"][0]["multi_agent_version"], "v2");
    assert_eq!(
        response["models"][0]["multi_agent_reasoning_effort"],
        "xhigh"
    );
}

#[test]
fn manual_reasoning_modes_prefer_medium_over_provider_ultra_default() {
    let mut model = json!({
        "default_reasoning_level": "ultra",
        "supported_reasoning_levels": [
            {"effort": "low"},
            {"effort": "medium"},
            {"effort": "ultra"}
        ]
    });

    apply_model_reasoning_allowed_levels(
        &mut model,
        Some(&["low".to_string(), "medium".to_string(), "ultra".to_string()]),
    );

    assert_eq!(model["default_reasoning_level"], "medium");
}

#[test]
fn manual_reasoning_modes_do_not_keep_provider_ultra_default_without_medium() {
    let mut model = json!({
        "default_reasoning_level": "ultra",
        "supported_reasoning_levels": [
            {"effort": "low"},
            {"effort": "high"},
            {"effort": "ultra"}
        ]
    });

    apply_model_reasoning_allowed_levels(
        &mut model,
        Some(&["low".to_string(), "high".to_string(), "ultra".to_string()]),
    );

    assert!(model.get("default_reasoning_level").is_none());
}

#[test]
fn manual_reasoning_modes_allow_provider_specific_efforts() {
    let mut model = json!({
        "default_reasoning_level": "low",
        "supported_reasoning_levels": [
            {"effort": "low", "description": "Provider low"}
        ]
    });

    apply_model_reasoning_allowed_levels(
        &mut model,
        Some(&["low".to_string(), "xhigh".to_string(), "max".to_string()]),
    );

    assert_eq!(
        model["supported_reasoning_levels"],
        json!([
            {"effort": "low", "description": "Provider low"},
            {"effort": "xhigh", "description": "xhigh"},
            {"effort": "max", "description": "max"}
        ])
    );
    assert!(model.get("default_reasoning_level").is_none());
}

#[test]
fn no_manual_override_preserves_provider_reasoning_modes() {
    let mut model = json!({
        "default_reasoning_level": "ultra",
        "supported_reasoning_levels": [
            {"effort": "low"},
            {"effort": "high"},
            {"effort": "ultra"}
        ]
    });

    apply_model_reasoning_allowed_levels(&mut model, None);

    assert_eq!(
        model["supported_reasoning_levels"],
        json!([{"effort": "low"}, {"effort": "high"}, {"effort": "ultra"}])
    );
    assert_eq!(model["default_reasoning_level"], "ultra");
}

#[test]
fn missing_reasoning_metadata_stays_empty_until_catalog_evidence_exists() {
    let model = json!({"supported_reasoning_levels": []});
    assert_eq!(model["supported_reasoning_levels"], json!([]));
    assert!(model.get("default_reasoning_level").is_none());
}

#[test]
fn provider_empty_reasoning_metadata_is_not_replaced_by_known_defaults() {
    let model = json!({"supported_reasoning_levels": []});

    // The caller skips this fallback when the provider explicitly sent an
    // empty field; the helper itself remains a no-op for unknown models.

    assert_eq!(model["supported_reasoning_levels"], json!([]));
}
