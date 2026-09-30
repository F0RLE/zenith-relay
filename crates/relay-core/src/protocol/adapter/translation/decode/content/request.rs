use super::super::super::{
    checked, optional_bool, optional_f64, optional_u64, required_text, AdapterError, AdapterResult,
    OutputFormat, Request, ToolChoice,
};
use crate::WireApi;
use serde_json::Value;

pub(in crate::protocol::adapter::translation::decode) fn common(
    request: &mut Request,
    value: &Value,
    max: &str,
    top_p: &str,
    stop: &str,
) -> AdapterResult<()> {
    request.max_tokens = optional_u64(value, max)?;
    request.temperature = optional_f64(value, "temperature")?;
    request.top_p = optional_f64(value, top_p)?;
    if let Some(value) = value.get(stop).filter(|value| !value.is_null()) {
        request.stop = if let Some(text) = value.as_str() {
            vec![text.into()]
        } else {
            value
                .as_array()
                .ok_or_else(AdapterError::invalid_request)?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(AdapterError::invalid_request)
                })
                .collect::<AdapterResult<_>>()?
        };
    }
    Ok(())
}

pub(in crate::protocol::adapter::translation::decode) fn choice(
    value: Option<&Value>,
    protocol: WireApi,
) -> AdapterResult<Option<ToolChoice>> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let kind = value
        .as_str()
        .or_else(|| value.get("type").and_then(Value::as_str))
        .ok_or_else(AdapterError::unsupported_tool)?;
    Ok(Some(match kind {
        "auto" => ToolChoice::Auto,
        "none" => ToolChoice::None,
        "required" | "any" => ToolChoice::Required,
        "function" | "tool" | "custom" => {
            let target = if protocol == WireApi::ChatCompletions {
                value
                    .get("function")
                    .ok_or_else(AdapterError::unsupported_tool)?
            } else {
                value
            };
            ToolChoice::Function(required_text(target, "name")?.into())
        }
        _ => return Err(AdapterError::unsupported_tool()),
    }))
}

pub(in crate::protocol::adapter::translation::decode) fn output_format(
    value: &Value,
) -> AdapterResult<Option<OutputFormat>> {
    if value.is_null() {
        return Ok(None);
    }
    checked(
        value,
        &[
            "type",
            "name",
            "schema",
            "strict",
            "description",
            "json_schema",
        ],
    )?;
    match value.get("type").and_then(Value::as_str) {
        Some("text") => Ok(None),
        Some("json_object") => Ok(Some(OutputFormat::JsonObject)),
        Some("json_schema") => {
            let schema = value.get("json_schema").unwrap_or(value);
            checked(schema, &["type", "name", "schema", "strict", "description"])?;
            Ok(Some(OutputFormat::JsonSchema {
                name: schema
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("response")
                    .into(),
                schema: schema
                    .get("schema")
                    .filter(|schema| schema.is_object())
                    .ok_or_else(AdapterError::invalid_request)?
                    .clone(),
                strict: optional_bool(schema, "strict")?,
            }))
        }
        _ => Err(AdapterError::parameter_unsupported()),
    }
}
