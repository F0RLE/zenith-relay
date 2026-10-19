use super::super::{
    checked, optional_u64, required_text, AdapterError, AdapterResult, Message, OutputFormat,
    Reasoning, Request, Role, ToolChoice,
};
use crate::WireApi;
use serde_json::Value;

pub(super) fn decode(request_body: &Value) -> AdapterResult<Request> {
    for parameter in ["cachedContent", "continuationToken"] {
        if request_body
            .get(parameter)
            .is_some_and(|value| !value.is_null())
        {
            return Err(AdapterError::parameter_unsupported_for(parameter));
        }
    }
    checked(
        request_body,
        &[
            "model",
            "stream",
            "contents",
            "systemInstruction",
            "tools",
            "toolConfig",
            "generationConfig",
        ],
    )?;
    let mut decoded_request = Request::default();
    if let Some(system) = request_body.get("systemInstruction") {
        checked(system, &["role", "parts"])?;
        decoded_request.messages.push(Message {
            role: Role::System,
            blocks: super::content::text_blocks(
                system
                    .get("parts")
                    .ok_or_else(AdapterError::invalid_request)?,
                WireApi::Gemini,
            )?,
        });
    }
    for content in request_body
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::invalid_request)?
    {
        checked(content, &["role", "parts"])?;
        decoded_request.messages.push(Message {
            role: super::content::role(content, WireApi::Gemini)?,
            blocks: super::content::text_blocks(
                content
                    .get("parts")
                    .ok_or_else(AdapterError::invalid_request)?,
                WireApi::Gemini,
            )?,
        });
    }
    decoded_request.tools = super::content::tools(request_body.get("tools"), WireApi::Gemini)?;
    decoded_request.tool_choice = tool_choice(request_body)?;
    if let Some(generation_config) = request_body
        .get("generationConfig")
        .filter(|generation_config_value| !generation_config_value.is_null())
    {
        checked(
            generation_config,
            &[
                "maxOutputTokens",
                "temperature",
                "topP",
                "topK",
                "presencePenalty",
                "frequencyPenalty",
                "seed",
                "stopSequences",
                "responseMimeType",
                "responseSchema",
                "responseJsonSchema",
                "thinkingConfig",
                "candidateCount",
            ],
        )?;
        if optional_u64(generation_config, "candidateCount")?.is_some_and(|n| n != 1) {
            return Err(AdapterError::parameter_unsupported());
        }
        super::content::sampling_extensions(&mut decoded_request, generation_config, true)?;
        super::content::common(
            &mut decoded_request,
            generation_config,
            "maxOutputTokens",
            "topP",
            "stopSequences",
        )?;
        let response_schema = super::content::schema_field(
            generation_config,
            "responseJsonSchema",
            "responseSchema",
        )?;
        let mime = generation_config
            .get("responseMimeType")
            .filter(|value| !value.is_null())
            .map(|value| {
                value.as_str().ok_or_else(|| {
                    AdapterError::invalid_request()
                        .with_parameter("generationConfig.responseMimeType")
                })
            })
            .transpose()?;
        if response_schema.is_some() && mime != Some("application/json") {
            return Err(
                AdapterError::invalid_request().with_parameter("generationConfig.responseMimeType")
            );
        }
        if let Some(mime) = mime {
            decoded_request.output_format = match mime {
                "text/plain" => None,
                "application/json" => {
                    Some(response_schema.map_or(OutputFormat::JsonObject, |schema| {
                        OutputFormat::JsonSchema {
                            name: "response".into(),
                            description: None,
                            schema,
                            strict: None,
                        }
                    }))
                }
                _ => return Err(AdapterError::parameter_unsupported()),
            };
        }
        if let Some(thinking) = generation_config
            .get("thinkingConfig")
            .filter(|thinking| !thinking.is_null())
        {
            checked(thinking, &["thinkingLevel", "thinkingBudget"])?;
            let budget = thinking
                .get("thinkingBudget")
                .filter(|value| !value.is_null());
            if budget.is_some_and(|budget| budget.as_u64().is_none() && budget.as_i64() != Some(-1))
            {
                return Err(AdapterError::invalid_request()
                    .with_parameter("generationConfig.thinkingConfig.thinkingBudget"));
            }
            decoded_request.reasoning = if thinking
                .get("thinkingLevel")
                .is_some_and(|level| !level.is_null())
            {
                if budget.is_some() {
                    return Err(AdapterError::reasoning_unsupported());
                }
                Some(Reasoning::Effort(
                    required_text(thinking, "thinkingLevel")?.into(),
                ))
            } else {
                budget.map(|budget| match budget.as_u64() {
                    Some(0) => Reasoning::Effort("none".into()),
                    Some(budget) => Reasoning::Budget(budget),
                    None => Reasoning::DynamicBudget,
                })
            };
        }
    }
    Ok(decoded_request)
}

fn tool_choice(request_body: &Value) -> AdapterResult<Option<ToolChoice>> {
    let Some(tool_config) = request_body
        .get("toolConfig")
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    checked(tool_config, &["functionCallingConfig"])?;
    let Some(choice) = tool_config
        .get("functionCallingConfig")
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    checked(choice, &["mode", "allowedFunctionNames"])?;
    let mode = choice
        .get("mode")
        .filter(|value| !value.is_null())
        .map(|_| required_text(choice, "mode"))
        .transpose()?
        .unwrap_or("AUTO");
    let names = choice
        .get("allowedFunctionNames")
        .filter(|value| !value.is_null())
        .map(|value| value.as_array().ok_or_else(AdapterError::invalid_request))
        .transpose()?;
    if mode != "ANY" && names.is_some_and(|names| !names.is_empty()) {
        return Err(AdapterError::parameter_unsupported_for(
            "toolConfig.functionCallingConfig.allowedFunctionNames",
        ));
    }
    Ok(Some(match mode {
        "AUTO" => ToolChoice::Auto,
        "NONE" => ToolChoice::None,
        "ANY" => match names.map(Vec::as_slice).unwrap_or_default() {
            [] => ToolChoice::Required,
            [name] => ToolChoice::Function(
                name.as_str()
                    .filter(|name| !name.trim().is_empty())
                    .ok_or_else(AdapterError::invalid_request)?
                    .into(),
            ),
            _ => {
                return Err(AdapterError::parameter_unsupported_for(
                    "toolConfig.functionCallingConfig.allowedFunctionNames",
                ))
            }
        },
        _ => return Err(AdapterError::unsupported_tool()),
    }))
}
