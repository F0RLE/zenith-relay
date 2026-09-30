use super::*;

pub(in crate::protocol::adapter::messages::request) fn translate_tool_choice(
    tool_choice: &Value,
    state: &MessagesBridgeState,
) -> AdapterResult<TranslatedToolChoice> {
    let has_tools = state.upstream_tools().is_some();
    match tool_choice {
        Value::String(value) => match value.as_str() {
            "auto" => Ok(TranslatedToolChoice {
                value: has_tools.then(|| json!({"type": "auto"})),
                allowed_names: None,
            }),
            "none" => Ok(TranslatedToolChoice {
                value: has_tools.then(|| json!({"type": "none"})),
                allowed_names: None,
            }),
            "required" if has_tools => Ok(TranslatedToolChoice {
                value: Some(json!({"type": "any"})),
                allowed_names: None,
            }),
            _ => Err(AdapterError::unsupported_tool()),
        },
        Value::Object(value)
            if matches!(
                value.get("type").and_then(Value::as_str),
                Some("function" | "custom")
            ) =>
        {
            let Some(name) = state.selected_upstream_tool_name(value) else {
                return Err(AdapterError::unsupported_tool());
            };
            Ok(TranslatedToolChoice {
                value: Some(json!({"type": "tool", "name": name})),
                allowed_names: None,
            })
        }
        Value::Object(value) if value.get("type").and_then(Value::as_str) == Some("namespace") => {
            let namespace = value
                .get("name")
                .or_else(|| value.get("namespace"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|namespace| !namespace.is_empty())
                .ok_or_else(AdapterError::unsupported_tool)?;
            let allowed_names = state
                .tool_targets
                .iter()
                .filter_map(|(upstream_name, target)| {
                    (target.namespace.as_deref() == Some(namespace)
                        && state.allows_tool_name(upstream_name))
                    .then_some(upstream_name.clone())
                })
                .collect::<BTreeSet<_>>();
            if allowed_names.is_empty() {
                return Ok(TranslatedToolChoice {
                    value: None,
                    allowed_names: None,
                });
            }
            Ok(TranslatedToolChoice {
                value: Some(json!({"type": "any"})),
                allowed_names: Some(allowed_names),
            })
        }
        Value::Object(value)
            if value.get("type").and_then(Value::as_str) == Some("allowed_tools") =>
        {
            let Some(configured_tools) = value.get("tools").and_then(Value::as_array) else {
                return Ok(TranslatedToolChoice {
                    value: None,
                    allowed_names: None,
                });
            };
            let mut allowed_names = BTreeSet::new();
            for tool in configured_tools {
                let Some(tool) = tool.as_object() else {
                    continue;
                };
                match tool.get("type").and_then(Value::as_str) {
                    Some("function" | "custom") => {
                        if let Some(name) = state.selected_upstream_tool_name(tool) {
                            allowed_names.insert(name);
                        }
                    }
                    Some("namespace") => {
                        let namespace = tool
                            .get("name")
                            .or_else(|| tool.get("namespace"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|namespace| !namespace.is_empty())
                            .unwrap_or_default();
                        if namespace.is_empty() {
                            continue;
                        }
                        allowed_names.extend(state.tool_targets.iter().filter_map(
                            |(upstream_name, target)| {
                                (target.namespace.as_deref() == Some(namespace)
                                    && state.allows_tool_name(upstream_name))
                                .then_some(upstream_name.clone())
                            },
                        ));
                    }
                    // A hosted-only tool cannot be selected through a
                    // Messages bridge. Keep the representable client tools in
                    // an allowed-tools set; fail below when none remain.
                    _ => {}
                }
            }
            if allowed_names.is_empty() {
                return Ok(TranslatedToolChoice {
                    value: None,
                    allowed_names: None,
                });
            }
            let value = match value.get("mode").and_then(Value::as_str).unwrap_or("auto") {
                "auto" => json!({"type": "auto"}),
                "required" => json!({"type": "any"}),
                _ => {
                    return Ok(TranslatedToolChoice {
                        value: None,
                        allowed_names: None,
                    })
                }
            };
            Ok(TranslatedToolChoice {
                value: Some(value),
                allowed_names: Some(allowed_names),
            })
        }
        Value::Null => Ok(TranslatedToolChoice {
            value: None,
            allowed_names: None,
        }),
        _ => Err(AdapterError::unsupported_tool()),
    }
}
