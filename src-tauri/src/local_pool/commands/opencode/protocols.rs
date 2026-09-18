use super::*;
use std::collections::BTreeMap;

pub(super) const GROUPS: [(WireApi, &str, &str); 4] = [
    (WireApi::Responses, PROVIDER_ID, PROVIDER_NPM),
    (
        WireApi::ChatCompletions,
        "zenith-relay-chat",
        "@ai-sdk/openai-compatible",
    ),
    (
        WireApi::Messages,
        "zenith-relay-messages",
        "@ai-sdk/anthropic",
    ),
    (WireApi::Gemini, "zenith-relay-gemini", "@ai-sdk/google"),
];

pub(super) fn managed_id(id: &str) -> bool {
    GROUPS.iter().any(|(_, candidate, _)| id == *candidate)
}

pub(super) fn managed_model(model: &str) -> bool {
    model.split_once('/').is_some_and(|(id, _)| managed_id(id))
}

fn supports(model: &ModelSummary, protocol: WireApi) -> bool {
    model
        .protocol_routes
        .iter()
        .any(|route| route.client_wire_api == protocol)
}

pub(super) fn preferred(model: &ModelSummary) -> WireApi {
    WireApi::ALL
        .into_iter()
        .find(|protocol| {
            model.protocol_routes.iter().any(|route| {
                route.client_wire_api == *protocol && route.upstream_wire_api == *protocol
            })
        })
        .or_else(|| {
            WireApi::ALL
                .into_iter()
                .find(|protocol| supports(model, *protocol))
        })
        .unwrap_or(WireApi::Responses)
}

pub(super) fn base_url(base: &str, protocol: WireApi) -> Result<String, LocalPoolError> {
    let mut url = url::Url::parse(base)
        .map_err(|_| LocalPoolError::invalid_state("invalid provider address"))?;
    if protocol == WireApi::Gemini {
        let path = url.path().trim_end_matches('/');
        let prefix = path
            .strip_suffix("/v1")
            .or_else(|| path.strip_suffix("/v1beta"))
            .unwrap_or(path);
        url.set_path(&format!("{prefix}/v1beta"));
    }
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

pub(super) fn provider(
    base: &str,
    secret: &str,
    models: &[ModelSummary],
    protocol: WireApi,
) -> Result<Value, LocalPoolError> {
    let mut configured = model_config(models);
    for (id, value) in &mut configured {
        let model = models.iter().find(|model| model.id == *id).unwrap();
        if !model.protocol_routes.is_empty() {
            let routes = model
                .protocol_routes
                .iter()
                .filter(|route| route.client_wire_api == protocol)
                .collect::<Vec<_>>();
            if let Some(variants) = value.get_mut("variants").and_then(Value::as_object_mut) {
                variants.retain(|effort, _| {
                    routes
                        .iter()
                        .any(|route| route.reasoning_efforts.contains(effort))
                });
            }
            for (key, feature) in [
                (
                    "tool_call",
                    zenith_relay_core::ProtocolFeature::FunctionTools,
                ),
                ("reasoning", zenith_relay_core::ProtocolFeature::Reasoning),
            ] {
                if routes.iter().all(|route| {
                    route.features.get(&feature)
                        == Some(&zenith_relay_core::CapabilityStatus::Unsupported)
                }) {
                    value[key] = false.into();
                }
            }
        }
        set_variants(value, protocol);
    }
    provider_with_models(base, secret, configured, protocol)
}

fn set_variants(value: &mut Value, protocol: WireApi) {
    if let Some(variants) = value.get_mut("variants").and_then(Value::as_object_mut) {
        variants.retain(|effort, options| {
            let generated = match (protocol, effort.as_str()) {
                (WireApi::Messages, "none") => json!({"thinking":{"type":"disabled"}}),
                (WireApi::Messages, "low" | "medium" | "high" | "max") => {
                    json!({"thinking":{"type":"adaptive"},"effort":effort})
                }
                (WireApi::Gemini, "none") => json!({"thinkingConfig":{"thinkingBudget":0}}),
                (WireApi::Gemini, "minimal" | "low" | "medium" | "high") => {
                    json!({"thinkingConfig":{"thinkingLevel":effort}})
                }
                (WireApi::Messages | WireApi::Gemini, _) => return false,
                _ => json!({"reasoningEffort":effort}),
            };
            *options = generated;
            true
        });
    }
}

fn provider_with_models(
    base: &str,
    secret: &str,
    models: Map<String, Value>,
    protocol: WireApi,
) -> Result<Value, LocalPoolError> {
    let (_, _, npm) = GROUPS
        .iter()
        .find(|(wire, _, _)| *wire == protocol)
        .unwrap();
    Ok(
        json!({"npm":npm,"name":"Zenith Relay","options":{"baseURL":base_url(base, protocol)?,"apiKey":secret},"models":models}),
    )
}

/// Refresh Relay-owned endpoint and catalog fields while preserving user options.
fn merge_provider(previous: Option<&Value>, mut generated: Value) -> Value {
    if let Some(previous) = previous.and_then(Value::as_object) {
        for (key, value) in previous {
            if !matches!(key.as_str(), "npm" | "options" | "models") {
                generated[key] = value.clone();
            }
        }
        if let Some(options) = previous.get("options").and_then(Value::as_object) {
            for (key, value) in options {
                if !matches!(key.as_str(), "baseURL" | "apiKey") {
                    generated["options"][key] = value.clone();
                }
            }
        }
        if let Some(models) = previous.get("models").and_then(Value::as_object) {
            for (id, model) in generated["models"].as_object_mut().unwrap() {
                if let Some(saved) = models.get(id).and_then(Value::as_object) {
                    for (key, value) in saved {
                        if key == "variants" {
                            if let Some(variants) = value.as_object() {
                                for (name, options) in variants {
                                    let generated_variant = is_generated_variant(name, options);
                                    if !generated_variant {
                                        model
                                            .as_object_mut()
                                            .unwrap()
                                            .entry("variants")
                                            .or_insert_with(|| json!({}))[name] = options.clone();
                                    }
                                }
                            }
                        } else if matches!(key.as_str(), "options" | "name" | "limit" | "headers") {
                            model[key] = value.clone();
                        }
                    }
                }
            }
        }
    }
    generated
}

fn is_generated_variant(name: &str, options: &Value) -> bool {
    if ![
        "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
    ]
    .contains(&name)
    {
        return false;
    }
    [
        json!({"reasoningEffort": name}),
        json!({"effort": name}),
        json!({"thinking":{"type":"adaptive"},"effort":name}),
        json!({"thinkingConfig":{"thinkingLevel":name}}),
    ]
    .contains(options)
        || (name == "none"
            && [
                json!({"thinking":{"type":"disabled"}}),
                json!({"thinkingConfig":{"thinkingBudget":0}}),
            ]
            .contains(options))
}

fn existing_protocol(
    config: &Map<String, Value>,
    model: &str,
    supports: impl Fn(WireApi) -> bool,
) -> Option<WireApi> {
    let selected = config
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    GROUPS
        .iter()
        .find(|(protocol, id, _)| supports(*protocol) && selected == format!("{id}/{model}"))
        .or_else(|| {
            GROUPS.iter().find(|(protocol, id, _)| {
                supports(*protocol)
                    && config
                        .get("provider")
                        .and_then(|providers| providers.get(*id))
                        .and_then(|provider| provider.get("models"))
                        .and_then(Value::as_object)
                        .is_some_and(|models| models.contains_key(model))
            })
        })
        .map(|(protocol, _, _)| *protocol)
}

pub(super) fn apply(
    config: &mut Map<String, Value>,
    base: &str,
    secret: &str,
    models: &[ModelSummary],
) -> Result<(), LocalPoolError> {
    let mut groups = BTreeMap::<WireApi, Vec<ModelSummary>>::new();
    for model in models
        .iter()
        .filter(|model| model.enabled && !model.protocol_routes.is_empty())
    {
        // Keep the old provider/model identifier whenever its route still works.
        let previous_protocol =
            existing_protocol(config, &model.id, |protocol| supports(model, protocol));
        groups
            .entry(previous_protocol.unwrap_or_else(|| preferred(model)))
            .or_default()
            .push(model.clone());
    }
    let groups = GROUPS
        .iter()
        .map(|(protocol, _, _)| {
            provider(
                base,
                secret,
                &groups.remove(protocol).unwrap_or_default(),
                *protocol,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    merge_groups(config, groups, false)
}

pub(super) fn apply_source(
    config: &mut Map<String, Value>,
    source: &ProviderSourceRecord,
    secret: &str,
    metadata: &ModelMetadataCatalog,
    select_connection: bool,
) -> Result<(), LocalPoolError> {
    let routes = source
        .effective_protocol_bindings()
        .map_err(LocalPoolError::invalid_state)?;
    let mut groups = BTreeMap::<WireApi, Vec<String>>::new();
    let native = routes
        .into_iter()
        .filter(|binding| binding.adapter == SourceAdapter::Native)
        .collect::<Vec<_>>();
    let model_ids = zenith_relay_core::normalize_model_ids(
        native.iter().flat_map(|binding| binding.model_ids.iter()),
    );
    for model in model_ids {
        let protocol = existing_protocol(config, &model, |protocol| {
            native
                .iter()
                .any(|binding| binding.wire_api == protocol && binding.model_ids.contains(&model))
        })
        .unwrap_or_else(|| {
            native
                .iter()
                .find(|binding| binding.model_ids.contains(&model))
                .unwrap()
                .wire_api
        });
        groups.entry(protocol).or_default().push(model);
    }
    let capabilities = source.protocol_config.effective_capabilities(
        &source.base_url,
        &source.models,
        &source.protocol_bindings,
        source.wire_api,
    );
    let mut generated_groups = Vec::new();
    for (protocol, _, _) in GROUPS {
        let models = groups.remove(&protocol).unwrap_or_default();
        let mut configured = model_config_ids(&models, metadata);
        // Direct connections cannot execute Relay translations. The SDK must
        // speak the exact native protocol declared by this source.
        for (model, value) in &mut configured {
            if let Some(capability) = capabilities.iter().find(|capability| {
                capability.model_id.eq_ignore_ascii_case(model)
                    && capability.upstream_wire_api == protocol
            }) {
                use zenith_relay_core::{CapabilityStatus, ProtocolFeature};
                if let Some(variants) = value.get_mut("variants").and_then(Value::as_object_mut) {
                    variants.retain(|effort, _| {
                        capability.features.get(&ProtocolFeature::Reasoning)
                            != Some(&CapabilityStatus::Unsupported)
                            && (capability.reasoning_efforts.is_empty()
                                || capability.reasoning_efforts.contains(effort))
                    });
                }
                for (key, feature) in [
                    ("reasoning", ProtocolFeature::Reasoning),
                    ("tool_call", ProtocolFeature::FunctionTools),
                    ("attachment", ProtocolFeature::Images),
                ] {
                    if capability.features.get(&feature) == Some(&CapabilityStatus::Unsupported) {
                        value[key] = false.into();
                    }
                }
            }
            set_variants(value, protocol);
        }
        let generated = provider_with_models(&source.base_url, secret, configured, protocol)?;
        generated_groups.push(generated);
    }
    merge_groups(config, generated_groups, select_connection)
}

fn merge_groups(
    config: &mut Map<String, Value>,
    groups: Vec<Value>,
    select_connection: bool,
) -> Result<(), LocalPoolError> {
    config
        .entry("$schema")
        .or_insert_with(|| "https://opencode.ai/config.json".into());
    let selected = config
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let providers = config
        .entry("provider")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            LocalPoolError::invalid_state("OpenCode provider configuration must be an object")
        })?;
    let mut selections = Vec::new();
    for ((_, id, _), generated) in GROUPS.into_iter().zip(groups) {
        let models = generated["models"].as_object().unwrap();
        selections.extend(models.keys().map(|model| format!("{id}/{model}")));
        if models.is_empty() && id != PROVIDER_ID && !providers.contains_key(id) {
            continue;
        }
        providers.insert(id.into(), merge_provider(providers.get(id), generated));
    }
    if !selections.contains(&selected)
        && (select_connection || managed_model(&selected) || !selected.contains('/'))
    {
        if let Some(first) = selections.first() {
            config.insert("model".into(), first.clone().into());
        } else if managed_model(&selected) {
            config.remove("model");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::protocol::ModelProtocolRoute;

    fn model(id: &str, native: WireApi) -> ModelSummary {
        let mut model = super::super::tests::model(id, true);
        model.protocol_routes = WireApi::ALL
            .into_iter()
            .map(|client| ModelProtocolRoute {
                client_wire_api: client,
                upstream_wire_api: native,
                features: BTreeMap::new(),
                reasoning_efforts: vec!["high".into()],
            })
            .collect();
        model
    }

    #[test]
    fn groups_new_models_by_native_sdk_and_preserves_existing_selection_and_options() {
        let models = [
            model("open", WireApi::Responses),
            model("chat", WireApi::ChatCompletions),
            model("claude", WireApi::Messages),
            model("gemini", WireApi::Gemini),
        ];
        let mut config = Map::new();
        apply(
            &mut config,
            "http://127.0.0.1:14998/v1",
            "synthetic",
            &models,
        )
        .unwrap();
        for (_, id, npm) in GROUPS {
            assert_eq!(config["provider"][id]["npm"], npm);
            assert_eq!(
                config["provider"][id]["models"].as_object().unwrap().len(),
                1
            );
        }
        assert_eq!(
            config["provider"]["zenith-relay-gemini"]["options"]["baseURL"],
            "http://127.0.0.1:14998/v1beta"
        );
        config.insert("model".into(), "zenith-relay/claude".into());
        config.get_mut("provider").unwrap()["zenith-relay"]["models"]["claude"] =
            json!({"name":"My model","options":{"custom":true}});
        config.get_mut("provider").unwrap()["zenith-relay"]["options"]["timeout"] = 45000.into();
        apply(
            &mut config,
            "http://127.0.0.1:14998/v1",
            "synthetic",
            &models,
        )
        .unwrap();
        assert_eq!(config["model"], "zenith-relay/claude");
        assert_eq!(
            config["provider"]["zenith-relay"]["models"]["claude"]["name"],
            "My model"
        );
        assert_eq!(
            config["provider"]["zenith-relay"]["models"]["claude"]["options"]["custom"],
            true
        );
        assert_eq!(
            config["provider"]["zenith-relay"]["options"]["timeout"],
            45000
        );
    }

    #[test]
    fn sdk_variants_disable_thinking_without_forwarding_invalid_effort_names() {
        for (protocol, expected) in [
            (
                WireApi::Messages,
                json!({"none":{"thinking":{"type":"disabled"}},"high":{"thinking":{"type":"adaptive"},"effort":"high"},"max":{"thinking":{"type":"adaptive"},"effort":"max"}}),
            ),
            (
                WireApi::Gemini,
                json!({"none":{"thinkingConfig":{"thinkingBudget":0}},"minimal":{"thinkingConfig":{"thinkingLevel":"minimal"}},"high":{"thinkingConfig":{"thinkingLevel":"high"}}}),
            ),
        ] {
            let mut value =
                json!({"variants":{"none":{},"minimal":{},"high":{},"max":{},"ultra":{}}});
            set_variants(&mut value, protocol);
            assert_eq!(value["variants"], expected);
            let previous = json!({"models":{"model":{"variants":{"none":{"thinking":{"type":"adaptive"},"effort":"none"}}}}});
            let merged = merge_provider(Some(&previous), json!({"models":{"model":value}}));
            assert_eq!(merged["models"]["model"]["variants"], expected);
        }
    }

    #[test]
    fn direct_refresh_preserves_protocol_ids_and_external_selection_and_filters_capabilities() {
        use zenith_relay_core::{
            CapabilityOrigin, CapabilityStatus, ModelEndpointCapability, ProtocolFeature,
            SourceProtocolBinding,
        };
        let ids = vec!["claude".into(), "gpt-test".into()];
        let mut source = super::super::tests::source(vec![
            SourceProtocolBinding::legacy(WireApi::Responses, &ids),
            SourceProtocolBinding::legacy(WireApi::Messages, &ids),
        ]);
        source
            .protocol_config
            .capabilities
            .push(ModelEndpointCapability {
                model_id: "claude".into(),
                upstream_wire_api: WireApi::Messages,
                status: CapabilityStatus::Declared,
                origin: CapabilityOrigin::Catalog,
                checked_at_ms: 1,
                features: BTreeMap::from([(
                    ProtocolFeature::FunctionTools,
                    CapabilityStatus::Unsupported,
                )]),
                reasoning_efforts: vec!["none".into(), "high".into()],
            });
        let metadata = ModelMetadataCatalog::from_models_dev_json(r#"{
            "test/claude":{"reasoning":true,"reasoning_effort_levels":["none","low","high","ultra"],"tool_call":true}
        }"#).unwrap();
        let mut config = Map::from_iter([
            ("model".into(), "user-provider/selected".into()),
            (
                "provider".into(),
                json!({"zenith-relay-messages":{"models":{"claude":{},"gpt-test":{}}}}),
            ),
        ]);
        apply_source(&mut config, &source, "synthetic", &metadata, false).unwrap();
        assert_eq!(config["model"], "user-provider/selected");
        let models = &config["provider"]["zenith-relay-messages"]["models"];
        assert!(models.get("gpt-test").is_some());
        assert_eq!(models["claude"]["tool_call"], false);
        assert_eq!(
            models["claude"]["variants"],
            json!({
                "none":{"thinking":{"type":"disabled"}},
                "high":{"thinking":{"type":"adaptive"},"effort":"high"}
            })
        );
        assert!(config["provider"]["zenith-relay"]["models"]
            .as_object()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn unavailable_models_and_obsolete_generated_reasoning_variants_are_removed() {
        let mut supported = model("claude", WireApi::Messages);
        supported.catalog_provider = Some("anthropic".into());
        supported.catalog_reasoning = Some(true);
        supported.catalog_reasoning_effort_levels = vec!["high".into(), "ultra".into()];
        let mut unknown = model("unknown", WireApi::Responses);
        unknown.protocol_routes.clear();
        let mut config = Map::from_iter([(
            "provider".into(),
            json!({"zenith-relay-messages":{"models":{"claude":{"variants":{"ultra":{"effort":"ultra"},"custom":{"temperature":0.2},"deliberate":{"thinking":{"type":"enabled","budgetTokens":4096}}}}}}}),
        )]);
        apply(
            &mut config,
            "http://127.0.0.1:14998/v1",
            "synthetic",
            &[supported, unknown],
        )
        .unwrap();
        let models = &config["provider"]["zenith-relay-messages"]["models"];
        assert_eq!(models["claude"]["variants"]["high"]["effort"], "high");
        assert!(models["claude"]["variants"].get("ultra").is_none());
        assert_eq!(models["claude"]["variants"]["custom"]["temperature"], 0.2);
        assert_eq!(
            models["claude"]["variants"]["deliberate"]["thinking"]["budgetTokens"],
            4096
        );
        assert!(!config["provider"].to_string().contains("unknown"));
    }
}
