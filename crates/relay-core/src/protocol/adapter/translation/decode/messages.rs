use super::super::{
    checked, optional_bool, optional_u64, required_text, AdapterError, AdapterResult, Message,
    Reasoning, Request, Role,
};
use crate::WireApi;
use serde_json::Value;

pub(super) fn decode(request_body: &Value) -> AdapterResult<Request> {
    checked(
        request_body,
        &[
            "model",
            "stream",
            "messages",
            "system",
            "max_tokens",
            "temperature",
            "top_p",
            "stop_sequences",
            "tools",
            "tool_choice",
            "thinking",
            "output_config",
        ],
    )?;
    let mut decoded_request = Request::default();
    if let Some(system) = request_body.get("system") {
        decoded_request.messages.push(Message {
            role: Role::System,
            blocks: super::content::text_blocks(system, WireApi::Messages)?,
        });
    }
    for message in request_body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::invalid_request)?
    {
        checked(message, &["role", "content"])?;
        decoded_request.messages.push(Message {
            role: super::content::role(message)?,
            blocks: super::content::text_blocks(
                message
                    .get("content")
                    .ok_or_else(AdapterError::invalid_request)?,
                WireApi::Messages,
            )?,
        });
    }
    decoded_request.tools = super::content::tools(request_body.get("tools"), WireApi::Messages)?;
    decoded_request.tool_choice =
        super::content::choice(request_body.get("tool_choice"), WireApi::Messages)?;
    if let Some(choice) = request_body.get("tool_choice") {
        checked(choice, &["type", "name", "disable_parallel_tool_use"])?;
        decoded_request.parallel_tools =
            optional_bool(choice, "disable_parallel_tool_use")?.map(|disabled| !disabled);
    }
    super::content::common(
        &mut decoded_request,
        request_body,
        "max_tokens",
        "top_p",
        "stop_sequences",
    )?;
    if let Some(output_config) = request_body
        .get("output_config")
        .filter(|output_config_value| !output_config_value.is_null())
    {
        checked(output_config, &["format", "effort"])?;
        decoded_request.output_format =
            super::content::output_format(output_config.get("format").unwrap_or(&Value::Null))?;
        decoded_request.reasoning = output_config
            .get("effort")
            .and_then(Value::as_str)
            .map(|effort| Reasoning::Effort(effort.into()));
    }
    if let Some(thinking) = request_body
        .get("thinking")
        .filter(|thinking_value| !thinking_value.is_null())
    {
        checked(thinking, &["type", "budget_tokens"])?;
        match required_text(thinking, "type")? {
            "disabled" => decoded_request.reasoning = Some(Reasoning::Effort("none".into())),
            "adaptive" if decoded_request.reasoning.is_some() => {}
            "enabled" => {
                decoded_request.reasoning = Some(Reasoning::Budget(
                    optional_u64(thinking, "budget_tokens")?
                        .ok_or_else(AdapterError::invalid_request)?,
                ))
            }
            _ => return Err(AdapterError::reasoning_unsupported()),
        }
    }
    Ok(decoded_request)
}
