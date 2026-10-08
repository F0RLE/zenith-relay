use super::super::super::{
    checked, optional_bool, required_text, AdapterError, AdapterResult, Function,
};
use crate::WireApi;
use serde_json::{json, Value};

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
            if protocol == WireApi::Responses
                && tool.get("type").and_then(Value::as_str) == Some("custom")
            {
                let tool_object = tool
                    .as_object()
                    .ok_or_else(AdapterError::unsupported_tool)?;
                if tool_object
                    .get("defer_loading")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    || tool_object
                        .get("allowed_callers")
                        .is_some_and(|callers| !callers.is_null())
                {
                    return Err(AdapterError::unsupported_tool());
                }
                functions.push(Function {
                    name: required_text(tool, "name")?.into(),
                    description: tool
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    parameters: super::super::super::super::messages::custom_tool_input_schema(
                        tool_object,
                    )?,
                    strict: Some(false),
                    custom: true,
                });
                continue;
            }
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(AdapterError::unsupported_tool());
            }
            let declaration = if protocol == WireApi::ChatCompletions {
                checked(tool, &["type", "function"])?;
                tool.get("function")
                    .ok_or_else(AdapterError::unsupported_tool)?
            } else {
                tool
            };
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
        custom: false,
    })
}
