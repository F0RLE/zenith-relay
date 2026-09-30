use super::*;

#[test]
fn model_summaries_keep_member_exclusions_and_apply_pool_hidden_state() {
    let source = SourceSummary {
        resolved_protocol_bindings: None,
        id: "source_1".into(),
        name: "Synthetic".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        operational_status: OperationalStatus::Rotation,
        base_url: "https://example.test/v1".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_bindings: Vec::new(),
        protocol_config: crate::SourceProtocolConfig::default(),
        models: vec![
            "gpt-old".into(),
            "gpt-5.4-mini".into(),
            "gpt-5.4".into(),
            "gpt-future-codex".into(),
        ],
        allowed_models: Vec::new(),
        excluded_models: vec!["gpt-old".into()],
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        api_equivalent: ApiEquivalentSummary::default(),
        secret_available: true,
        last_error_code: None,
        refresh_revision: None,
        refresh_state: SourceRefreshState::default(),
        provider_stats: None,
    };

    let models = pool_model_summaries(&[source], &[], &["GPT-5.4-MINI".into()]);

    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["gpt-old", "gpt-5.4-mini", "gpt-5.4", "gpt-future-codex"]
    );
    assert!(models[0].enabled);
    assert!(!models[1].enabled);
    assert!(models[2].enabled);
    assert!(models[3].enabled);
    assert_eq!(models[2].member_count, 1);
    assert!(models[2].output_micro_usd_per_million.is_some());
    assert!(models[3].output_micro_usd_per_million.is_none());
}
#[test]
fn pool_inventory_enriches_unavailable_models_without_creating_routes() {
    let mut source = source_summary("source", &["future", "older", "unknown"]);
    source.enabled = false;
    source.secret_available = false;
    source.draining = true;
    source.excluded_models = vec!["*".into()];
    source.protocol_config = crate::SourceProtocolConfig::automatic(&source.base_url);
    source.protocol_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::Messages,
        adapter: SourceAdapter::Native,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: vec!["binding-only".into(), "FUTURE".into()],
    }];
    let mut outside = source_summary("outside", &["not-in-pool", "future"]);
    outside.in_pool = false;
    let mut account = account_summary(true, &["FUTURE", "account-only"]);
    account.enabled = false;
    account.secret_available = false;
    account.proxy_available = false;
    account.draining = true;
    account.excluded_models = vec!["*".into()];
    let sources = [source, outside];
    let accounts = [account, account_summary(false, &["outside-account"])];
    let metadata = ModelMetadataCatalog::from_models_dev_json(r#"{
            "synthetic/future": {"name":"Future Model","family":"test","release_date":"2026-01-01","reasoning_effort_levels":["low","high"]},
            "synthetic/older": {"name":"Older Model","family":"test","release_date":"2025-01-01"}
        }"#).unwrap();
    let mut models = pool_model_summaries(&sources, &accounts, &["future".into()]);
    apply_model_metadata(&mut models, &metadata);
    apply_pool_model_configuration(
        &mut models,
        &sources,
        &accounts,
        &BTreeMap::from([(
            "future".into(),
            ApiModelPriceOverride {
                input_micro_usd_per_million: 12,
                cached_input_micro_usd_per_million: Some(3),
                cache_write_5m_micro_usd_per_million: None,
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 34,
            },
        )]),
        &BTreeMap::from([("future".into(), vec!["high".into()])]),
        &BTreeMap::new(),
        None,
    );
    apply_model_display_order_with_catalog(&mut models, &[], &metadata);
    assert_eq!(models.len(), 5);
    let future = models.iter().find(|model| model.id == "future").unwrap();
    assert_eq!(future.member_count, 2);
    assert!(!future.enabled);
    assert_eq!(future.catalog_name.as_deref(), Some("Future Model"));
    assert_eq!(future.catalog_provider.as_deref(), Some("synthetic"));
    assert_eq!(future.input_micro_usd_per_million, Some(12));
    assert_eq!(future.output_micro_usd_per_million, Some(34));
    assert_eq!(future.reasoning_supported_levels, ["low", "high"]);
    assert_eq!(future.reasoning_allowed_levels, ["high"]);
    assert!(future.reasoning_configurable);
    assert!(models
        .iter()
        .all(|model| model.protocol_routes.is_empty() && !model.codex_visible));
    assert!(
        models.iter().position(|m| m.id == "future") < models.iter().position(|m| m.id == "older")
    );
    assert!(models
        .iter()
        .find(|m| m.id == "unknown")
        .unwrap()
        .reasoning_supported_levels
        .is_empty());
}
#[test]
fn pool_inventory_does_not_treat_saved_provider_modes_as_model_metadata() {
    let mut source = source_summary("source", &["future-model"]);
    source.secret_available = false;
    source.protocol_config = serde_json::from_value(serde_json::json!({
        "mode": "auto",
        "capabilities": [{
            "modelId": "future-model", "upstreamWireApi": "messages",
            "status": "declared", "origin": "catalog", "checkedAtMs": 1,
            "reasoningEfforts": ["high", "low"]
        }]
    }))
    .unwrap();
    let mut models = pool_model_summaries(std::slice::from_ref(&source), &[], &[]);
    apply_pool_model_configuration(
        &mut models,
        &[source],
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
    );
    assert!(models[0].reasoning_supported_levels.is_empty());
    assert!(models[0].reasoning_levels.is_empty());
    assert!(!models[0].reasoning_configurable);
    assert!(models[0].protocol_routes.is_empty());
}
#[test]
fn member_model_order_uses_complete_inventory_independently_of_rules() {
    let metadata = ModelMetadataCatalog::from_models_dev_json(
        r#"{
                "test/newer":{"release_date":"2026-01-01"},
                "test/older":{"release_date":"2025-01-01"},
                "test/catalog-only":{"release_date":"2026-02-01"}
            }"#,
    )
    .unwrap();
    let inventory = ["unknown-z", "older", "newer", "unknown-a"];
    let mut sources = vec![source_summary("source", &inventory)];
    let mut accounts = vec![account_summary(false, &inventory)];
    sources[0].excluded_models = vec!["newer".into()];
    sources[0].enabled = false;
    accounts[0].allowed_models = vec!["older".into()];
    let original_source = sources[0].clone();
    let original_account = accounts[0].clone();

    apply_member_model_display_order(&mut sources, &mut accounts, &[], &metadata);
    let expected = ["newer", "older", "unknown-a", "unknown-z"];
    assert_eq!(sources[0].models, expected);
    assert_eq!(accounts[0].models, expected);
    let mut expected_source = original_source;
    expected_source.models = expected.iter().map(ToString::to_string).collect();
    let mut expected_account = original_account;
    expected_account.models = expected_source.models.clone();
    assert_eq!(sources[0], expected_source);
    assert_eq!(accounts[0], expected_account);
    let identities = member_model_catalog(&sources, &accounts, &metadata);
    assert_eq!(identities.len(), 2);
    assert_eq!(identities["newer"].catalog_provider, "test");
    assert_eq!(identities["older"].catalog_provider, "test");
    assert!(!identities.contains_key("unknown-z"));
    assert!(!identities.contains_key("catalog-only"));

    sources[0].excluded_models.clear();
    accounts[0].excluded_models = vec!["older".into()];
    apply_member_model_display_order(&mut sources, &mut accounts, &[], &metadata);
    assert_eq!(sources[0].models, expected);
    assert_eq!(accounts[0].models, expected);

    apply_member_model_display_order(
        &mut sources,
        &mut accounts,
        &["stale".into(), "older".into(), "NEWER".into()],
        &metadata,
    );
    assert_eq!(
        sources[0].models,
        ["older", "newer", "unknown-a", "unknown-z"]
    );
    assert_eq!(accounts[0].models, sources[0].models);
}
#[test]
fn advisory_metadata_changes_only_presentation_fields_and_order() {
    let source = source_summary("source", &["older", "newer"]);
    let mut models = pool_model_summaries(std::slice::from_ref(&source), &[], &[]);
    let original_state = models
        .iter()
        .map(|model| {
            (
                model.id.clone(),
                (
                    model.enabled,
                    model.member_count,
                    model.input_micro_usd_per_million,
                    model.output_micro_usd_per_million,
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let metadata = ModelMetadataCatalog::from_models_dev_json(
        r#"{
                "test/newer":{
                    "name":"Newer",
                    "family":"test",
                    "release_date":"2026-01-01",
                    "reasoning":true,
                    "reasoning_effort_levels":["low","high"],
                    "default_reasoning_effort":"low",
                    "tool_call":true,
                    "structured_output":true,
                    "attachment":true,
                    "open_weights":false,
                    "modalities":{"input":["text","image"],"output":["text"]},
                    "limit":{"context":128000,"input":120000,"output":8000}
                },
                "test/older":{"name":"Older","family":"test","release_date":"2025-01-01"}
            }"#,
    )
    .unwrap();

    apply_model_metadata(&mut models, &metadata);
    apply_model_display_order_with_catalog(&mut models, &[], &metadata);

    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["newer", "older"]
    );
    assert_eq!(models[0].catalog_name.as_deref(), Some("Newer"));
    assert_eq!(models[0].catalog_reasoning, Some(true));
    assert_eq!(models[0].catalog_reasoning_effort_levels, ["low", "high"]);
    assert_eq!(
        models[0].catalog_default_reasoning_effort.as_deref(),
        Some("low")
    );
    assert_eq!(models[0].catalog_tool_call, Some(true));
    assert_eq!(models[0].catalog_structured_output, Some(true));
    assert_eq!(models[0].catalog_attachment, Some(true));
    assert_eq!(models[0].catalog_open_weights, Some(false));
    assert_eq!(models[0].catalog_input_modalities, ["text", "image"]);
    assert_eq!(models[0].catalog_output_modalities, ["text"]);
    assert_eq!(models[0].catalog_context_limit, Some(128_000));
    assert_eq!(models[0].catalog_input_limit, Some(120_000));
    assert_eq!(models[0].catalog_output_limit, Some(8_000));
    assert_eq!(
        models
            .iter()
            .map(|model| {
                (
                    model.id.clone(),
                    (
                        model.enabled,
                        model.member_count,
                        model.input_micro_usd_per_million,
                        model.output_micro_usd_per_million,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>(),
        original_state
    );
}
#[test]
fn refreshing_metadata_clears_removed_presentation_fields() {
    let source = source_summary("source", &["test/model"]);
    let mut models = pool_model_summaries(std::slice::from_ref(&source), &[], &[]);
    let metadata = ModelMetadataCatalog::from_models_dev_json(
        r#"{"test/model":{"name":"Catalog Name","family":"test","status":"active"}}"#,
    )
    .unwrap();

    apply_model_metadata(&mut models, &metadata);
    assert_eq!(models[0].catalog_name.as_deref(), Some("Catalog Name"));
    assert_eq!(models[0].codex_display_name, "Catalog Name");
    assert_eq!(models[0].catalog_provider.as_deref(), Some("test"));

    apply_model_metadata(&mut models, &ModelMetadataCatalog::empty());

    assert_eq!(models[0].catalog_provider, None);
    assert_eq!(models[0].codex_display_name, "Model");
    assert_eq!(models[0].catalog_family, None);
    assert_eq!(models[0].catalog_name, None);
    assert_eq!(models[0].catalog_release_date, None);
    assert_eq!(models[0].catalog_last_updated, None);
    assert_eq!(models[0].catalog_status, None);
}
#[test]
fn source_summary_projects_every_catalog_model_to_all_client_protocols() {
    let legacy = SourceSummary {
        resolved_protocol_bindings: None,
        id: "legacy".into(),
        name: "Legacy".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        operational_status: OperationalStatus::Rotation,
        base_url: "https://example.test/v1".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_bindings: Vec::new(),
        protocol_config: crate::SourceProtocolConfig::default(),
        models: vec!["gpt-legacy".into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        api_equivalent: ApiEquivalentSummary::default(),
        secret_available: true,
        last_error_code: None,
        refresh_revision: None,
        refresh_state: SourceRefreshState::default(),
        provider_stats: None,
    };
    assert_eq!(
        legacy.models_for_wire_api(WireApi::Responses),
        ["gpt-legacy"]
    );
    for wire_api in WireApi::ALL {
        assert_eq!(legacy.models_for_wire_api(wire_api), ["gpt-legacy"]);
        assert!(legacy.supports_wire_api(wire_api));
    }

    let mixed = SourceSummary {
        protocol_bindings: vec![
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-native".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-bridged".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-native".into()],
            },
        ],
        models: vec![
            "gpt-native".into(),
            "claude-bridged".into(),
            "claude-native".into(),
        ],
        ..legacy
    };
    for wire_api in WireApi::ALL {
        assert_eq!(
            mixed.models_for_wire_api(wire_api),
            ["gpt-native", "claude-bridged", "claude-native"]
        );
        assert!(mixed.supports_wire_api(wire_api));
    }
}
