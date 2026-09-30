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
    stream: bool,
    response_scope: &str,
    response_id_seed: &str,
) -> AdapterResult<GeminiBridgeRequest> {
    prepare_responses_to_gemini_with_reasoning(
        request,
        model,
        stream,
        MessagesReasoningMode::Disabled,
        None,
        response_scope,
        response_id_seed,
    )
}

pub(crate) fn prepare_responses_to_gemini_with_reasoning(
    request: &Value,
    model: &str,
    stream: bool,
    reasoning_mode: MessagesReasoningMode,
    previous: Option<MessagesBridgeState>,
    response_scope: &str,
    response_id_seed: &str,
) -> AdapterResult<GeminiBridgeRequest> {
    let (object, mut state) = prepare_bridge_state(
        request,
        model,
        reasoning_mode,
        previous,
        crate::WireApi::Gemini,
    )?;
    if let Some(tools) = request_tool_catalog(object)? {
        let (declarations, targets) = tools::translate_tools(&tools)?;
        state.tools = (!declarations.is_empty()).then_some(declarations);
        state.tool_targets = targets;
        state.tool_choice = None;
        state.tool_allow_list = None;
    }
    if let Some(choice) = object.get("tool_choice") {
        let (translated, allowed) = tools::translate_tool_choice(choice, &state)?;
        state.tool_choice = translated;
        state.tool_allow_list = allowed;
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
        content::append_system_parts(&mut state, content::content_parts(instructions)?)?;
    }

    let mut body = Map::from_iter([("contents".to_string(), Value::Array(state.messages.clone()))]);
    if let Some(system) = state.system.clone() {
        body.insert("systemInstruction".to_string(), system);
    }
    if let Some(tools) = state.upstream_tools() {
        body.insert(
            "tools".to_string(),
            json!([{"functionDeclarations": tools}]),
        );
    }
    if let Some(tool_config) = state.tool_choice.clone() {
        body.insert("toolConfig".to_string(), tool_config);
    }
    let mut generation = Map::new();
    copy_number(object, "temperature", "temperature", &mut generation)?;
    copy_number(object, "top_p", "topP", &mut generation)?;
    copy_number(object, "top_k", "topK", &mut generation)?;
    copy_number(
        object,
        "presence_penalty",
        "presencePenalty",
        &mut generation,
    )?;
    copy_number(
        object,
        "frequency_penalty",
        "frequencyPenalty",
        &mut generation,
    )?;
    copy_number(object, "seed", "seed", &mut generation)?;
    copy_number(
        object,
        "max_output_tokens",
        "maxOutputTokens",
        &mut generation,
    )?;
    if let Some(stop) = object.get("stop") {
        generation.insert(
            "stopSequences".to_string(),
            Value::Array(stop_sequences(stop)?),
        );
    }
    apply_response_format(object, &mut generation)?;
    apply_reasoning(object.get("reasoning"), reasoning_mode, &mut generation)?;
    if !generation.is_empty() {
        body.insert("generationConfig".to_string(), Value::Object(generation));
    }
    let _ = stream;

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
        upstream_body: Value::Object(body),
        model: model.to_string(),
        response_id,
        state,
    })
}

fn apply_response_format(
    object: &Map<String, Value>,
    generation: &mut Map<String, Value>,
) -> AdapterResult<()> {
    let format = object
        .get("text")
        .and_then(|value| value.get("format"))
        .or_else(|| object.get("response_format"));
    let Some(format) = format else {
        return Ok(());
    };
    match format.get("type").and_then(Value::as_str).unwrap_or("text") {
        "json_object" => {
            generation.insert("responseMimeType".to_string(), json!("application/json"));
        }
        "json_schema" => {
            if format
                .get("strict")
                .or_else(|| format.pointer("/json_schema/strict"))
                == Some(&Value::Bool(true))
            {
                return Err(AdapterError::parameter_unsupported());
            }
            generation.insert("responseMimeType".to_string(), json!("application/json"));
            let schema = format
                .get("schema")
                .or_else(|| {
                    format
                        .get("json_schema")
                        .and_then(|value| value.get("schema"))
                })
                .ok_or_else(AdapterError::invalid_request)?;
            generation.insert("responseJsonSchema".to_string(), schema.clone());
        }
        "text" => {}
        _ => return Err(AdapterError::unsupported_binding()),
    }
    Ok(())
}

fn apply_reasoning(
    reasoning: Option<&Value>,
    mode: MessagesReasoningMode,
    generation: &mut Map<String, Value>,
) -> AdapterResult<()> {
    let effort = reasoning
        .and_then(Value::as_object)
        .and_then(|value| value.get("effort"))
        .and_then(Value::as_str)
        .map(|value| value.trim().to_ascii_lowercase());
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
        generation.insert(
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
    generation.insert("thinkingConfig".to_string(), Value::Object(config));
    Ok(())
}

fn copy_number(
    input: &Map<String, Value>,
    source: &str,
    target: &str,
    output: &mut Map<String, Value>,
) -> AdapterResult<()> {
    let Some(value) = input.get(source) else {
        return Ok(());
    };
    if !value.is_number() {
        return Err(AdapterError::invalid_request());
    }
    output.insert(target.to_string(), value.clone());
    Ok(())
}

fn stop_sequences(value: &Value) -> AdapterResult<Vec<Value>> {
    match value {
        Value::String(value) if !value.is_empty() => Ok(vec![Value::String(value.clone())]),
        Value::Array(values)
            if values
                .iter()
                .all(|value| value.as_str().is_some_and(|value| !value.is_empty())) =>
        {
            Ok(values.clone())
        }
        _ => Err(AdapterError::invalid_request()),
    }
}

mod content;
mod tools;

pub(in crate::protocol::adapter) use content::append_message;
