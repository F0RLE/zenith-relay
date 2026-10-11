//! Responses requests translated into Anthropic Messages bodies.

use super::{
    prepare_bridge_state, request_tool_catalog, AdapterError, AdapterResult, ClientToolTarget,
    MessagesBridgeRequest, MessagesBridgeState, MessagesReasoningMode, ResponsesToolKind,
    TranslatedTools,
};
use crate::{AdapterRequestContext, CacheWriteTtl};
use serde_json::{json, Map, Value};

/// Converts a Codex Responses request to the Anthropic Messages contract.
///
/// JSON-schema functions retain their object input. Direct custom tools are
/// represented as a function with one raw-text field and are translated back
/// to the exact Responses custom-call shape before the client sees them.
/// Hosted tools are omitted; forcing one is rejected before sending.
pub fn prepare_responses_to_messages(
    request: &Value,
    model: &str,
    stream: bool,
    reasoning_mode: MessagesReasoningMode,
    previous_bridge_state: Option<MessagesBridgeState>,
) -> AdapterResult<MessagesBridgeRequest> {
    prepare_responses_to_messages_scoped(
        request,
        model,
        stream,
        reasoning_mode,
        previous_bridge_state,
        "",
    )
}

/// Variant of [`prepare_responses_to_messages`] that scopes generated local
/// response ids to one runtime route. A provider can legally reuse the same
/// upstream message id on two independent endpoints, so hashing only that
/// upstream id would let one continuation overwrite another.
pub fn prepare_responses_to_messages_scoped(
    request: &Value,
    model: &str,
    stream: bool,
    reasoning_mode: MessagesReasoningMode,
    previous_bridge_state: Option<MessagesBridgeState>,
    response_scope: &str,
) -> AdapterResult<MessagesBridgeRequest> {
    prepare_responses_to_messages_with_capabilities(
        AdapterRequestContext {
            client_wire_api: crate::WireApi::Responses,
            request,
            model,
            stream,
            reasoning_mode,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: previous_bridge_state,
            response_scope,
            response_id_seed: "",
        },
        None,
    )
}

pub(crate) fn prepare_responses_to_messages_with_capabilities(
    context: AdapterRequestContext<'_>,
    capabilities: Option<&crate::model_metadata::ModelCapabilities>,
) -> AdapterResult<MessagesBridgeRequest> {
    let AdapterRequestContext {
        request,
        model,
        stream,
        reasoning_mode,
        cache_write_ttl,
        previous,
        response_scope,
        ..
    } = context;
    let (request_object, mut bridge_state) = prepare_bridge_state(
        request,
        model,
        reasoning_mode,
        previous,
        crate::WireApi::Messages,
    )?;

    if let Some(tools) = request_tool_catalog(request_object)? {
        let TranslatedTools {
            upstream,
            client_tools,
        } = tools::translate_tools(&tools)?;
        bridge_state.tools = (!upstream.is_empty()).then_some(upstream);
        bridge_state.tool_targets = client_tools;
    }
    if let Some(tool_choice) = request_object.get("tool_choice") {
        let translated = tools::translate_tool_choice(tool_choice, &bridge_state)?;
        bridge_state.tool_choice = translated.translated_choice;
        bridge_state.tool_allow_list = translated.allowed_names;
    }

    content::append_responses_input(
        &mut bridge_state,
        request_object
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?,
    )?;
    bridge_state.validate_tool_history(crate::WireApi::Messages)?;
    if bridge_state.messages.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    bridge_state.historical_system = bridge_state.system.clone();
    if let Some(instructions) = request_object
        .get("instructions")
        .filter(|instructions_value| !instructions_value.is_null())
    {
        content::append_system_value(&mut bridge_state, instructions)?;
    }

    let mut upstream_fields = Map::from_iter([
        ("model".to_string(), Value::String(model.to_string())),
        (
            "messages".to_string(),
            Value::Array(bridge_state.messages.clone()),
        ),
        ("stream".to_string(), Value::Bool(stream)),
        (
            "max_tokens".to_string(),
            request_object
                .get("max_output_tokens")
                .filter(|limit| !limit.is_null())
                .cloned()
                .unwrap_or_else(|| Value::from(8_192_u64)),
        ),
    ]);
    if let Some(system) = bridge_state.system.clone() {
        upstream_fields.insert("system".to_string(), system);
    }
    if let Some(tools) = bridge_state.upstream_tools() {
        upstream_fields.insert("tools".to_string(), Value::Array(tools));
        if let Some(tool_choice) = bridge_state.tool_choice.clone() {
            upstream_fields.insert("tool_choice".to_string(), tool_choice);
        }
    }
    if let Some(parallel) = request_object
        .get("parallel_tool_calls")
        .filter(|parallel_tool_calls_value| !parallel_tool_calls_value.is_null())
    {
        let parallel = parallel
            .as_bool()
            .ok_or_else(AdapterError::invalid_request)?;
        if upstream_fields.contains_key("tools") {
            let choice = upstream_fields
                .entry("tool_choice")
                .or_insert_with(|| json!({"type":"auto"}));
            choice["disable_parallel_tool_use"] = (!parallel).into();
            bridge_state.tool_choice = Some(choice.clone());
        }
    }
    if let Some(format) = request_object
        .get("text")
        .and_then(|text| text.get("format"))
        .filter(|format_value| !format_value.is_null())
    {
        match format.get("type").and_then(Value::as_str) {
            Some("text") => {}
            Some("json_schema") => {
                let schema = format
                    .get("schema")
                    .filter(|schema| schema.is_object())
                    .ok_or_else(AdapterError::invalid_request)?;
                upstream_fields.insert(
                    "output_config".into(),
                    json!({"format":{"type":"json_schema","schema":schema}}),
                );
            }
            _ => return Err(AdapterError::parameter_unsupported()),
        }
    }
    if let Some(temperature) = request_object
        .get("temperature")
        .filter(|value| !value.is_null())
    {
        if temperature.as_f64().is_none() {
            return Err(AdapterError::invalid_request().with_parameter("temperature"));
        }
        upstream_fields.insert("temperature".to_string(), temperature.clone());
    }
    if let Some(top_p) = request_object.get("top_p").filter(|value| !value.is_null()) {
        if top_p.as_f64().is_none() {
            return Err(AdapterError::invalid_request().with_parameter("top_p"));
        }
        upstream_fields.insert("top_p".to_string(), top_p.clone());
    }
    if let Some(stop) = request_object.get("stop").filter(|value| !value.is_null()) {
        let sequences = match stop {
            Value::String(sequence) => vec![Value::String(sequence.clone())],
            Value::Array(sequences) if sequences.iter().all(Value::is_string) => sequences.clone(),
            _ => return Err(AdapterError::invalid_request().with_parameter("stop")),
        };
        upstream_fields.insert("stop_sequences".to_string(), Value::Array(sequences));
    }
    apply_reasoning(
        &mut upstream_fields,
        request_object.get("reasoning"),
        reasoning_mode,
        request_object
            .get("max_output_tokens")
            .is_some_and(|limit| !limit.is_null()),
        capabilities,
    )?;
    let mut upstream_body = Value::Object(upstream_fields);
    apply_cache_write_ttl(&mut upstream_body, cache_write_ttl)?;
    Ok(MessagesBridgeRequest {
        upstream_body,
        bridge_state,
        response_scope: response_scope.trim().to_string(),
    })
}

pub(crate) fn apply_cache_write_ttl(
    message_request: &mut Value,
    cache_write_ttl: CacheWriteTtl,
) -> AdapterResult<()> {
    let Some(ttl) = cache_write_ttl.anthropic_ttl() else {
        return Ok(());
    };
    let message_request_object = message_request
        .as_object_mut()
        .ok_or_else(AdapterError::invalid_request)?;
    let mut updated = false;
    for key in ["system", "tools"] {
        if let Some(blocks) = message_request_object
            .get_mut(key)
            .and_then(Value::as_array_mut)
        {
            for block in blocks {
                if block
                    .as_object()
                    .is_some_and(|block| block.contains_key("cache_control"))
                {
                    updated |= set_cache_control(block, ttl);
                }
            }
        }
    }
    if let Some(messages) = message_request_object
        .get_mut("messages")
        .and_then(Value::as_array_mut)
    {
        for message in messages {
            if let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) {
                for block in blocks {
                    if block
                        .as_object()
                        .is_some_and(|block| block.contains_key("cache_control"))
                    {
                        updated |= set_cache_control(block, ttl);
                    }
                }
            }
        }
    }
    if updated {
        return Ok(());
    }
    let prefix_marked = message_request_object
        .get_mut("system")
        .is_some_and(|system| set_last_cache_control(system, ttl));
    if !prefix_marked {
        if let Some(tools) = message_request_object.get_mut("tools") {
            set_last_cache_control(tools, ttl);
        }
    }
    if let Some(messages) = message_request_object
        .get_mut("messages")
        .and_then(Value::as_array_mut)
    {
        if let Some(content) = messages
            .iter_mut()
            .rev()
            .find_map(|message| message.get_mut("content"))
        {
            set_last_cache_control(content, ttl);
        }
    }
    Ok(())
}

fn set_last_cache_control(content_value: &mut Value, ttl: &str) -> bool {
    if let Some(blocks) = content_value.as_array_mut() {
        return blocks
            .iter_mut()
            .rev()
            .any(|block| set_cache_control(block, ttl));
    }
    let Some(text) = content_value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_string)
    else {
        return false;
    };
    *content_value = Value::Array(vec![json!({
        "type": "text",
        "text": text,
        "cache_control": {"type": "ephemeral", "ttl": ttl},
    })]);
    true
}

fn set_cache_control(block: &mut Value, ttl: &str) -> bool {
    let Some(block) = block.as_object_mut() else {
        return false;
    };
    // Native thinking stays unchanged; empty text is not a valid cache breakpoint.
    if matches!(
        block.get("type").and_then(Value::as_str),
        Some("thinking" | "redacted_thinking")
    ) || (block.get("type").and_then(Value::as_str) == Some("text")
        && block
            .get("text")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty))
    {
        return false;
    }
    block.insert(
        "cache_control".to_string(),
        json!({"type": "ephemeral", "ttl": ttl}),
    );
    true
}

fn apply_reasoning(
    message_fields: &mut Map<String, Value>,
    reasoning: Option<&Value>,
    mode: MessagesReasoningMode,
    explicit_max_tokens: bool,
    capabilities: Option<&crate::model_metadata::ModelCapabilities>,
) -> AdapterResult<()> {
    let Some(effort) = super::super::contracts::responses_reasoning_effort(reasoning)? else {
        return Ok(());
    };
    super::super::contracts::ReasoningPolicy::new(crate::WireApi::Messages, mode, capabilities)
        .apply_effort(message_fields, &effort, explicit_max_tokens)
}

mod content;
mod tools;

pub(in crate::protocol::adapter) use tools::custom_tool_input_schema;
