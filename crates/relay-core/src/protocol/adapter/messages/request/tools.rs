//! Responses tool definitions translated into Anthropic tool blocks.

use super::{
    AdapterError, AdapterResult, ClientToolTarget, MessagesBridgeState, ResponsesToolKind,
    TranslatedTools,
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn translate_tools(tools: &[Value]) -> AdapterResult<TranslatedTools> {
    let mut upstream = Vec::with_capacity(tools.len());
    let mut client_tools = BTreeMap::new();
    for tool in tools {
        let Some(tool) = tool.as_object() else {
            continue;
        };
        match tool.get("type").and_then(Value::as_str) {
            Some("function" | "custom") => {
                translate_client_tool(&mut upstream, &mut client_tools, tool, None, None)?;
            }
            Some("namespace") => {
                let Some(namespace) = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|namespace_name| !namespace_name.is_empty())
                else {
                    continue;
                };
                let namespace_description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|description_text| !description_text.is_empty());
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
                        translate_client_tool(
                            &mut upstream,
                            &mut client_tools,
                            child,
                            Some(namespace),
                            namespace_description,
                        )?;
                    }
                    // The Responses namespace can carry tools that require
                    // server execution or a separate adapter. Do not make a
                    // Messages source advertise them under a fake contract.
                }
            }
            // Hosted tools have no Messages equivalent; leave them out.
            _ => {}
        }
    }
    Ok(TranslatedTools {
        upstream,
        client_tools,
    })
}

fn translate_client_tool(
    upstream: &mut Vec<Value>,
    client_tools: &mut BTreeMap<String, ClientToolTarget>,
    tool: &Map<String, Value>,
    namespace: Option<&str>,
    namespace_description: Option<&str>,
) -> AdapterResult<()> {
    let target = ClientToolTarget::from_definition(tool, namespace)?;
    let upstream_name = target.upstream_name();
    if client_tools.contains_key(&upstream_name) {
        return Err(AdapterError::unsupported_tool());
    }

    let mut translated =
        Map::from_iter([("name".to_string(), Value::String(upstream_name.clone()))]);
    match target.kind {
        ResponsesToolKind::Function => {
            let mut schema = tool
                .get("parameters")
                .or_else(|| tool.get("input_schema"))
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            let schema = schema
                .as_object_mut()
                .ok_or_else(AdapterError::unsupported_tool)?;
            match schema.get("type").and_then(Value::as_str) {
                Some("object") => {}
                None => {
                    schema.insert("type".to_string(), Value::String("object".to_string()));
                }
                _ => return Err(AdapterError::unsupported_tool()),
            }
            translated.insert("input_schema".to_string(), Value::Object(schema.clone()));
            if let Some(strict) = tool
                .get("strict")
                .filter(|strict_value| !strict_value.is_null())
            {
                if !strict.is_boolean() {
                    return Err(AdapterError::invalid_request());
                }
                translated.insert("strict".to_string(), strict.clone());
            }
        }
        ResponsesToolKind::Custom => {
            translated.insert("input_schema".to_string(), custom_tool_input_schema(tool)?);
        }
    }
    if let Some(description) = super::super::super::contracts::bridged_tool_description(
        tool,
        namespace,
        namespace_description,
        &target.name,
    ) {
        translated.insert("description".to_string(), Value::String(description));
    }
    client_tools.insert(upstream_name, target);
    upstream.push(Value::Object(translated));
    Ok(())
}

pub(in crate::protocol::adapter) fn custom_tool_input_schema(
    tool: &Map<String, Value>,
) -> AdapterResult<Value> {
    let mut input_schema =
        Map::from_iter([("type".to_string(), Value::String("string".to_string()))]);
    if let Some(format_value) = tool.get("format") {
        let format_object = format_value
            .as_object()
            .ok_or_else(AdapterError::unsupported_tool)?;
        match format_object.get("type").and_then(Value::as_str) {
            Some("text") => {}
            Some("grammar") => {
                let syntax = format_object
                    .get("syntax")
                    .and_then(Value::as_str)
                    .filter(|syntax| matches!(*syntax, "lark" | "regex"))
                    .ok_or_else(AdapterError::unsupported_tool)?;
                let definition = format_object
                    .get("definition")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|definition| !definition.is_empty())
                    .ok_or_else(AdapterError::unsupported_tool)?;
                input_schema.insert(
                    "description".to_string(),
                    Value::String(format!(
                        "Raw tool input. It must satisfy this {syntax} grammar:\n{definition}"
                    )),
                );
            }
            _ => return Err(AdapterError::unsupported_tool()),
        }
    }
    Ok(json!({
        "type": "object",
        "properties": {"input": Value::Object(input_schema)},
        "required": ["input"],
        "additionalProperties": false,
    }))
}

#[derive(Debug)]
pub(super) struct TranslatedToolChoice {
    pub(super) translated_choice: Option<Value>,
    pub(super) allowed_names: Option<BTreeSet<String>>,
}

mod choice;
pub(in crate::protocol::adapter::messages::request) use choice::translate_tool_choice;
