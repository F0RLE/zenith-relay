//! Reject Responses controls that a bridged provider cannot represent.

use super::{request_tool_catalog, AdapterError, AdapterResult};
use crate::WireApi;
use serde_json::Value;

/// Responses context management cannot be translated into another protocol.
/// Inspect only the control field and input item types, never text or nested
/// tool payloads.
fn validate_bridge_compaction(request: &Value) -> AdapterResult<()> {
    let is_compaction = super::super::compaction::is_compaction_item;
    let configured = request
        .get("context_management")
        .is_some_and(|context_management_value| match context_management_value {
            Value::Null => false,
            Value::Array(items) => !items.is_empty(),
            Value::Object(object_fields) => !object_fields.is_empty(),
            _ => true,
        });
    let history = request
        .get("input")
        .is_some_and(|request_input| match request_input {
            Value::Array(items) => items.iter().any(is_compaction),
            Value::Object(_) => is_compaction(request_input),
            _ => false,
        });
    if configured || history {
        return Err(
            AdapterError::compaction_unsupported().with_parameter(if configured {
                "context_management"
            } else {
                "input"
            }),
        );
    }
    Ok(())
}

pub(in crate::protocol::adapter) fn validate_responses_bridge_request(
    request: &Value,
    upstream: WireApi,
) -> AdapterResult<()> {
    validate_bridge_compaction(request)?;
    let request_object = request
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    let mut allowed = vec![
        "model",
        "stream",
        "input",
        "instructions",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "max_output_tokens",
        "temperature",
        "top_p",
        "stop",
        "text",
        "reasoning",
        "previous_response_id",
        "store",
        "background",
        "include",
        "context_management",
        "prompt_cache_key",
        "client_metadata",
    ];
    if upstream == WireApi::Gemini {
        allowed.extend([
            "top_k",
            "presence_penalty",
            "frequency_penalty",
            "seed",
            "response_format",
        ]);
    }
    if let Some((request_field_name, _)) =
        request_object.iter().find(|(field_name, field_value)| {
            !allowed.contains(&field_name.as_str()) && !field_value.is_null()
        })
    {
        // Only report contract field names, never arbitrary keys from a payload.
        let parameter = [
            "stream_options",
            "metadata",
            "access_programs",
            "prompt_cache_retention",
            "prompt_cache_options",
        ]
        .into_iter()
        .find(|known| *known == request_field_name)
        .unwrap_or("request");
        return Err(AdapterError::parameter_unsupported_for(parameter));
    }
    validate_responses_transport_controls(request)?;
    for field_name in ["stream", "parallel_tool_calls"] {
        if request_object
            .get(field_name)
            .is_some_and(|control_value| !control_value.is_null() && !control_value.is_boolean())
        {
            return Err(AdapterError::invalid_request());
        }
    }
    for field_name in [
        "temperature",
        "top_p",
        "top_k",
        "presence_penalty",
        "frequency_penalty",
        "seed",
    ] {
        if request_object
            .get(field_name)
            .is_some_and(|numeric_value| !numeric_value.is_null() && !numeric_value.is_number())
        {
            return Err(AdapterError::invalid_request());
        }
    }
    if request_object
        .get("max_output_tokens")
        .is_some_and(|token_value| {
            !token_value.is_null() && token_value.as_u64().is_none_or(|tokens| tokens == 0)
        })
    {
        return Err(AdapterError::invalid_request());
    }
    for field_name in ["instructions", "previous_response_id", "prompt_cache_key"] {
        if request_object
            .get(field_name)
            .is_some_and(|text_value| !text_value.is_null() && !text_value.is_string())
        {
            return Err(AdapterError::invalid_request());
        }
    }
    for field_name in ["text", "reasoning"] {
        if request_object
            .get(field_name)
            .is_some_and(|object_value| !object_value.is_null() && !object_value.is_object())
        {
            return Err(AdapterError::invalid_request());
        }
    }
    if let Some(reasoning) = request_object.get("reasoning").and_then(Value::as_object) {
        if reasoning
            .get("effort")
            .is_some_and(|effort_value| !effort_value.is_null() && !effort_value.is_string())
        {
            return Err(AdapterError::invalid_request());
        }
    }
    if let Some(format) = request
        .pointer("/text/format")
        .filter(|format_value| !format_value.is_null())
    {
        validate_bridge_fields(format, &["type", "name", "schema", "strict", "description"])?;
    }
    for field_name in ["background", "store"] {
        if request_object
            .get(field_name)
            .is_some_and(|option_value| !option_value.is_null() && option_value != false)
        {
            return Err(AdapterError::parameter_unsupported_for(field_name));
        }
    }
    if request_object
        .get("text")
        .and_then(Value::as_object)
        .is_some_and(|text| {
            text.iter()
                .any(|(field_name, field_value)| field_name != "format" && !field_value.is_null())
        })
    {
        let field = if request
            .pointer("/text/verbosity")
            .is_some_and(|verbosity_value| !verbosity_value.is_null())
        {
            "text.verbosity"
        } else {
            "text"
        };
        return Err(AdapterError::parameter_unsupported_for(field));
    }
    if request_object
        .get("reasoning")
        .and_then(Value::as_object)
        .is_some_and(|reasoning| {
            reasoning.iter().any(|(field_name, field_value)| {
                !matches!(field_name.as_str(), "effort" | "summary") && !field_value.is_null()
            })
        })
    {
        return Err(AdapterError::parameter_unsupported_for("reasoning"));
    }
    if upstream == WireApi::Gemini
        && request_object.get("parallel_tool_calls") == Some(&Value::Bool(false))
    {
        return Err(AdapterError::parameter_unsupported_for(
            "parallel_tool_calls",
        ));
    }
    if let Some(tools) = request_tool_catalog(request_object)? {
        for tool in &tools {
            validate_bridge_tool(tool)?;
        }
    }
    Ok(())
}

fn validate_responses_transport_controls(request: &Value) -> AdapterResult<()> {
    // The cache key is consumed by Relay affinity. Client metadata is tracing
    // information for the receiving server, not model input or provider metadata.
    if let Some(metadata) = request
        .get("client_metadata")
        .filter(|metadata_value| !metadata_value.is_null())
    {
        if metadata.as_object().is_none_or(|metadata_fields| {
            metadata_fields
                .values()
                .any(|metadata_field| !metadata_field.is_string())
        }) {
            return Err(AdapterError::invalid_request().with_parameter("client_metadata"));
        }
    }
    // `include` asks for optional output fields; it does not supply encrypted
    // history. Bridges keep their native continuation state locally. Never
    // fabricate an OpenAI encrypted blob or discard one received in input.
    if let Some(include) = request
        .get("include")
        .filter(|include_value| !include_value.is_null())
    {
        let include_items = include
            .as_array()
            .ok_or_else(|| AdapterError::invalid_request().with_parameter("include"))?;
        if include_items
            .iter()
            .any(|include_item| include_item.as_str() != Some("reasoning.encrypted_content"))
        {
            return Err(AdapterError::parameter_unsupported_for("include"));
        }
    }
    if let Some(summary) = request
        .pointer("/reasoning/summary")
        .filter(|summary_value| !summary_value.is_null())
    {
        if summary != "auto" {
            return Err(AdapterError::parameter_unsupported_for("reasoning.summary"));
        }
    }
    if request
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|input_items| {
            input_items.iter().any(|input_item| {
                input_item
                    .get("encrypted_content")
                    .is_some_and(|encrypted_content| !encrypted_content.is_null())
            })
        })
    {
        return Err(AdapterError::parameter_unsupported_for(
            "input.encrypted_content",
        ));
    }
    Ok(())
}

fn validate_bridge_tool(tool: &Value) -> AdapterResult<()> {
    let tool_object = tool
        .as_object()
        .ok_or_else(AdapterError::unsupported_tool)?;
    match tool_object.get("type").and_then(Value::as_str) {
        Some("function" | "custom") | None
            if tool_object.get("name").and_then(Value::as_str).is_some() =>
        {
            validate_bridge_fields(
                tool,
                &[
                    "type",
                    "name",
                    "description",
                    "parameters",
                    "format",
                    "strict",
                    "defer_loading",
                    "allowed_callers",
                ],
            )?;
            for field_name in ["strict", "defer_loading"] {
                if tool_object
                    .get(field_name)
                    .is_some_and(|flag_value| !flag_value.is_null() && !flag_value.is_boolean())
                {
                    return Err(AdapterError::invalid_request());
                }
            }
            // `defer_loading` and `allowed_callers` only steer OpenAI-side tool
            // search and caller policy. A bridged upstream sees an ordinary
            // client tool, so the flags are dropped instead of failing the request.
        }
        Some("namespace") => {
            validate_bridge_fields(tool, &["type", "name", "description", "tools"])?;
            if tool_object
                .get("name")
                .and_then(Value::as_str)
                .is_none_or(|tool_name| tool_name.is_empty())
            {
                return Err(AdapterError::unsupported_tool());
            }
            for child in tool_object
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(AdapterError::unsupported_tool)?
            {
                validate_bridge_tool(child)?;
            }
        }
        // Hosted tools (web search, image generation, ...) run on OpenAI's
        // servers and have no equivalent on a bridged upstream. They are left
        // out of the translated catalog; a forced choice of one still fails.
        Some(other) if !matches!(other, "function" | "custom") => {}
        _ => return Err(AdapterError::unsupported_tool()),
    }
    Ok(())
}

pub(in crate::protocol::adapter) fn validate_bridge_fields(
    field_container: &Value,
    allowed: &[&str],
) -> AdapterResult<()> {
    let field_entries = field_container
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    if field_entries.iter().any(|(field_name, field_value)| {
        !allowed.contains(&field_name.as_str()) && !field_value.is_null()
    }) {
        return Err(AdapterError::parameter_unsupported());
    }
    Ok(())
}
