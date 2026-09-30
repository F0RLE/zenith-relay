use super::super::{
    checked, optional_bool, optional_u64, required_text, AdapterError, AdapterResult, Message,
    Reasoning, Request, Role,
};
use crate::WireApi;
use serde_json::Value;

pub(super) fn decode(value: &Value) -> AdapterResult<Request> {
    checked(
        value,
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
    let mut request = Request::default();
    if let Some(system) = value.get("system") {
        request.messages.push(Message {
            role: Role::System,
            blocks: super::content::text_blocks(system, WireApi::Messages)?,
        });
    }
    for message in value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::invalid_request)?
    {
        checked(message, &["role", "content"])?;
        request.messages.push(Message {
            role: super::content::role(message)?,
            blocks: super::content::text_blocks(
                message
                    .get("content")
                    .ok_or_else(AdapterError::invalid_request)?,
                WireApi::Messages,
            )?,
        });
    }
    request.tools = super::content::tools(value.get("tools"), WireApi::Messages)?;
    request.tool_choice = super::content::choice(value.get("tool_choice"), WireApi::Messages)?;
    if let Some(choice) = value.get("tool_choice") {
        checked(choice, &["type", "name", "disable_parallel_tool_use"])?;
        request.parallel_tools =
            optional_bool(choice, "disable_parallel_tool_use")?.map(|disabled| !disabled);
    }
    super::content::common(&mut request, value, "max_tokens", "top_p", "stop_sequences")?;
    if let Some(output) = value.get("output_config").filter(|v| !v.is_null()) {
        checked(output, &["format", "effort"])?;
        request.output_format =
            super::content::output_format(output.get("format").unwrap_or(&Value::Null))?;
        request.reasoning = output
            .get("effort")
            .and_then(Value::as_str)
            .map(|effort| Reasoning::Effort(effort.into()));
    }
    if let Some(thinking) = value.get("thinking").filter(|v| !v.is_null()) {
        checked(thinking, &["type", "budget_tokens"])?;
        match required_text(thinking, "type")? {
            "disabled" => request.reasoning = Some(Reasoning::Effort("none".into())),
            "adaptive" if request.reasoning.is_some() => {}
            "enabled" => {
                request.reasoning = Some(Reasoning::Budget(
                    optional_u64(thinking, "budget_tokens")?
                        .ok_or_else(AdapterError::invalid_request)?,
                ))
            }
            _ => return Err(AdapterError::reasoning_unsupported()),
        }
    }
    Ok(request)
}
