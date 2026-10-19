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

pub(in crate::protocol::adapter::translation::decode) fn sampling_extensions(
    request: &mut Request,
    fields: &Value,
    gemini: bool,
) -> AdapterResult<()> {
    request.top_k = optional_u64(fields, if gemini { "topK" } else { "top_k" })?;
    if request.top_k == Some(0) {
        return Err(AdapterError::invalid_request().with_parameter("top_k"));
    }
    request.presence_penalty = optional_f64(
        fields,
        if gemini {
            "presencePenalty"
        } else {
            "presence_penalty"
        },
    )?;
    request.frequency_penalty = optional_f64(
        fields,
        if gemini {
            "frequencyPenalty"
        } else {
            "frequency_penalty"
        },
    )?;
    request.seed = fields
        .get("seed")
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_i64()
                .ok_or_else(|| AdapterError::invalid_request().with_parameter("seed"))
        })
        .transpose()?;
    Ok(())
}

pub(in crate::protocol::adapter::translation::decode) fn choice(
    choice_value: Option<&Value>,
    protocol: WireApi,
) -> AdapterResult<Option<ToolChoice>> {
    let Some(choice_value) = choice_value.filter(|choice_value| !choice_value.is_null()) else {
        return Ok(None);
    };
    let choice = match protocol {
        WireApi::ChatCompletions => {
            if let Some(kind) = choice_value.as_str() {
                return Ok(Some(match kind {
                    "auto" => ToolChoice::Auto,
                    "none" => ToolChoice::None,
                    "required" => ToolChoice::Required,
                    _ => return Err(AdapterError::unsupported_tool()),
                }));
            }
            checked(choice_value, &["type", "function"])
                .map_err(|error| error.with_parameter("tool_choice"))?;
            if choice_value.get("type").and_then(Value::as_str) != Some("function") {
                return Err(AdapterError::unsupported_tool());
            }
            let function = choice_value
                .get("function")
                .ok_or_else(AdapterError::unsupported_tool)?;
            checked(function, &["name"])
                .map_err(|error| error.with_parameter("tool_choice.function"))?;
            ToolChoice::Function(required_text(function, "name")?.into())
        }
        WireApi::Messages => {
            let kind = required_text(choice_value, "type")?;
            let fields: &[&str] = match kind {
                "auto" | "any" => &["type", "disable_parallel_tool_use"],
                "tool" => &["type", "name", "disable_parallel_tool_use"],
                "none" => &["type"],
                _ => return Err(AdapterError::unsupported_tool()),
            };
            checked(choice_value, fields).map_err(|error| error.with_parameter("tool_choice"))?;
            match kind {
                "auto" => ToolChoice::Auto,
                "none" => ToolChoice::None,
                "any" => ToolChoice::Required,
                _ => ToolChoice::Function(required_text(choice_value, "name")?.into()),
            }
        }
        _ => return Err(AdapterError::unsupported_tool()),
    };
    Ok(Some(choice))
}

pub(in crate::protocol::adapter::translation::decode) fn output_format(
    format_value: &Value,
) -> AdapterResult<Option<OutputFormat>> {
    if format_value.is_null() {
        return Ok(None);
    }
    match required_text(format_value, "type")? {
        kind @ ("text" | "json_object") => {
            checked(format_value, &["type"])?;
            Ok((kind == "json_object").then_some(OutputFormat::JsonObject))
        }
        "json_schema" => {
            let schema_value = if let Some(schema_value) = format_value
                .get("json_schema")
                .filter(|value| !value.is_null())
            {
                checked(format_value, &["type", "json_schema"])?;
                checked(schema_value, &["name", "schema", "strict", "description"])?;
                schema_value
            } else {
                checked(
                    format_value,
                    &["type", "name", "schema", "strict", "description"],
                )?;
                format_value
            };
            Ok(Some(OutputFormat::JsonSchema {
                name: schema_value
                    .get("name")
                    .filter(|value| !value.is_null())
                    .map(|_| required_text(schema_value, "name"))
                    .transpose()?
                    .unwrap_or("response")
                    .into(),
                description: schema_value
                    .get("description")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(AdapterError::invalid_request)
                    })
                    .transpose()?,
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
