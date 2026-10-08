use super::*;

pub(super) fn set_variants(model_config: &mut Value, protocol: WireApi) {
    if let Some(variants) = model_config
        .get_mut("variants")
        .and_then(Value::as_object_mut)
    {
        variants.retain(|effort, variant_config| {
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
            *variant_config = generated;
            true
        });
    }
}

pub(super) fn provider_with_models(
    relay_base_url: &str,
    api_key: &str,
    models: Map<String, Value>,
    protocol: WireApi,
) -> Result<Value, LocalPoolError> {
    let (_, _, npm) = GROUPS
        .iter()
        .find(|(wire, _, _)| *wire == protocol)
        .unwrap();
    Ok(
        json!({"npm":npm,"name":"Zenith Relay","options":{"baseURL":base_url(relay_base_url, protocol)?,"apiKey":api_key},"models":models}),
    )
}

/// Refresh Relay-owned endpoint and catalog fields while preserving user options.
pub(super) fn merge_provider(previous_provider: Option<&Value>, mut generated: Value) -> Value {
    if let Some(previous_fields) = previous_provider.and_then(Value::as_object) {
        for (field_name, field_value) in previous_fields {
            if !matches!(field_name.as_str(), "npm" | "options" | "models") {
                generated[field_name] = field_value.clone();
            }
        }
        if let Some(saved_options) = previous_fields.get("options").and_then(Value::as_object) {
            for (option_name, option_value) in saved_options {
                if !matches!(option_name.as_str(), "baseURL" | "apiKey") {
                    generated["options"][option_name] = option_value.clone();
                }
            }
        }
        if let Some(saved_models) = previous_fields.get("models").and_then(Value::as_object) {
            for (model_id, model_config) in generated["models"].as_object_mut().unwrap() {
                if let Some(saved_model) = saved_models.get(model_id).and_then(Value::as_object) {
                    for (field_name, field_value) in saved_model {
                        if field_name == "variants" {
                            if let Some(saved_variants) = field_value.as_object() {
                                for (variant_name, variant_config) in saved_variants {
                                    let generated_variant =
                                        is_generated_variant(variant_name, variant_config);
                                    if !generated_variant {
                                        model_config
                                            .as_object_mut()
                                            .unwrap()
                                            .entry("variants")
                                            .or_insert_with(|| json!({}))[variant_name] =
                                            variant_config.clone();
                                    }
                                }
                            }
                        } else if matches!(
                            field_name.as_str(),
                            "options" | "name" | "limit" | "headers"
                        ) {
                            model_config[field_name] = field_value.clone();
                        }
                    }
                }
            }
        }
    }
    generated
}

pub(super) fn is_generated_variant(variant_name: &str, variant_config: &Value) -> bool {
    if ![
        "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
    ]
    .contains(&variant_name)
    {
        return false;
    }
    [
        json!({"reasoningEffort": variant_name}),
        json!({"effort": variant_name}),
        json!({"thinking":{"type":"adaptive"},"effort":variant_name}),
        json!({"thinkingConfig":{"thinkingLevel":variant_name}}),
    ]
    .contains(variant_config)
        || (variant_name == "none"
            && [
                json!({"thinking":{"type":"disabled"}}),
                json!({"thinkingConfig":{"thinkingBudget":0}}),
            ]
            .contains(variant_config))
}

pub(super) fn existing_protocol(
    config: &Map<String, Value>,
    model: &str,
    supports: impl Fn(WireApi) -> bool,
) -> Option<WireApi> {
    let selected_model = config
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    GROUPS
        .iter()
        .find(|(protocol, id, _)| supports(*protocol) && selected_model == format!("{id}/{model}"))
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

pub(super) fn merge_groups(
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
    for ((_, id, _), generated_provider) in GROUPS.into_iter().zip(groups) {
        let models = generated_provider["models"].as_object().unwrap();
        selections.extend(models.keys().map(|model_id| format!("{id}/{model_id}")));
        if models.is_empty() && id != PROVIDER_ID && !providers.contains_key(id) {
            continue;
        }
        providers.insert(
            id.into(),
            merge_provider(providers.get(id), generated_provider),
        );
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
