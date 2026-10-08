use super::super::super::{
    checked, optional_bool, optional_f64, optional_u64, required_text, AdapterError, AdapterResult,
    OutputFormat, Request, ToolChoice,
};
use crate::WireApi;
use serde_json::Value;

pub(in crate::protocol::adapter::translation::decode) fn common(
    request: &mut Request,
    request_fields: &Value,
    max: &str,
    top_p: &str,
    stop: &str,
) -> AdapterResult<()> {
    request.max_tokens = optional_u64(request_fields, max)?;
    request.temperature = optional_f64(request_fields, "temperature")?;
    request.top_p = optional_f64(request_fields, top_p)?;
    if let Some(stop_value) = request_fields
        .get(stop)
        .filter(|stop_value| !stop_value.is_null())
    {
        request.stop = if let Some(text) = stop_value.as_str() {
            vec![text.into()]
        } else {
            stop_value
                .as_array()
                .ok_or_else(AdapterError::invalid_request)?
                .iter()
                .map(|stop_item| {
                    stop_item
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
    choice_value: Option<&Value>,
    protocol: WireApi,
) -> AdapterResult<Option<ToolChoice>> {
    let Some(choice_value) = choice_value.filter(|choice_value| !choice_value.is_null()) else {
        return Ok(None);
    };
    let kind = choice_value
        .as_str()
        .or_else(|| choice_value.get("type").and_then(Value::as_str))
        .ok_or_else(AdapterError::unsupported_tool)?;
    Ok(Some(match kind {
        "auto" => ToolChoice::Auto,
        "none" => ToolChoice::None,
        "required" | "any" => ToolChoice::Required,
        "function" | "tool" | "custom" => {
            let target = if protocol == WireApi::ChatCompletions {
                choice_value
                    .get("function")
                    .ok_or_else(AdapterError::unsupported_tool)?
            } else {
                choice_value
            };
            ToolChoice::Function(required_text(target, "name")?.into())
        }
        _ => return Err(AdapterError::unsupported_tool()),
    }))
}

pub(in crate::protocol::adapter::translation::decode) fn output_format(
    format_value: &Value,
) -> AdapterResult<Option<OutputFormat>> {
    if format_value.is_null() {
        return Ok(None);
    }
    checked(
        format_value,
        &[
            "type",
            "name",
            "schema",
            "strict",
            "description",
            "json_schema",
        ],
    )?;
    match format_value.get("type").and_then(Value::as_str) {
        Some("text") => Ok(None),
        Some("json_object") => Ok(Some(OutputFormat::JsonObject)),
        Some("json_schema") => {
            let schema_value = format_value.get("json_schema").unwrap_or(format_value);
            checked(
                schema_value,
                &["type", "name", "schema", "strict", "description"],
            )?;
            Ok(Some(OutputFormat::JsonSchema {
                name: schema_value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("response")
                    .into(),
                schema: schema_value
                    .get("schema")
                    .filter(|schema_value| schema_value.is_object())
                    .ok_or_else(AdapterError::invalid_request)?
                    .clone(),
                strict: optional_bool(schema_value, "strict")?,
            }))
        }
        _ => Err(AdapterError::parameter_unsupported()),
    }
}
