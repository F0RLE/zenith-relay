use super::super::{
    checked, optional_u64, required_text, AdapterError, AdapterResult, Message, OutputFormat,
    Reasoning, Request, Role, ToolChoice,
};
use crate::WireApi;
use serde_json::Value;

pub(super) fn decode(value: &Value) -> AdapterResult<Request> {
    checked(
        value,
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
    let mut request = Request::default();
    if let Some(system) = value.get("systemInstruction") {
        checked(system, &["role", "parts"])?;
        request.messages.push(Message {
            role: Role::System,
            blocks: super::content::text_blocks(
                system
                    .get("parts")
                    .ok_or_else(AdapterError::invalid_request)?,
                WireApi::Gemini,
            )?,
        });
    }
    for content in value
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::invalid_request)?
    {
        checked(content, &["role", "parts"])?;
        request.messages.push(Message {
            role: super::content::role(content)?,
            blocks: super::content::text_blocks(
                content
                    .get("parts")
                    .ok_or_else(AdapterError::invalid_request)?,
                WireApi::Gemini,
            )?,
        });
    }
    request.tools = super::content::tools(value.get("tools"), WireApi::Gemini)?;
    if let Some(config) = value.get("toolConfig") {
        checked(config, &["functionCallingConfig"])?;
        let choice = config
            .get("functionCallingConfig")
            .ok_or_else(AdapterError::unsupported_tool)?;
        checked(choice, &["mode", "allowedFunctionNames"])?;
        request.tool_choice = Some(match required_text(choice, "mode")? {
            "AUTO" => ToolChoice::Auto,
            "NONE" => ToolChoice::None,
            "ANY" => {
                if let Some(names) = choice.get("allowedFunctionNames").and_then(Value::as_array) {
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
    if let Some(config) = value.get("generationConfig").filter(|v| !v.is_null()) {
        checked(
            config,
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
        if optional_u64(config, "candidateCount")?.is_some_and(|n| n != 1) {
            return Err(AdapterError::parameter_unsupported());
        }
        super::content::common(
            &mut request,
            config,
            "maxOutputTokens",
            "topP",
            "stopSequences",
        )?;
        if let Some(mime) = config.get("responseMimeType").and_then(Value::as_str) {
            request.output_format = match mime {
                "text/plain" => None,
                "application/json" => Some(
                    config
                        .get("responseJsonSchema")
                        .or_else(|| config.get("responseSchema"))
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
        if let Some(thinking) = config.get("thinkingConfig") {
            checked(thinking, &["thinkingLevel", "thinkingBudget"])?;
            request.reasoning =
                if let Some(level) = thinking.get("thinkingLevel").and_then(Value::as_str) {
                    Some(Reasoning::Effort(level.to_ascii_lowercase()))
                } else {
                    optional_u64(thinking, "thinkingBudget")?.map(Reasoning::Budget)
                };
        }
    }
    Ok(request)
}
