use super::{
    apply_managed_provider, managed_provider, model_ids, normalize_snapshot_name, parse_jsonc,
    remove_managed_configuration, serialize_config, source_opencode_models, ProviderSourceRecord,
    SourceAdapter, PROVIDER_ID, PROVIDER_NPM,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use zenith_relay_core::{protocol::ModelSummary, SourceProtocolBinding, WireApi};

pub(super) fn source(bindings: Vec<SourceProtocolBinding>) -> ProviderSourceRecord {
    ProviderSourceRecord {
        id: "source".into(),
        name: "Source".into(),
        enabled: true,
        in_pool: false,
        draining: false,
        base_url: "https://provider.example/v1".into(),
        secret_ref: "source:test".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: bindings,
        models: vec![
            "gpt-test".into(),
            "gpt-other".into(),
            "chat-only".into(),
            "claude".into(),
        ],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        last_used_at: None,
        last_test_at: None,
        last_test_status: None,
        last_error: None,
    }
}

pub(super) fn model(id: &str, enabled: bool) -> ModelSummary {
    ModelSummary {
        id: id.into(),
        protocol_routes: vec![zenith_relay_core::protocol::ModelProtocolRoute {
            client_wire_api: WireApi::Responses,
            upstream_wire_api: WireApi::Responses,
            features: BTreeMap::new(),
            reasoning_efforts: Vec::new(),
        }],
        enabled,
        member_count: 1,
        codex_visible: enabled,
        codex_display_name: id.into(),
        catalog_provider: None,
        catalog_source_model_id: None,
        catalog_canonical_model_id: None,
        catalog_family: None,
        catalog_name: None,
        catalog_release_date: None,
        catalog_last_updated: None,
        catalog_status: None,
        catalog_reasoning: None,
        catalog_reasoning_method: None,
        catalog_reasoning_effort_levels: Vec::new(),
        catalog_default_reasoning_effort: None,
        catalog_reasoning_budget_min_tokens: None,
        catalog_reasoning_budget_max_tokens: None,
        catalog_reasoning_budget_default_tokens: None,
        catalog_tool_call: None,
        catalog_structured_output: None,
        catalog_attachment: None,
        catalog_open_weights: None,
        catalog_input_modalities: Vec::new(),
        catalog_output_modalities: Vec::new(),
        catalog_context_limit: None,
        catalog_input_limit: None,
        catalog_output_limit: None,
        input_micro_usd_per_million: None,
        cached_input_micro_usd_per_million: None,
        cache_write_5m_micro_usd_per_million: None,
        cache_write_1h_micro_usd_per_million: None,
        output_micro_usd_per_million: None,
        image_request_prices: Vec::new(),
        custom_price: false,
        reasoning_levels: Vec::new(),
        reasoning_supported_levels: Vec::new(),
        reasoning_allowed_levels: Vec::new(),
        reasoning_configurable: false,
        reasoning_manual_fallback: false,
        speed_supported: false,
        speed_tiers: Vec::new(),
        speed_tier: Default::default(),
        speed_configurable: false,
    }
}

#[test]
fn parses_comments_and_trailing_commas_without_changing_strings() {
    let value = parse_jsonc(
        r#"{
                // keep this URL exactly as written
                "url": "https://relay.example/v1//chat",
                "models": ["one", "two",], /* trailing comma */
            }"#,
    )
    .unwrap();
    assert_eq!(value["url"], "https://relay.example/v1//chat");
    assert_eq!(value["models"][1], "two");
}

#[test]
fn keeps_prepared_catalog_order_and_excludes_disabled_models() {
    let models = vec![
        model("claude-opus-4-8", true),
        model("gpt-5.4", false),
        model("gemini-2.5-pro", true),
    ];
    assert_eq!(
        model_ids(&models)
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["claude-opus-4-8", "gemini-2.5-pro"]
    );
}

#[test]
fn configures_opencode_for_responses_tool_calls() {
    let provider = managed_provider(
        "http://127.0.0.1:14998/v1",
        "test-secret",
        &[model("gpt-5.6-sol", true)],
    );

    assert_eq!(provider["npm"], PROVIDER_NPM);
    assert_eq!(provider["options"]["baseURL"], "http://127.0.0.1:14998/v1");
    assert_eq!(provider["models"]["gpt-5.6-sol"]["attachment"], true);
    assert_eq!(
        provider["models"]["gpt-5.6-sol"]["modalities"]["input"],
        json!(["text", "image"])
    );
}

#[test]
fn model_capabilities_are_shared_by_pool_and_direct_source_configs() {
    let metadata = super::ModelMetadataCatalog::from_models_dev_json(
        r#"{
            "test/text-only": {"reasoning": true, "reasoning_effort_levels": ["low", "high"],
            "attachment": false, "tool_call": true,
            "modalities": {"input": ["text"], "output": ["text"]},
            "limit": {"context": 32000, "input": 24000, "output": 8000}}
        }"#,
    )
    .unwrap();
    let mut models = vec![model("text-only", true), model("unknown", true)];
    zenith_relay_core::protocol::apply_model_metadata(&mut models, &metadata);
    let pooled = super::model_config(&models);
    let direct = super::model_config_ids(&["text-only".into(), "unknown".into()], &metadata);
    assert_eq!(pooled, direct);
    assert_eq!(pooled["text-only"]["attachment"], false);
    assert_eq!(pooled["text-only"]["modalities"]["input"], json!(["text"]));
    assert_eq!(pooled["text-only"]["tool_call"], true);
    assert_eq!(pooled["text-only"]["limit"]["context"], 32000);
    assert_eq!(
        pooled["unknown"]["modalities"]["input"],
        json!(["text", "image"])
    );
    assert_eq!(pooled["unknown"]["reasoning"], false);
    assert_eq!(pooled["unknown"]["tool_call"], true);
    assert!(pooled["unknown"].get("limit").is_none());
    models[0].reasoning_configurable = true;
    models[0].reasoning_allowed_levels = vec!["high".into(), "max".into()];
    let filtered = super::model_config(&models);
    assert_eq!(
        filtered["text-only"]["variants"],
        json!({"high": {"reasoningEffort": "high"}})
    );

    models[0].reasoning_supported_levels = vec!["low".into(), "high".into(), "ultra".into()];
    models[0].reasoning_allowed_levels = vec!["high".into(), "ultra".into()];
    let native = super::model_config(&models);
    assert_eq!(
        native["text-only"]["variants"],
        json!({
            "high": {"reasoningEffort": "high"},
            "ultra": {"reasoningEffort": "ultra"}
        })
    );
}

#[test]
fn direct_source_models_include_all_native_protocols() {
    let source = source(vec![
        SourceProtocolBinding::legacy(
            WireApi::Responses,
            &["gpt-test".into(), "gpt-other".into(), "gpt-test".into()],
        ),
        SourceProtocolBinding::legacy(WireApi::ChatCompletions, &["chat-only".into()]),
    ]);

    assert_eq!(
        source_opencode_models(&source).unwrap(),
        ["gpt-test", "gpt-other", "chat-only", "claude"]
    );
}

#[test]
fn direct_source_models_use_the_native_upstream_of_legacy_bridges() {
    let mut binding = SourceProtocolBinding::legacy(WireApi::Responses, &["claude".into()]);
    binding.adapter = SourceAdapter::ResponsesToMessages;
    let source = source(vec![binding]);
    assert_eq!(
        source_opencode_models(&source).unwrap(),
        ["gpt-test", "gpt-other", "chat-only", "claude"]
    );
    assert!(source
        .effective_protocol_bindings()
        .unwrap()
        .iter()
        .any(|route| {
            route.wire_api == WireApi::Messages && route.adapter == SourceAdapter::Native
        }));
}

#[test]
fn empty_catalog_replaces_stale_models_without_touching_other_providers() {
    let mut config = serde_json::from_value(json!({
        "model": format!("{PROVIDER_ID}/stale-model"),
        "provider": {
            PROVIDER_ID: { "models": { "stale-model": {} } },
            "anthropic": { "name": "Claude" }
        }
    }))
    .unwrap();

    apply_managed_provider(&mut config, "http://127.0.0.1:14998/v1", "test-secret", &[]).unwrap();

    assert_eq!(
        config["provider"][PROVIDER_ID]["models"],
        Value::Object(Map::new())
    );
    assert!(config["provider"].get("anthropic").is_some());
    assert!(config.get("model").is_none());
}

#[test]
fn accepts_a_trimmed_snapshot_name_and_rejects_invalid_values() {
    assert_eq!(
        normalize_snapshot_name(" Before switching ").unwrap(),
        "Before switching"
    );
    assert!(normalize_snapshot_name("").is_err());
    assert!(normalize_snapshot_name("bad\nname").is_err());
    assert!(normalize_snapshot_name(&"x".repeat(81)).is_err());
}

#[test]
fn removing_a_missing_original_only_removes_relay_configuration() {
    let mut config = serde_json::from_value(json!({
        "model": format!("{PROVIDER_ID}/gpt-5.6-sol"),
        "provider": {
            PROVIDER_ID: { "name": "Relay" },
            "anthropic": { "name": "Claude" }
        },
        "theme": "dark"
    }))
    .unwrap();

    assert!(remove_managed_configuration(&mut config));
    assert_eq!(config.get("model"), None);
    assert_eq!(config.get("theme"), Some(&json!("dark")));
    assert!(config
        .get("provider")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|providers| providers.contains_key("anthropic")));
    assert!(!config
        .get("provider")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|providers| providers.contains_key(PROVIDER_ID)));
}

#[test]
fn serializes_opencode_variants_in_reasoning_order() {
    let config = json!({
        "provider": {
            "zenith-relay": {
                "models": {
                    "claude-opus-5-5": {
                        "variants": {
                            "high": {"reasoningEffort": "high"},
                            "custom": {"temperature": 0.2},
                            "low": {"reasoningEffort": "low"},
                            "max": {"reasoningEffort": "max"},
                            "ultra": {"reasoningEffort": "ultra"},
                            "medium": {"reasoningEffort": "medium"},
                            "none": {"reasoningEffort": "none"},
                            "xhigh": {"reasoningEffort": "xhigh"},
                            "alpha": {"temperature": 0.1},
                            "minimal": {"reasoningEffort": "minimal"}
                        }
                    }
                }
            }
        }
    });
    let serialized = serialize_config(config.as_object().unwrap()).unwrap();
    assert_eq!(
        object_keys_after(&serialized, "variants"),
        ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra", "alpha", "custom"]
    );

    let plain = json!({"b": 1, "a": {"z": true, "m": [1, {"q": 2, "p": 3}]}});
    assert_eq!(
        serialize_config(plain.as_object().unwrap()).unwrap(),
        serde_json::to_string_pretty(&plain).unwrap()
    );
}

fn object_keys_after(json: &str, name: &str) -> Vec<String> {
    let marker = format!("\"{name}\"");
    let marker_at = json.find(&marker).unwrap();
    let object_at = json[marker_at + marker.len()..].find('{').unwrap() + marker_at + marker.len();
    let mut keys = Vec::new();
    let mut depth = 0_usize;
    let mut index = object_at;
    let bytes = json.as_bytes();
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let mut end = index + 1;
                let mut escaped = false;
                while end < bytes.len() {
                    match bytes[end] {
                        b'\\' if !escaped => escaped = true,
                        b'"' if !escaped => break,
                        _ => escaped = false,
                    }
                    end += 1;
                }
                let token = json[index + 1..end].to_owned();
                let mut after = end + 1;
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
                if depth == 1 && after < bytes.len() && bytes[after] == b':' {
                    keys.push(token);
                }
                index = end + 1;
            }
            b'{' | b'[' => {
                depth += 1;
                index += 1;
            }
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
    keys
}
