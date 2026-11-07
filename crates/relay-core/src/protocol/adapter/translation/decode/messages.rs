use super::super::{
    AdapterError, AdapterResult, Message, Reasoning, ReasoningDisplay, Request, Role, checked,
    optional_bool, optional_u64, required_text,
};
use crate::WireApi;
use serde_json::Value;

mod context_management;
mod inline_tools;
mod system;
mod tool_search;

pub(super) fn decode(request_body: &Value) -> AdapterResult<Request> {
    system::validate_positions(
        request_body
            .get("messages")
            .and_then(Value::as_array)
            .ok_or_else(AdapterError::invalid_request)?,
    )?;
    context_management::validate(request_body)?;
    let discovery = inline_tools::prepare(request_body)?;
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
            "context_management",
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
            blocks: system::text_blocks(system, "system", true)?,
        });
    }
    for message in request_body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::invalid_request)?
    {
        checked(message, &["role", "content"])?;
        let role = super::content::role(message, WireApi::Messages)?;
        let content = message
            .get("content")
            .ok_or_else(AdapterError::invalid_request)?;
        let blocks = if role == Role::System {
            system::text_blocks(content, "messages[].content", false)?
        } else {
            super::content::text_blocks(content, WireApi::Messages)?
        };
        decoded_request.messages.push(Message { role, blocks });
    }
    decoded_request.tools = super::content::tools(request_body.get("tools"), WireApi::Messages)?;
    decoded_request.tool_choice =
        super::content::choice(request_body.get("tool_choice"), WireApi::Messages)?;
    if let Some(choice) = request_body
        .get("tool_choice")
        .filter(|choice| !choice.is_null())
    {
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
        let kind = required_text(thinking, "type")?;
        checked(
            thinking,
            if kind == "disabled" {
                &["type", "budget_tokens"]
            } else {
                &["type", "budget_tokens", "display"]
            },
        )?;
        decoded_request.reasoning_display = match thinking
            .get("display")
            .filter(|value| !value.is_null())
        {
            None => None,
            Some(Value::String(display)) if display == "summarized" => {
                Some(ReasoningDisplay::Summarized)
            }
            Some(Value::String(display)) if display == "omitted" => Some(ReasoningDisplay::Omitted),
            _ => return Err(AdapterError::parameter_unsupported_for("thinking.display")),
        };
        let budget = optional_u64(thinking, "budget_tokens")?;
        match kind {
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
