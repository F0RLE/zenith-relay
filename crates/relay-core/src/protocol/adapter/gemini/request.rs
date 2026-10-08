//! Responses requests translated into Gemini generateContent bodies.

use super::{
    bridged_namespace_tool_name, prepare_bridge_state, request_tool_catalog, AdapterError,
    AdapterResult, ClientToolTarget, GeminiBridgeRequest, MessagesBridgeState,
    MessagesReasoningMode, ResponsesToolKind,
};
use serde_json::{json, Map, Value};

/// Compatibility entry point used by protocol-level callers without a
/// configured thinking policy.
#[cfg(test)]
pub fn prepare_responses_to_gemini(
    request: &Value,
    model: &str,
    _stream: bool,
    response_scope: &str,
    response_id_seed: &str,
) -> AdapterResult<GeminiBridgeRequest> {
    prepare_responses_to_gemini_with_reasoning(
        request,
        model,
        _stream,
        MessagesReasoningMode::Disabled,
        None,
        response_scope,
        response_id_seed,
    )
}

pub(crate) fn prepare_responses_to_gemini_with_reasoning(
    request: &Value,
    model: &str,
    _stream: bool,
    reasoning_mode: MessagesReasoningMode,
    previous_bridge_state: Option<MessagesBridgeState>,
    response_scope: &str,
    response_id_seed: &str,
) -> AdapterResult<GeminiBridgeRequest> {
    let (request_object, mut bridge_state) = prepare_bridge_state(
        request,
        model,
        reasoning_mode,
        previous_bridge_state,
        crate::WireApi::Gemini,
    )?;
    if let Some(tools) = request_tool_catalog(request_object)? {
        let (declarations, targets) = tools::translate_tools(&tools)?;
        bridge_state.tools = (!declarations.is_empty()).then_some(declarations);
        bridge_state.tool_targets = targets;
        bridge_state.tool_choice = None;
        bridge_state.tool_allow_list = None;
    }
    if let Some(choice) = request_object.get("tool_choice") {
        let (translated, allowed) = tools::translate_tool_choice(choice, &bridge_state)?;
        bridge_state.tool_choice = translated;
        bridge_state.tool_allow_list = allowed;
    }
    content::append_responses_input(
        &mut bridge_state,
        request_object
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?,
    )?;
    if bridge_state.messages.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    bridge_state.historical_system = bridge_state.system.clone();
    if let Some(instructions) = request_object
        .get("instructions")
        .filter(|instructions_value| !instructions_value.is_null())
    {
        content::append_system_parts(&mut bridge_state, content::content_parts(instructions)?)?;
    }

    let mut gemini_request_fields = Map::from_iter([(
        "contents".to_string(),
        Value::Array(bridge_state.messages.clone()),
    )]);
    if let Some(system) = bridge_state.system.clone() {
        gemini_request_fields.insert("systemInstruction".to_string(), system);
    }
    if let Some(tools) = bridge_state.upstream_tools() {
        gemini_request_fields.insert(
            "tools".to_string(),
            json!([{"functionDeclarations": tools}]),
        );
    }
    if let Some(tool_config) = bridge_state.tool_choice.clone() {
        gemini_request_fields.insert("toolConfig".to_string(), tool_config);
    }
    let mut generation = Map::new();
    copy_number(
        request_object,
        "temperature",
        "temperature",
        &mut generation,
    )?;
    copy_number(request_object, "top_p", "topP", &mut generation)?;
    copy_number(request_object, "top_k", "topK", &mut generation)?;
    copy_number(
        request_object,
        "presence_penalty",
        "presencePenalty",
        &mut generation,
    )?;
    copy_number(
        request_object,
        "frequency_penalty",
        "frequencyPenalty",
        &mut generation,
    )?;
    copy_number(request_object, "seed", "seed", &mut generation)?;
    copy_number(
        request_object,
        "max_output_tokens",
        "maxOutputTokens",
        &mut generation,
    )?;
    if let Some(stop) = request_object.get("stop") {
        generation.insert(
            "stopSequences".to_string(),
            Value::Array(stop_sequences(stop)?),
        );
    }
    apply_response_format(request_object, &mut generation)?;
    apply_reasoning(
        request_object.get("reasoning"),
        reasoning_mode,
        &mut generation,
    )?;
    if !generation.is_empty() {
        gemini_request_fields.insert("generationConfig".to_string(), Value::Object(generation));
    }
    let seed = response_id_seed
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>();
    if seed.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    let route = response_scope
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(32)
        .collect::<String>();
    let response_id = if route.is_empty() {
        format!("gemini_bridge_{seed}")
    } else {
        format!("gemini_bridge_{route}_{seed}")
    };
    Ok(GeminiBridgeRequest {
        upstream_body: Value::Object(gemini_request_fields),
        model: model.to_string(),
        response_id,
        bridge_state,
    })
}

fn apply_response_format(
    request_fields: &Map<String, Value>,
    generation_fields: &mut Map<String, Value>,
) -> AdapterResult<()> {
    let format_value = request_fields
        .get("text")
        .and_then(|text_value| text_value.get("format"))
        .or_else(|| request_fields.get("response_format"));
    let Some(format_value) = format_value else {
        return Ok(());
    };
    match format_value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("text")
    {
        "json_object" => {
            generation_fields.insert("responseMimeType".to_string(), json!("application/json"));
        }
        "json_schema" => {
            if format_value
                .get("strict")
                .or_else(|| format_value.pointer("/json_schema/strict"))
                == Some(&Value::Bool(true))
            {
                return Err(AdapterError::parameter_unsupported());
            }
            generation_fields.insert("responseMimeType".to_string(), json!("application/json"));
            let response_schema = format_value
                .get("schema")
                .or_else(|| {
                    format_value
                        .get("json_schema")
                        .and_then(|json_schema| json_schema.get("schema"))
                })
                .ok_or_else(AdapterError::invalid_request)?;
            generation_fields.insert("responseJsonSchema".to_string(), response_schema.clone());
        }
        "text" => {}
        _ => return Err(AdapterError::unsupported_binding()),
    }
    Ok(())
}

fn apply_reasoning(
    reasoning_value: Option<&Value>,
    mode: MessagesReasoningMode,
    generation_fields: &mut Map<String, Value>,
) -> AdapterResult<()> {
    let effort = reasoning_value
        .and_then(Value::as_object)
        .and_then(|reasoning_object| reasoning_object.get("effort"))
        .and_then(Value::as_str)
        .map(|effort_text| effort_text.trim().to_ascii_lowercase());
    let Some(effort) = effort else {
        return Ok(());
    };
    if mode == MessagesReasoningMode::Disabled {
        return Err(AdapterError::reasoning_unsupported());
    }
    if mode == MessagesReasoningMode::Adaptive {
        if !super::super::contracts::SourceAdapter::ResponsesToGemini
            .supports_reasoning_effort(mode, &effort)
        {
            return Err(AdapterError::reasoning_unsupported());
        }
        generation_fields.insert(
            "thinkingConfig".to_string(),
            json!({"thinkingLevel":effort,"includeThoughts":true}),
        );
        return Ok(());
    }
    let budget = match effort.as_str() {
        "none" => 0,
        "minimal" => 1_024,
        "low" => 4_096,
        "medium" => 8_192,
        "high" => 16_384,
        "xhigh" => 24_576,
        "max" | "ultra" => 32_000,
        _ => return Err(AdapterError::reasoning_unsupported()),
    };
    let mut config = Map::from_iter([("includeThoughts".to_string(), Value::Bool(true))]);
    if mode == MessagesReasoningMode::Budget {
        config.insert("thinkingBudget".to_string(), Value::from(budget));
    }
    generation_fields.insert("thinkingConfig".to_string(), Value::Object(config));
    Ok(())
}

fn copy_number(
    request_fields: &Map<String, Value>,
    source_field: &str,
    target_field: &str,
    generation_fields: &mut Map<String, Value>,
) -> AdapterResult<()> {
    let Some(source_value) = request_fields.get(source_field) else {
        return Ok(());
    };
    if !source_value.is_number() {
        return Err(AdapterError::invalid_request());
    }
    generation_fields.insert(target_field.to_string(), source_value.clone());
    Ok(())
}

fn stop_sequences(stop_value: &Value) -> AdapterResult<Vec<Value>> {
    match stop_value {
        Value::String(stop_text) if !stop_text.is_empty() => {
            Ok(vec![Value::String(stop_text.clone())])
        }
        Value::Array(stop_values)
            if stop_values.iter().all(|stop_value| {
                stop_value
                    .as_str()
                    .is_some_and(|stop_text| !stop_text.is_empty())
            }) =>
        {
            Ok(stop_values.clone())
        }
        _ => Err(AdapterError::invalid_request()),
    }
}

mod content;
mod tools;

pub(in crate::protocol::adapter) use content::append_message;
