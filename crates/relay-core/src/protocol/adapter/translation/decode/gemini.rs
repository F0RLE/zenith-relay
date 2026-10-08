use super::super::{
    checked, optional_u64, required_text, AdapterError, AdapterResult, Message, OutputFormat,
    Reasoning, Request, Role, ToolChoice,
};
use crate::WireApi;
use serde_json::Value;

pub(super) fn decode(request_body: &Value) -> AdapterResult<Request> {
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
            role: super::content::role(content)?,
            blocks: super::content::text_blocks(
                content
                    .get("parts")
                    .ok_or_else(AdapterError::invalid_request)?,
                WireApi::Gemini,
            )?,
        });
    }
    decoded_request.tools = super::content::tools(request_body.get("tools"), WireApi::Gemini)?;
    if let Some(tool_config) = request_body.get("toolConfig") {
        checked(tool_config, &["functionCallingConfig"])?;
        let choice_config = tool_config
            .get("functionCallingConfig")
            .ok_or_else(AdapterError::unsupported_tool)?;
        checked(choice_config, &["mode", "allowedFunctionNames"])?;
        decoded_request.tool_choice = Some(match required_text(choice_config, "mode")? {
            "AUTO" => ToolChoice::Auto,
            "NONE" => ToolChoice::None,
            "ANY" => {
                if let Some(names) = choice_config
                    .get("allowedFunctionNames")
                    .and_then(Value::as_array)
                {
                    if names.len() != 1 {
                        return Err(AdapterError::unsupported_tool());
                    }
                    ToolChoice::Function(
                        names[0]
                            .as_str()
                            .ok_or_else(AdapterError::unsupported_tool)?
                            .into(),
                    )
                } else {
                    ToolChoice::Required
                }
            }
            _ => return Err(AdapterError::unsupported_tool()),
        });
    }
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
        super::content::common(
            &mut decoded_request,
            generation_config,
            "maxOutputTokens",
            "topP",
            "stopSequences",
        )?;
        if let Some(mime) = generation_config
            .get("responseMimeType")
            .and_then(Value::as_str)
        {
            decoded_request.output_format = match mime {
                "text/plain" => None,
                "application/json" => Some(
                    generation_config
                        .get("responseJsonSchema")
                        .or_else(|| generation_config.get("responseSchema"))
                        .map_or(OutputFormat::JsonObject, |schema| {
                            OutputFormat::JsonSchema {
                                name: "response".into(),
                                schema: schema.clone(),
                                strict: None,
                            }
                        }),
                ),
                _ => return Err(AdapterError::parameter_unsupported()),
            };
        }
        if let Some(thinking) = generation_config.get("thinkingConfig") {
            checked(thinking, &["thinkingLevel", "thinkingBudget"])?;
            decoded_request.reasoning =
                if let Some(level) = thinking.get("thinkingLevel").and_then(Value::as_str) {
                    Some(Reasoning::Effort(level.to_ascii_lowercase()))
                } else {
                    optional_u64(thinking, "thinkingBudget")?.map(Reasoning::Budget)
                };
        }
    }
    Ok(decoded_request)
}
