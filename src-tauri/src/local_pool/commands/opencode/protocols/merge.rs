use super::*;

pub(super) fn set_variants(value: &mut Value, protocol: WireApi) {
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

pub(super) fn provider_with_models(
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
pub(super) fn merge_provider(previous: Option<&Value>, mut generated: Value) -> Value {
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

pub(super) fn is_generated_variant(name: &str, options: &Value) -> bool {
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

pub(super) fn existing_protocol(
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
