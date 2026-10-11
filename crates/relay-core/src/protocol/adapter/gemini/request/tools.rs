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
            // Hosted tools have no Gemini function-declaration equivalent.
            _ => {}
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
    if let Some(description) =
        super::super::super::contracts::bridged_tool_description(tool, namespace, None, tool_name)
    {
        declaration.insert("description".to_string(), Value::String(description));
    }
    let parameters = if tool_kind == ResponsesToolKind::Custom {
        super::super::super::messages::custom_tool_input_schema(tool)?
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
    // Gemini has no strict-schema switch; the flag is a hint, so it is dropped.
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
    use crate::protocol::adapter::contracts::{resolve_responses_tool_choice, ToolChoice};

    let available = bridge_state.available_tool_targets();
    let (choice, allowed) = resolve_responses_tool_choice(choice, &available)?;
    let config = choice.and_then(|choice| {
        if available.is_empty() {
            return None;
        }
        let calling = match choice {
            ToolChoice::Auto => json!({"mode":"AUTO"}),
            ToolChoice::None => json!({"mode":"NONE"}),
            ToolChoice::Required => json!({"mode":"ANY"}),
            ToolChoice::Function(name) => json!({"mode":"ANY","allowedFunctionNames":[name]}),
        };
        Some(json!({"functionCallingConfig":calling}))
    });
    Ok((config, allowed))
}
