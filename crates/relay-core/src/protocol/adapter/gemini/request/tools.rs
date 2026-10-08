//! Responses tool definitions translated into Gemini function declarations.

use super::{
    bridged_namespace_tool_name, AdapterError, AdapterResult, ClientToolTarget,
    MessagesBridgeState, ResponsesToolKind,
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn translate_tools(
    tools: &[Value],
) -> AdapterResult<(Vec<Value>, BTreeMap<String, ClientToolTarget>)> {
    let mut declarations = Vec::new();
    let mut targets = BTreeMap::new();
    for tool_value in tools {
        let tool = tool_value
            .as_object()
            .ok_or_else(AdapterError::invalid_request)?;
        match tool.get("type").and_then(Value::as_str) {
            Some("namespace") => {
                let namespace = tool
                    .get("name")
                    .or_else(|| tool.get("namespace"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|namespace_name| !namespace_name.is_empty())
                    .ok_or_else(AdapterError::invalid_request)?;
                let Some(children) = tool.get("tools").and_then(Value::as_array) else {
                    continue;
                };
                for child in children {
                    let Some(child) = child.as_object() else {
                        continue;
                    };
                    if matches!(
                        child.get("type").and_then(Value::as_str),
                        Some("function" | "custom") | None
                    ) {
                        translate_gemini_tool(
                            &mut declarations,
                            &mut targets,
                            child,
                            Some(namespace),
                        )?;
                    }
                }
            }
            Some("function" | "custom") | None if tool.get("name").is_some() => {
                translate_gemini_tool(&mut declarations, &mut targets, tool, None)?;
            }
            _ => return Err(AdapterError::unsupported_tool()),
        }
    }
    Ok((declarations, targets))
}

fn translate_gemini_tool(
    declarations: &mut Vec<Value>,
    targets: &mut BTreeMap<String, ClientToolTarget>,
    tool: &Map<String, Value>,
    namespace: Option<&str>,
) -> AdapterResult<()> {
    let tool_kind = match tool.get("type").and_then(Value::as_str) {
        Some("custom") => ResponsesToolKind::Custom,
        Some("function") | None => ResponsesToolKind::Function,
        _ => return Ok(()),
    };
    let tool_name = tool
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|tool_name| !tool_name.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    let upstream_name = namespace
        .map(|namespace| bridged_namespace_tool_name(namespace, tool_name))
        .unwrap_or_else(|| tool_name.to_string());
    if targets.contains_key(&upstream_name) {
        return Err(AdapterError::invalid_request());
    }
    let mut declaration =
        Map::from_iter([("name".to_string(), Value::String(upstream_name.clone()))]);
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|description_text| !description_text.is_empty());
    if let Some(namespace) = namespace {
        let mut description_value = format!("Codex namespace `{namespace}` tool `{tool_name}`.");
        if let Some(description) = description {
            description_value.push(' ');
            description_value.push_str(description);
        }
        declaration.insert("description".to_string(), Value::String(description_value));
    } else if let Some(description) = description {
        declaration.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
    }
    let parameters = if tool_kind == ResponsesToolKind::Custom {
        json!({"type":"object","properties":{"input":{"type":"string"}},"required":["input"]})
    } else {
        tool.get("parameters")
            .or_else(|| tool.get("parameters_json_schema"))
            .or_else(|| tool.get("input_schema"))
            .cloned()
            .unwrap_or_else(|| json!({"type":"object","properties":{}}))
    };
    if !parameters.is_object() {
        return Err(AdapterError::invalid_request());
    }
    if tool.get("strict").and_then(Value::as_bool) == Some(true) {
        return Err(AdapterError::parameter_unsupported());
    }
    declaration.insert("parametersJsonSchema".to_string(), parameters);
    declarations.push(Value::Object(declaration));
    targets.insert(
        upstream_name,
        ClientToolTarget {
            kind: tool_kind,
            name: tool_name.to_string(),
            namespace: namespace.map(str::to_string),
        },
    );
    Ok(())
}

pub(super) fn translate_tool_choice(
    choice: &Value,
    bridge_state: &MessagesBridgeState,
) -> AdapterResult<(Option<Value>, Option<BTreeSet<String>>)> {
    let requested_mode = choice.as_str().or_else(|| {
        choice
            .get("mode")
            .filter(|_| choice.get("type").and_then(Value::as_str) != Some("allowed_tools"))
            .and_then(Value::as_str)
    });
    if let Some(requested_mode) = requested_mode {
        let calling_mode = match requested_mode.to_ascii_lowercase().as_str() {
            "none" => "NONE",
            "required" => "ANY",
            _ => "AUTO",
        };
        return Ok((
            Some(json!({"functionCallingConfig":{"mode":calling_mode}})),
            None,
        ));
    }
    let choice_object = choice
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    if choice_object.get("type").and_then(Value::as_str) == Some("allowed_tools") {
        let calling_mode = match choice_object
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("auto")
        {
            "required" => "ANY",
            "none" => "NONE",
            _ => "AUTO",
        };
        let mut allowed = BTreeSet::new();
        if let Some(tools) = choice_object.get("tools") {
            for tool in tools.as_array().ok_or_else(AdapterError::invalid_request)? {
                if let Some(tool) = tool.as_object() {
                    if tool.get("type").and_then(Value::as_str) == Some("namespace") {
                        let namespace = tool
                            .get("name")
                            .or_else(|| tool.get("namespace"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|namespace_name| !namespace_name.is_empty());
                        if let Some(namespace) = namespace {
                            allowed.extend(bridge_state.tool_targets.iter().filter_map(
                                |(upstream_name, target)| {
                                    (target.namespace.as_deref() == Some(namespace)
                                        && bridge_state.allows_tool_name(upstream_name))
                                    .then_some(upstream_name.clone())
                                },
                            ));
                        }
                    } else if let Some(name) = bridge_state.selected_upstream_tool_name(tool) {
                        allowed.insert(name);
                    }
                }
            }
        }
        return Ok((
            Some(
                json!({"functionCallingConfig":{"mode":calling_mode,"allowedFunctionNames":allowed.iter().cloned().collect::<Vec<_>>()}}),
            ),
            (!allowed.is_empty()).then_some(allowed),
        ));
    }
    let Some(upstream_name) = bridge_state.selected_upstream_tool_name(choice_object) else {
        return Err(AdapterError::invalid_request());
    };
    Ok((
        Some(
            json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":[upstream_name]}}),
        ),
        Some(BTreeSet::from([upstream_name])),
    ))
}
