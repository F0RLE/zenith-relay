use super::super::super::{
    checked, optional_bool, required_text, AdapterError, AdapterResult, ClientToolTarget, Function,
    ResponsesToolKind,
};
use crate::WireApi;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub(in crate::protocol::adapter::translation::decode) fn tools(
    tool_values: Option<&Value>,
    protocol: WireApi,
) -> AdapterResult<Vec<Function>> {
    let Some(tool_list) = tool_values.filter(|tool_value| !tool_value.is_null()) else {
        return Ok(Vec::new());
    };
    let mut functions = Vec::new();
    for tool in tool_list
        .as_array()
        .ok_or_else(AdapterError::invalid_request)?
    {
        if protocol == WireApi::Gemini {
            checked(tool, &["functionDeclarations"])?;
            for declaration in tool
                .get("functionDeclarations")
                .and_then(Value::as_array)
                .ok_or_else(AdapterError::unsupported_tool)?
            {
                functions.push(function(
                    declaration,
                    "parameters",
                    &["name", "description", "parameters", "parametersJsonSchema"],
                    true,
                )?);
            }
        } else if protocol == WireApi::Messages {
            functions.push(function(
                tool,
                "input_schema",
                &["name", "description", "input_schema", "type"],
                false,
            )?);
        } else {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(AdapterError::unsupported_tool());
            }
            checked(tool, &["type", "function"])?;
            let declaration = tool
                .get("function")
                .ok_or_else(AdapterError::unsupported_tool)?;
            functions.push(function(
                declaration,
                "parameters",
                &["type", "name", "description", "parameters", "strict"],
                false,
            )?);
        }
    }
    let mut names = std::collections::BTreeSet::new();
    if functions.iter().any(|tool| !names.insert(&tool.name)) {
        return Err(AdapterError::invalid_request());
    }
    Ok(functions)
}

/// Builds the upstream function list for a Responses client from its complete
/// catalog (root `tools` plus `additional_tools` items). Namespace children are
/// flattened to opaque names and recorded in the returned target map so the
/// response can restore the client's own name and namespace. Hosted tools have
/// no function equivalent upstream and are left out.
pub(in crate::protocol::adapter::translation::decode) fn responses_tools(
    request_body: &Value,
) -> AdapterResult<(Vec<Function>, BTreeMap<String, ClientToolTarget>)> {
    let request_object = request_body
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    let catalog = super::super::super::super::contracts::request_tool_catalog(request_object)?
        .unwrap_or_default();
    let mut functions = Vec::new();
    let mut targets = BTreeMap::new();
    for tool in &catalog {
        let tool = tool
            .as_object()
            .ok_or_else(AdapterError::unsupported_tool)?;
        match tool.get("type").and_then(Value::as_str) {
            Some("namespace") => {
                let namespace = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|namespace_name| !namespace_name.is_empty())
                    .ok_or_else(AdapterError::unsupported_tool)?;
                let namespace_description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|description_text| !description_text.is_empty());
                for child in tool
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(AdapterError::unsupported_tool)?
                {
                    let child = child
                        .as_object()
                        .ok_or_else(AdapterError::unsupported_tool)?;
                    if matches!(
                        child.get("type").and_then(Value::as_str),
                        Some("function" | "custom") | None
                    ) {
                        push_responses_tool(
                            &mut functions,
                            &mut targets,
                            child,
                            Some(namespace),
                            namespace_description,
                        )?;
                    }
                }
            }
            Some("function" | "custom") | None => {
                push_responses_tool(&mut functions, &mut targets, tool, None, None)?;
            }
            // Hosted tools run on the provider's servers; nothing to declare.
            Some(_) => {}
        }
    }
    Ok((functions, targets))
}

fn push_responses_tool(
    functions: &mut Vec<Function>,
    targets: &mut BTreeMap<String, ClientToolTarget>,
    tool: &Map<String, Value>,
    namespace: Option<&str>,
    namespace_description: Option<&str>,
) -> AdapterResult<()> {
    let target = ClientToolTarget::from_definition(tool, namespace)?;
    let upstream_name = target.upstream_name();
    if targets.contains_key(&upstream_name) {
        return Err(AdapterError::invalid_request());
    }
    let (parameters, strict) = match target.kind {
        ResponsesToolKind::Function => {
            let schema = tool
                .get("parameters")
                .cloned()
                .unwrap_or_else(|| json!({"type":"object","properties":{}}));
            if !schema.is_object() {
                return Err(AdapterError::invalid_request());
            }
            let strict = tool
                .get("strict")
                .filter(|strict_value| !strict_value.is_null())
                .map(|strict_value| {
                    strict_value
                        .as_bool()
                        .ok_or_else(AdapterError::invalid_request)
                })
                .transpose()?;
            (schema, strict)
        }
        ResponsesToolKind::Custom => (
            super::super::super::super::messages::custom_tool_input_schema(tool)?,
            Some(false),
        ),
    };
    let description = super::super::super::super::contracts::bridged_tool_description(
        tool,
        namespace,
        namespace_description,
        &target.name,
    );
    functions.push(Function {
        name: upstream_name.clone(),
        description,
        parameters,
        strict,
    });
    targets.insert(upstream_name, target);
    Ok(())
}

fn function(
    declaration: &Value,
    schema_key: &str,
    allowed: &[&str],
    gemini: bool,
) -> AdapterResult<Function> {
    checked(declaration, allowed)?;
    if declaration
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| !matches!(kind, "function" | "custom"))
    {
        return Err(AdapterError::unsupported_tool());
    }
    let schema = declaration
        .get(schema_key)
        .or_else(|| {
            gemini
                .then(|| declaration.get("parametersJsonSchema"))
                .flatten()
        })
        .cloned()
        .unwrap_or_else(|| json!({"type":"object","properties":{}}));
    if !schema.is_object() {
        return Err(AdapterError::invalid_request());
    }
    Ok(Function {
        name: required_text(declaration, "name")?.into(),
        description: declaration
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        parameters: schema,
        strict: optional_bool(declaration, "strict")?,
    })
}
