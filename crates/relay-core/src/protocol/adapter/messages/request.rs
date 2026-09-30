//! Responses requests translated into Anthropic Messages bodies.

use super::{
    bridged_namespace_tool_name, prepare_bridge_state, request_tool_catalog, AdapterError,
    AdapterResult, ClientToolTarget, MessagesBridgeRequest, MessagesBridgeState,
    MessagesReasoningMode, ResponsesToolKind, TranslatedTools,
};
use crate::CacheWriteTtl;
use serde_json::{json, Map, Value};

/// Converts a Codex Responses request to the Anthropic Messages contract.
///
/// JSON-schema functions retain their object input. Direct custom tools are
/// represented as a function with one raw-text field and are translated back
/// to the exact Responses custom-call shape before the client sees them.
/// Provider-hosted tools require a native route and are rejected before sending.
pub fn prepare_responses_to_messages(
    request: &Value,
    model: &str,
    stream: bool,
    reasoning_mode: MessagesReasoningMode,
    previous: Option<MessagesBridgeState>,
) -> AdapterResult<MessagesBridgeRequest> {
    prepare_responses_to_messages_scoped(request, model, stream, reasoning_mode, previous, "")
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
    previous: Option<MessagesBridgeState>,
    response_scope: &str,
) -> AdapterResult<MessagesBridgeRequest> {
    prepare_responses_to_messages_scoped_with_cache_ttl(
        request,
        model,
        stream,
        reasoning_mode,
        CacheWriteTtl::Provider,
        previous,
        response_scope,
    )
}

pub(crate) fn prepare_responses_to_messages_scoped_with_cache_ttl(
    request: &Value,
    model: &str,
    stream: bool,
    reasoning_mode: MessagesReasoningMode,
    cache_write_ttl: CacheWriteTtl,
    previous: Option<MessagesBridgeState>,
    response_scope: &str,
) -> AdapterResult<MessagesBridgeRequest> {
    let (object, mut state) = prepare_bridge_state(
        request,
        model,
        reasoning_mode,
        previous,
        crate::WireApi::Messages,
    )?;

    if let Some(tools) = request_tool_catalog(object)? {
        let TranslatedTools {
            upstream,
            client_tools,
        } = tools::translate_tools(&tools)?;
        state.tools = (!upstream.is_empty()).then_some(upstream);
        state.tool_targets = client_tools;
        // A Responses request that supplies a new tool catalog without an
        // explicit choice returns to the protocol default of automatic
        // selection. Retaining a previous restricted list would silently hide
        // newly supplied tools.
        state.tool_choice = None;
        state.tool_allow_list = None;
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        let translated = tools::translate_tool_choice(tool_choice, &state)?;
        state.tool_choice = translated.value;
        state.tool_allow_list = translated.allowed_names;
    }

    content::append_responses_input(
        &mut state,
        object
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?,
    )?;
    if state.messages.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    state.historical_system = state.system.clone();
    if let Some(instructions) = object.get("instructions").filter(|value| !value.is_null()) {
        content::append_system_value(&mut state, instructions)?;
    }

    let mut body = Map::from_iter([
        ("model".to_string(), Value::String(model.to_string())),
        ("messages".to_string(), Value::Array(state.messages.clone())),
        ("stream".to_string(), Value::Bool(stream)),
        (
            "max_tokens".to_string(),
            object
                .get("max_output_tokens")
                .cloned()
                .unwrap_or_else(|| Value::from(8_192_u64)),
        ),
    ]);
    if let Some(system) = state.system.clone() {
        body.insert("system".to_string(), system);
    }
    if let Some(tools) = state.upstream_tools() {
        body.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(tool_choice) = state.tool_choice.clone() {
        body.insert("tool_choice".to_string(), tool_choice);
    }
    if let Some(parallel) = object
        .get("parallel_tool_calls")
        .filter(|value| !value.is_null())
    {
        let parallel = parallel
            .as_bool()
            .ok_or_else(AdapterError::invalid_request)?;
        body.entry("tool_choice")
            .or_insert_with(|| json!({"type":"auto"}))["disable_parallel_tool_use"] =
            (!parallel).into();
    }
    if let Some(format) = object
        .get("text")
        .and_then(|text| text.get("format"))
        .filter(|value| !value.is_null())
    {
        match format.get("type").and_then(Value::as_str) {
            Some("text") => {}
            Some("json_schema") => {
                let schema = format
                    .get("schema")
                    .filter(|schema| schema.is_object())
                    .ok_or_else(AdapterError::invalid_request)?;
                body.insert(
                    "output_config".into(),
                    json!({"format":{"type":"json_schema","schema":schema}}),
                );
            }
            _ => return Err(AdapterError::parameter_unsupported()),
        }
    }
    if let Some(temperature) = object.get("temperature") {
        body.insert("temperature".to_string(), temperature.clone());
    }
    if let Some(top_p) = object.get("top_p") {
        body.insert("top_p".to_string(), top_p.clone());
    }
    if let Some(stop_sequences) = object.get("stop") {
        body.insert("stop_sequences".to_string(), stop_sequences.clone());
    }
    apply_reasoning(
        &mut body,
        object.get("reasoning"),
        reasoning_mode,
        object.contains_key("max_output_tokens"),
    )?;
    let mut upstream_body = Value::Object(body);
    apply_cache_write_ttl(&mut upstream_body, cache_write_ttl)?;
    Ok(MessagesBridgeRequest {
        upstream_body,
        state,
        response_scope: response_scope.trim().to_string(),
    })
}

pub(crate) fn apply_cache_write_ttl(
    body: &mut Value,
    cache_write_ttl: CacheWriteTtl,
) -> AdapterResult<()> {
    let Some(ttl) = cache_write_ttl.anthropic_ttl() else {
        return Ok(());
    };
    let object = body
        .as_object_mut()
        .ok_or_else(AdapterError::invalid_request)?;
    let mut updated = false;
    for key in ["system", "tools"] {
        if let Some(blocks) = object.get_mut(key).and_then(Value::as_array_mut) {
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
    if let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) {
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
    let prefix_marked = object
        .get_mut("system")
        .is_some_and(|system| set_last_cache_control(system, ttl));
    if !prefix_marked {
        if let Some(tools) = object.get_mut("tools") {
            set_last_cache_control(tools, ttl);
        }
    }
    if let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) {
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

fn set_last_cache_control(value: &mut Value, ttl: &str) -> bool {
    if let Some(block) = value.as_array_mut().and_then(|blocks| blocks.last_mut()) {
        return set_cache_control(block, ttl);
    }
    let Some(text) = value.as_str().map(str::to_string) else {
        return false;
    };
    *value = Value::Array(vec![json!({
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
    block.insert(
        "cache_control".to_string(),
        json!({"type": "ephemeral", "ttl": ttl}),
    );
    true
}

fn apply_reasoning(
    body: &mut Map<String, Value>,
    reasoning: Option<&Value>,
    mode: MessagesReasoningMode,
    explicit_max_tokens: bool,
) -> AdapterResult<()> {
    let effort = reasoning
        .and_then(Value::as_object)
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(Value::as_str)
        .map(|effort| effort.trim().to_ascii_lowercase())
        .filter(|effort| !effort.is_empty());
    let Some(effort) = effort else {
        return Ok(());
    };
    if effort == "none" {
        body.insert("thinking".to_string(), json!({"type":"disabled"}));
        return Ok(());
    }
    if ["temperature", "top_p"]
        .iter()
        .any(|name| body.get(*name).is_some_and(|value| !value.is_null()))
    {
        return Err(AdapterError::parameter_unsupported());
    }
    match mode {
        MessagesReasoningMode::Disabled => Err(AdapterError::reasoning_unsupported()),
        MessagesReasoningMode::Budget => {
            let budget_tokens = match effort.as_str() {
                "minimal" => 1_024,
                "low" => 4_096,
                "high" => 16_384,
                "xhigh" => 24_576,
                "max" | "ultra" => 32_000,
                "medium" => 8_192,
                _ => return Err(AdapterError::reasoning_unsupported()),
            };
            let minimum_max_tokens = budget_tokens + 1_024;
            if explicit_max_tokens
                && body
                    .get("max_tokens")
                    .and_then(Value::as_u64)
                    .is_none_or(|limit| limit < minimum_max_tokens)
            {
                return Err(AdapterError::parameter_unsupported());
            }
            let max_tokens = body
                .get("max_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default()
                .max(minimum_max_tokens);
            body.insert("max_tokens".to_string(), Value::from(max_tokens));
            body.insert(
                "thinking".to_string(),
                json!({"type": "enabled", "budget_tokens": budget_tokens}),
            );
            Ok(())
        }
        MessagesReasoningMode::Adaptive => {
            let effort = match effort.as_str() {
                "low" | "medium" | "high" | "max" => effort.as_str(),
                _ => return Err(AdapterError::reasoning_unsupported()),
            };
            body.insert("thinking".to_string(), json!({"type": "adaptive"}));
            body.entry("output_config").or_insert_with(|| json!({}))["effort"] = effort.into();
            Ok(())
        }
    }
}

mod content;
mod tools;

pub(in crate::protocol::adapter) use tools::custom_tool_input_schema;
