use super::super::super::{
    checked, optional_bool, required_text, AdapterError, AdapterResult, Function,
};
use crate::WireApi;
use serde_json::{json, Value};

pub(in crate::protocol::adapter::translation::decode) fn tools(
    values: Option<&Value>,
    protocol: WireApi,
) -> AdapterResult<Vec<Function>> {
    let Some(values) = values.filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for tool in values
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
                result.push(function(
                    declaration,
                    "parameters",
                    &["name", "description", "parameters", "parametersJsonSchema"],
                    true,
                )?);
            }
        } else if protocol == WireApi::Messages {
            result.push(function(
                tool,
                "input_schema",
                &["name", "description", "input_schema", "type"],
                false,
            )?);
        } else {
            if protocol == WireApi::Responses
                && tool.get("type").and_then(Value::as_str) == Some("custom")
            {
                let object = tool
                    .as_object()
                    .ok_or_else(AdapterError::unsupported_tool)?;
                if object
                    .get("defer_loading")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    || object
                        .get("allowed_callers")
                        .is_some_and(|callers| !callers.is_null())
                {
                    return Err(AdapterError::unsupported_tool());
                }
                result.push(Function {
                    name: required_text(tool, "name")?.into(),
                    description: tool
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    parameters: super::super::super::super::messages::custom_tool_input_schema(
                        object,
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
            result.push(function(
                declaration,
                "parameters",
                &["type", "name", "description", "parameters", "strict"],
                false,
            )?);
        }
    }
    let mut names = std::collections::BTreeSet::new();
    if result.iter().any(|tool| !names.insert(&tool.name)) {
        return Err(AdapterError::invalid_request());
    }
    Ok(result)
}

fn function(
    value: &Value,
    schema_key: &str,
    allowed: &[&str],
    gemini: bool,
) -> AdapterResult<Function> {
    checked(value, allowed)?;
    if value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| !matches!(kind, "function" | "custom"))
    {
        return Err(AdapterError::unsupported_tool());
    }
    let schema = value
        .get(schema_key)
        .or_else(|| gemini.then(|| value.get("parametersJsonSchema")).flatten())
        .cloned()
        .unwrap_or_else(|| json!({"type":"object","properties":{}}));
    if !schema.is_object() {
        return Err(AdapterError::invalid_request());
    }
    Ok(Function {
        name: required_text(value, "name")?.into(),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        parameters: schema,
        strict: optional_bool(value, "strict")?,
        custom: false,
    })
}
