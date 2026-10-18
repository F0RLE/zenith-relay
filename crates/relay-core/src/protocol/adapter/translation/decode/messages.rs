use super::super::{
    checked, optional_bool, optional_u64, required_text, AdapterError, AdapterResult, Message,
    Reasoning, Request, Role,
};
use crate::WireApi;
use serde_json::Value;

mod tool_search;

pub(super) fn decode(request_body: &Value) -> AdapterResult<Request> {
    let discovery = tool_search::prepare(request_body)?;
    let request_body = discovery.as_ref();
    super::super::options::messages_fields(
        request_body,
        &[
            "model",
            "stream",
            "messages",
            "system",
            "max_tokens",
            "temperature",
            "top_p",
            "top_k",
            "stop_sequences",
            "tools",
            "tool_choice",
            "thinking",
            "output_config",
            "metadata",
        ],
    )?;
    let mut decoded_request = Request {
        metadata: super::super::options::metadata(request_body, WireApi::Messages)?,
        ..Default::default()
    };
    super::content::sampling_extensions(&mut decoded_request, request_body, false)?;
    if let Some(system) = request_body
        .get("system")
        .filter(|system| !system.is_null())
    {
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
            role: super::content::role(message, WireApi::Messages)?,
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
    if let Some(choice) = request_body
        .get("tool_choice")
        .filter(|choice| !choice.is_null())
    {
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
        if output_config
            .get("effort")
            .is_some_and(|effort| !effort.is_null())
        {
            let effort = required_text(output_config, "effort")?;
            decoded_request.reasoning = Some(Reasoning::Effort(effort.into()));
        }
    }
    if let Some(thinking) = request_body
        .get("thinking")
        .filter(|thinking_value| !thinking_value.is_null())
    {
        checked(thinking, &["type", "budget_tokens"])?;
        let budget = optional_u64(thinking, "budget_tokens")?;
        match required_text(thinking, "type")? {
            "disabled" if budget.is_none() => {
                if decoded_request.reasoning
                    .as_ref()
                    .is_some_and(|reasoning| !matches!(reasoning, Reasoning::Effort(effort) if effort.trim().eq_ignore_ascii_case("none")))
                {
                    return Err(AdapterError::reasoning_unsupported());
                }
                decoded_request.reasoning = Some(Reasoning::Effort("none".into()));
            }
            "adaptive" if budget.is_none() => match decoded_request.reasoning.as_ref() {
                None => decoded_request.reasoning = Some(Reasoning::Adaptive),
                Some(Reasoning::Effort(effort)) if !effort.trim().eq_ignore_ascii_case("none") => {}
                _ => return Err(AdapterError::reasoning_unsupported()),
            },
            "enabled" if decoded_request.reasoning.is_none() => {
                let budget = budget.ok_or_else(AdapterError::invalid_request)?;
                if budget < 1_024 {
                    return Err(AdapterError::reasoning_unsupported());
                }
                decoded_request.reasoning = Some(Reasoning::Budget(budget));
            }
            _ => return Err(AdapterError::reasoning_unsupported()),
        }
    }
    Ok(decoded_request)
}
