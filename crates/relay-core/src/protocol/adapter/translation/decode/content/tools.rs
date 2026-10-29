use super::super::super::{
    AdapterError, AdapterResult, ClientToolTarget, Function, ResponsesToolKind, checked,
    optional_bool, required_text,
};
use crate::WireApi;
use serde_json::{Map, Value, json};
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
            super::super::super::options::messages_fields(
                tool,
                &["name", "description", "input_schema", "type", "strict"],
            )?;
            functions.push(function(
                tool,
                "input_schema",
                &[
                    "name",
                    "description",
                    "input_schema",
                    "type",
                    "strict",
                    "cache_control",
                ],
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
/// no function equivalent upstream and must be rejected, not silently omitted.
pub(in crate::protocol::adapter::translation::decode) struct ResponsesTools {
    pub functions: Vec<Function>,
    pub targets: BTreeMap<String, ClientToolTarget>,
    pub web_search: Option<Value>,
}

pub(in crate::protocol::adapter::translation::decode) fn responses_tools(
    request_body: &Value,
) -> AdapterResult<ResponsesTools> {
    let request_object = request_body
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    let catalog = super::super::super::super::contracts::request_tool_catalog(request_object)?
        .unwrap_or_default();
    let mut functions = Vec::new();
    let mut targets = BTreeMap::new();
    let mut web_search = None;
    push_responses_tools(
        &mut functions,
        &mut targets,
        &catalog,
        None,
        None,
        &mut web_search,
    )?;
    Ok(ResponsesTools {
        functions,
        targets,
        web_search,
    })
}

fn push_responses_tools(
    functions: &mut Vec<Function>,
    targets: &mut BTreeMap<String, ClientToolTarget>,
    tools: &[Value],
    namespace: Option<&str>,
    namespace_description: Option<&str>,
    web_search: &mut Option<Value>,
) -> AdapterResult<()> {
    for tool in tools {
        let tool = tool
            .as_object()
            .ok_or_else(AdapterError::unsupported_tool)?;
        match tool.get("type").and_then(Value::as_str) {
            Some("namespace") => {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|namespace_name| !namespace_name.is_empty())
                    .ok_or_else(AdapterError::unsupported_tool)?;
                let description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|description_text| !description_text.is_empty());
                let qualified = namespace
                    .map(|parent| format!("{parent}.{name}"))
                    .unwrap_or_else(|| name.to_owned());
                let description = [namespace_description, description]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                let children = tool
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(AdapterError::unsupported_tool)?;
                push_responses_tools(
                    functions,
                    targets,
                    children,
                    Some(&qualified),
                    Some(&description),
                    web_search,
                )?;
            }
            Some("web_search" | "web_search_2025_08_26") if namespace.is_none() => {
                if web_search.is_some() {
                    return Err(AdapterError::invalid_request().with_parameter("tools"));
                }
                *web_search = Some(super::super::super::web_search::tool(&Value::Object(
                    tool.clone(),
                ))?);
            }
            Some("function" | "custom" | "tool_search") | None => {
                push_responses_tool(functions, targets, tool, namespace, namespace_description)?;
            }
            Some(_) => return Err(AdapterError::unsupported_tool().with_parameter("tools.type")),
        }
    }
    Ok(())
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
        ResponsesToolKind::Function | ResponsesToolKind::ToolSearch => {
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
    let schema = if gemini {
        super::schema_field(declaration, "parametersJsonSchema", schema_key)?
    } else {
        declaration.get(schema_key).cloned()
    }
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
