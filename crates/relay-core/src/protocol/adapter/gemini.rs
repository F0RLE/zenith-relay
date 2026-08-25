use super::contracts::{
    AdapterError, AdapterResult, ClientToolTarget, MessagesBridgeState, MessagesReasoningMode,
    ResponsesToolKind,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_INLINE_MEDIA_BYTES: usize = 20 * 1024 * 1024;

/// Complete Responses-to-Gemini request plus the local state needed by the
/// next `previous_response_id` turn.
#[derive(Clone, Debug)]
pub struct GeminiBridgeRequest {
    pub(super) upstream_body: Value,
    pub(super) model: String,
    pub(super) response_id: String,
    pub(super) state: MessagesBridgeState,
}

impl GeminiBridgeRequest {
    pub fn upstream_body(&self) -> &Value {
        &self.upstream_body
    }
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn response_id(&self) -> &str {
        &self.response_id
    }
    pub(super) fn state(&self) -> &MessagesBridgeState {
        &self.state
    }
}

#[derive(Clone, Debug)]
pub struct GeminiBridgeResponse {
    pub response_body: Value,
    pub response_id: String,
    pub continuation: MessagesBridgeState,
}

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
    let object = request
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    for key in ["background", "include"] {
        if object.get(key).is_some_and(|value| !value.is_null()) {
            return Err(AdapterError::unsupported_binding());
        }
    }
    let previous_response_id = object
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut state = match (previous_response_id, previous) {
        (Some(_), Some(state)) if state.model == model => state,
        (Some(_), Some(_)) => return Err(AdapterError::continuation_mismatch()),
        (Some(_), None) => return Err(AdapterError::continuation_missing()),
        (None, _) => MessagesBridgeState::new(model, reasoning_mode),
    };
    if state.reasoning_mode != reasoning_mode {
        return Err(AdapterError::continuation_mismatch());
    }
    if previous_response_id.is_none() {
        if let Some(instructions) = object.get("instructions") {
            let parts = content_parts(instructions)?;
            append_system_parts(&mut state, parts)?;
        }
    } else if object.contains_key("instructions") {
        return Err(AdapterError::continuation_mismatch());
    }
    if let Some(tools) = request_tool_catalog(object)? {
        let (declarations, targets) = translate_tools(&tools)?;
        state.tools = (!declarations.is_empty()).then_some(declarations);
        state.tool_targets = targets;
        state.tool_choice = None;
        state.tool_allow_list = None;
    }
    if let Some(choice) = object.get("tool_choice") {
        let (translated, allowed) = translate_tool_choice(choice, &state)?;
        state.tool_choice = translated;
        state.tool_allow_list = allowed;
    }
    append_responses_input(
        &mut state,
        object
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?,
    )?;
    if state.messages.is_empty() {
        return Err(AdapterError::invalid_request());
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

pub fn translate_gemini_response(
    request: GeminiBridgeRequest,
    upstream: &Value,
) -> AdapterResult<GeminiBridgeResponse> {
    let candidate = first_candidate(upstream)?;
    let parts = candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::upstream_response_invalid)?;
    let (output, _) = responses_output_from_gemini_parts(&request, parts)?;
    if output.is_empty() {
        return Err(AdapterError::upstream_response_invalid());
    }
    let response_id = request.response_id.clone();
    let mut response_body = responses_body_from_output(
        &response_id,
        &request.model,
        output,
        upstream.get("usageMetadata"),
    );
    if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
        if !matches!(reason, "STOP" | "FINISH_REASON_UNSPECIFIED") {
            response_body["status"] = Value::String("incomplete".to_string());
            response_body["incomplete_details"] = json!({"reason": reason.to_ascii_lowercase()});
        }
    }
    let mut continuation = request.state.clone();
    append_message(&mut continuation, "model", parts.to_vec());
    Ok(GeminiBridgeResponse {
        response_body,
        response_id,
        continuation,
    })
}

pub(super) fn responses_body_from_output(
    response_id: &str,
    model: &str,
    output: Vec<Value>,
    usage: Option<&Value>,
) -> Value {
    json!({"id": response_id, "object": "response", "created_at": 0, "status": "completed",
        "model": model, "output": output, "usage": responses_usage(usage)})
}

pub(super) fn responses_usage(usage: Option<&Value>) -> Value {
    let input = usage
        .and_then(|u| u.get("promptTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output = usage
        .and_then(|u| u.get("candidatesTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let total = usage
        .and_then(|u| u.get("totalTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or_else(|| input.saturating_add(output));
    let mut result = Map::from_iter([
        ("input_tokens".to_string(), Value::from(input)),
        ("output_tokens".to_string(), Value::from(output)),
        ("total_tokens".to_string(), Value::from(total)),
    ]);
    if let Some(cached) = usage
        .and_then(|u| u.get("cachedContentTokenCount"))
        .and_then(Value::as_u64)
    {
        result.insert(
            "input_tokens_details".to_string(),
            json!({"cached_tokens": cached}),
        );
    }
    if let Some(reasoning) = usage
        .and_then(|u| u.get("thoughtsTokenCount"))
        .and_then(Value::as_u64)
    {
        result.insert(
            "output_tokens_details".to_string(),
            json!({"reasoning_tokens": reasoning}),
        );
    }
    Value::Object(result)
}

fn first_candidate(upstream: &Value) -> AdapterResult<&Value> {
    upstream
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .ok_or_else(AdapterError::upstream_response_invalid)
}

fn request_tool_catalog(object: &Map<String, Value>) -> AdapterResult<Option<Vec<Value>>> {
    let mut declared = false;
    let mut tools = Vec::new();
    if let Some(root) = object.get("tools") {
        declared = true;
        tools.extend(
            root.as_array()
                .ok_or_else(AdapterError::invalid_request)?
                .iter()
                .cloned(),
        );
    }
    if let Some(input) = object.get("input").and_then(Value::as_array) {
        for item in input {
            if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
                continue;
            }
            declared = true;
            tools.extend(
                item.get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(AdapterError::invalid_request)?
                    .iter()
                    .cloned(),
            );
        }
    }
    Ok(declared.then_some(tools))
}

fn translate_tools(
    tools: &[Value],
) -> AdapterResult<(Vec<Value>, BTreeMap<String, ClientToolTarget>)> {
    let mut declarations = Vec::new();
    let mut targets = BTreeMap::new();
    for value in tools {
        let tool = value
            .as_object()
            .ok_or_else(AdapterError::invalid_request)?;
        match tool.get("type").and_then(Value::as_str) {
            Some("namespace") => {
                let namespace = tool
                    .get("name")
                    .or_else(|| tool.get("namespace"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(AdapterError::invalid_request)?;
                let Some(children) = tool.get("tools").and_then(Value::as_array) else {
                    continue;
                };
                for child in children {
                    let Some(child) = child.as_object() else {
                        continue;
                    };
                    if matches!(
                        child.get("type").and_then(Value::as_str),
                        Some("function" | "custom") | None
                    ) {
                        translate_gemini_tool(
                            &mut declarations,
                            &mut targets,
                            child,
                            Some(namespace),
                        )?;
                    }
                }
            }
            Some("function" | "custom") | None if tool.get("name").is_some() => {
                translate_gemini_tool(&mut declarations, &mut targets, tool, None)?;
            }
            _ => {}
        }
    }
    Ok((declarations, targets))
}

fn translate_gemini_tool(
    declarations: &mut Vec<Value>,
    targets: &mut BTreeMap<String, ClientToolTarget>,
    tool: &Map<String, Value>,
    namespace: Option<&str>,
) -> AdapterResult<()> {
    let kind = match tool.get("type").and_then(Value::as_str) {
        Some("custom") => ResponsesToolKind::Custom,
        Some("function") | None => ResponsesToolKind::Function,
        _ => return Ok(()),
    };
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    let upstream_name = namespace
        .map(|namespace| bridged_namespace_tool_name(namespace, name))
        .unwrap_or_else(|| name.to_string());
    if targets.contains_key(&upstream_name) {
        return Err(AdapterError::invalid_request());
    }
    let mut declaration =
        Map::from_iter([("name".to_string(), Value::String(upstream_name.clone()))]);
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(namespace) = namespace {
        let mut description_value = format!("Codex namespace `{namespace}` tool `{name}`.");
        if let Some(description) = description {
            description_value.push(' ');
            description_value.push_str(description);
        }
        declaration.insert("description".to_string(), Value::String(description_value));
    } else if let Some(description) = description {
        declaration.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
    }
    let parameters = if kind == ResponsesToolKind::Custom {
        json!({"type":"object","properties":{"input":{"type":"string"}},"required":["input"]})
    } else {
        tool.get("parameters")
            .or_else(|| tool.get("parameters_json_schema"))
            .or_else(|| tool.get("input_schema"))
            .cloned()
            .unwrap_or_else(|| json!({"type":"object","properties":{}}))
    };
    if !parameters.is_object() {
        return Err(AdapterError::invalid_request());
    }
    declaration.insert(
        "parameters".to_string(),
        sanitize_gemini_schema(&parameters),
    );
    declarations.push(Value::Object(declaration));
    targets.insert(
        upstream_name,
        ClientToolTarget {
            kind,
            name: name.to_string(),
            namespace: namespace.map(str::to_string),
        },
    );
    Ok(())
}

fn bridged_namespace_tool_name(namespace: &str, name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update((namespace.len() as u64).to_le_bytes());
    hasher.update(namespace.as_bytes());
    hasher.update((name.len() as u64).to_le_bytes());
    hasher.update(name.as_bytes());
    let digest = hasher.finalize();
    format!("relay_ns_{}", hex::encode(&digest[..12]))
}

fn translate_tool_choice(
    choice: &Value,
    state: &MessagesBridgeState,
) -> AdapterResult<(Option<Value>, Option<BTreeSet<String>>)> {
    let mode = choice
        .as_str()
        .or_else(|| choice.get("mode").and_then(Value::as_str));
    if let Some(mode) = mode {
        let mode = match mode.to_ascii_lowercase().as_str() {
            "none" => "NONE",
            "required" => "ANY",
            _ => "AUTO",
        };
        return Ok((Some(json!({"functionCallingConfig":{"mode":mode}})), None));
    }
    let object = choice
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    if object.get("type").and_then(Value::as_str) == Some("allowed_tools") {
        let mode = match object.get("mode").and_then(Value::as_str).unwrap_or("auto") {
            "required" => "ANY",
            "none" => "NONE",
            _ => "AUTO",
        };
        let mut allowed = BTreeSet::new();
        if let Some(tools) = object.get("tools") {
            for tool in tools.as_array().ok_or_else(AdapterError::invalid_request)? {
                if let Some(tool) = tool.as_object() {
                    if tool.get("type").and_then(Value::as_str) == Some("namespace") {
                        let namespace = tool
                            .get("name")
                            .or_else(|| tool.get("namespace"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty());
                        if let Some(namespace) = namespace {
                            allowed.extend(state.tool_targets.iter().filter_map(
                                |(upstream_name, target)| {
                                    (target.namespace.as_deref() == Some(namespace)
                                        && state.allows_tool_name(upstream_name))
                                    .then_some(upstream_name.clone())
                                },
                            ));
                        }
                    } else if let Some(name) = selected_upstream_tool_name(state, tool) {
                        allowed.insert(name);
                    }
                }
            }
        }
        return Ok((
            Some(
                json!({"functionCallingConfig":{"mode":mode,"allowedFunctionNames":allowed.iter().cloned().collect::<Vec<_>>()}}),
            ),
            (!allowed.is_empty()).then_some(allowed),
        ));
    }
    let Some(upstream_name) = selected_upstream_tool_name(state, object) else {
        return Err(AdapterError::invalid_request());
    };
    Ok((
        Some(
            json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":[upstream_name]}}),
        ),
        Some(BTreeSet::from([upstream_name])),
    ))
}

fn selected_upstream_tool_name(
    state: &MessagesBridgeState,
    tool: &Map<String, Value>,
) -> Option<String> {
    let kind = ResponsesToolKind::from_definition(tool).ok()?;
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())?;
    let namespace = tool
        .get("namespace")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|namespace| !namespace.is_empty());
    let upstream_name = state.upstream_tool_name(namespace, name)?;
    (state.client_tool_kind(upstream_name) == Some(kind)).then(|| upstream_name.to_string())
}

fn append_responses_input(state: &mut MessagesBridgeState, input: &Value) -> AdapterResult<()> {
    match input {
        Value::String(text) => {
            if !text.is_empty() {
                append_message(state, "user", vec![json!({"text":text})]);
            }
            Ok(())
        }
        Value::Array(items) => {
            for value in items {
                let item = value
                    .as_object()
                    .ok_or_else(AdapterError::invalid_request)?;
                if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                    continue;
                }
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call_output" | "custom_tool_call_output") => {
                        let call_id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let name = find_call_name(state, call_id)
                            .or_else(|| item.get("name").and_then(Value::as_str))
                            .ok_or_else(AdapterError::invalid_request)?;
                        let response = if item.get("type").and_then(Value::as_str)
                            == Some("custom_tool_call_output")
                        {
                            json!({"input": output_text(item.get("output").ok_or_else(AdapterError::invalid_request)?)?})
                        } else {
                            output_value(
                                item.get("output")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?
                        };
                        append_message(
                            state,
                            "user",
                            vec![
                                json!({"functionResponse":{"name":name,"response":response,"id":call_id}}),
                            ],
                        );
                    }
                    Some("function_call" | "custom_tool_call") => {
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let namespace = item
                            .get("namespace")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty());
                        let upstream_name = state
                            .upstream_tool_name(namespace, name)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let call_id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let args = if item.get("type").and_then(Value::as_str)
                            == Some("custom_tool_call")
                        {
                            json!({"input": output_text(item.get("input").ok_or_else(AdapterError::invalid_request)?)?})
                        } else {
                            serde_json::from_str::<Value>(
                                item.get("arguments")
                                    .and_then(Value::as_str)
                                    .unwrap_or("{}"),
                            )
                            .map_err(|_| AdapterError::invalid_request())?
                        };
                        if !args.is_object() {
                            return Err(AdapterError::invalid_request());
                        }
                        append_message(
                            state,
                            "model",
                            vec![
                                json!({"functionCall":{"name":upstream_name,"args":args,"id":call_id}}),
                            ],
                        );
                    }
                    Some("reasoning") => {
                        if state.messages.is_empty() {
                            return Err(AdapterError::continuation_missing());
                        }
                    }
                    Some("message") | None => {
                        let role = item
                            .get("role")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        let role = match role {
                            "user" => "user",
                            "assistant" => "model",
                            "developer" | "system" => "system",
                            _ => return Err(AdapterError::invalid_request()),
                        };
                        let parts = content_parts(
                            item.get("content")
                                .ok_or_else(AdapterError::invalid_request)?,
                        )?;
                        if parts.is_empty() {
                            continue;
                        }
                        if role == "system" {
                            append_system_parts(state, parts)?;
                        } else {
                            append_message(state, role, parts);
                        }
                    }
                    _ => return Err(AdapterError::invalid_request()),
                }
            }
            Ok(())
        }
        _ => Err(AdapterError::invalid_request()),
    }
}

fn append_message(state: &mut MessagesBridgeState, role: &str, parts: Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = state.messages.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some(role) {
            if let Some(existing) = last.get_mut("parts").and_then(Value::as_array_mut) {
                existing.extend(parts);
                return;
            }
        }
    }
    state.messages.push(json!({"role": role, "parts": parts}));
}

fn find_call_name<'a>(state: &'a MessagesBridgeState, call_id: &str) -> Option<&'a str> {
    state.messages.iter().rev().find_map(|message| {
        message
            .get("parts")
            .and_then(Value::as_array)
            .and_then(|parts| {
                parts.iter().rev().find_map(|part| {
                    let call = part.get("functionCall")?.as_object()?;
                    (call.get("id").and_then(Value::as_str) == Some(call_id))
                        .then(|| call.get("name").and_then(Value::as_str))
                        .flatten()
                })
            })
    })
}

fn append_system_parts(state: &mut MessagesBridgeState, parts: Vec<Value>) -> AdapterResult<()> {
    if parts.is_empty() {
        return Ok(());
    }
    match state.system.take() {
        None => state.system = Some(json!({"parts": parts})),
        Some(Value::Object(mut object)) => {
            let existing = object
                .entry("parts".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            let Some(existing) = existing.as_array_mut() else {
                return Err(AdapterError::invalid_request());
            };
            existing.extend(parts);
            state.system = Some(Value::Object(object));
        }
        Some(_) => return Err(AdapterError::invalid_request()),
    }
    Ok(())
}

fn content_parts(content: &Value) -> AdapterResult<Vec<Value>> {
    match content {
        Value::String(text) if !text.is_empty() => Ok(vec![json!({"text":text})]),
        Value::String(_) => Ok(Vec::new()),
        Value::Array(parts) => parts.iter().map(content_part).collect(),
        _ => Err(AdapterError::invalid_request()),
    }
}

fn content_part(value: &Value) -> AdapterResult<Value> {
    let part = value
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    match part.get("type").and_then(Value::as_str) {
        Some("input_text" | "output_text" | "text") => part
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(|text| json!({"text":text}))
            .ok_or_else(AdapterError::invalid_request),
        Some("input_image" | "output_image") => image_part(part),
        Some("input_file" | "output_file") => file_part(part),
        _ => Err(AdapterError::unsupported_binding()),
    }
}

fn image_part(part: &Map<String, Value>) -> AdapterResult<Value> {
    let url = part
        .get("image_url")
        .or_else(|| part.get("url"))
        .and_then(Value::as_str)
        .ok_or_else(AdapterError::invalid_request)?;
    if let Some((header, data)) = url.split_once(',') {
        let mime = header
            .strip_prefix("data:")
            .and_then(|value| value.strip_suffix(";base64"))
            .filter(|value| {
                matches!(
                    *value,
                    "image/gif" | "image/jpeg" | "image/png" | "image/webp"
                )
            })
            .ok_or_else(AdapterError::invalid_request)?;
        let decoded = STANDARD
            .decode(data)
            .map_err(|_| AdapterError::invalid_request())?;
        if decoded.len() > MAX_INLINE_MEDIA_BYTES {
            return Err(AdapterError::invalid_request());
        }
        return Ok(json!({"inlineData":{"mimeType":mime,"data":data}}));
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(AdapterError::invalid_request());
    }
    Ok(json!({"fileData":{"fileUri":url,"mimeType":"image/*"}}))
}

fn file_part(part: &Map<String, Value>) -> AdapterResult<Value> {
    let url = part
        .get("file_url")
        .or_else(|| part.get("file_uri"))
        .or_else(|| part.get("url"))
        .and_then(Value::as_str)
        .filter(|value| value.starts_with("https://") || value.starts_with("http://"))
        .ok_or_else(AdapterError::invalid_request)?;
    let mime = part
        .get("mime_type")
        .or_else(|| part.get("mimeType"))
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream");
    Ok(json!({"fileData":{"fileUri":url,"mimeType":mime}}))
}

fn output_text(value: &Value) -> AdapterResult<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .map(str::to_string)
            .reduce(|mut left, right| {
                left.push_str(&right);
                left
            })
            .ok_or_else(AdapterError::invalid_request),
        _ => Err(AdapterError::invalid_request()),
    }
}

fn output_value(value: &Value) -> AdapterResult<Value> {
    match value {
        Value::String(text) => {
            Ok(serde_json::from_str(text).unwrap_or_else(|_| json!({"output":text})))
        }
        Value::Object(_) => Ok(value.clone()),
        Value::Array(parts) => {
            let mut text = Vec::new();
            let mut media = Vec::new();
            for part in parts {
                let object = part.as_object().ok_or_else(AdapterError::invalid_request)?;
                match object.get("type").and_then(Value::as_str) {
                    Some("input_text" | "output_text" | "text") => text.push(
                        object
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?
                            .to_string(),
                    ),
                    Some("input_image" | "output_image" | "input_file" | "output_file") => {
                        media.push(content_part(part)?);
                    }
                    _ => text.push(
                        serde_json::to_string(part).map_err(|_| AdapterError::invalid_request())?,
                    ),
                }
            }
            let mut response = Map::new();
            if !text.is_empty() {
                response.insert("output".to_string(), Value::String(text.join("\n")));
            }
            if !media.is_empty() {
                response.insert("parts".to_string(), Value::Array(media));
            }
            if response.is_empty() {
                response.insert("output".to_string(), Value::String(String::new()));
            }
            Ok(Value::Object(response))
        }
        _ => Err(AdapterError::invalid_request()),
    }
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
            generation.insert("responseMimeType".to_string(), json!("application/json"));
            let schema = format
                .get("schema")
                .or_else(|| {
                    format
                        .get("json_schema")
                        .and_then(|value| value.get("schema"))
                })
                .ok_or_else(AdapterError::invalid_request)?;
            generation.insert("responseSchema".to_string(), sanitize_gemini_schema(schema));
        }
        "text" => {}
        _ => return Err(AdapterError::unsupported_binding()),
    }
    Ok(())
}

/// Gemini's structured-output schema is a deliberately small subset of JSON
/// Schema. Bridge requests must remove draft keywords before they reach the
/// provider; native Gemini requests are never passed through this function.
fn sanitize_gemini_schema(schema: &Value) -> Value {
    const SUPPORTED_FIELDS: [&str; 8] = [
        "type",
        "description",
        "properties",
        "required",
        "items",
        "enum",
        "title",
        "nullable",
    ];

    let Value::Object(object) = schema else {
        return schema.clone();
    };
    let mut sanitized = Map::new();
    for field in SUPPORTED_FIELDS {
        let Some(value) = object.get(field) else {
            continue;
        };
        let value = match field {
            "properties" => match value.as_object() {
                Some(properties) => Value::Object(
                    properties
                        .iter()
                        .map(|(name, property)| (name.clone(), sanitize_gemini_schema(property)))
                        .collect(),
                ),
                None => value.clone(),
            },
            "items" => sanitize_gemini_schema(value),
            _ => value.clone(),
        };
        sanitized.insert(field.to_string(), value);
    }
    Value::Object(sanitized)
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
        .map(|value| value.to_ascii_lowercase())
        .filter(|value| value != "none");
    let Some(effort) = effort else {
        return Ok(());
    };
    if mode == MessagesReasoningMode::Disabled {
        return Err(AdapterError::reasoning_unsupported());
    }
    let budget = match effort.as_str() {
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

fn responses_output_from_gemini_parts(
    request: &GeminiBridgeRequest,
    parts: &[Value],
) -> AdapterResult<(Vec<Value>, Vec<Value>)> {
    let mut output = Vec::new();
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut call_index = 0_usize;
    let flush_text = |output: &mut Vec<Value>, text: &mut String| {
        if !text.is_empty() {
            output.push(json!({"id":format!("msg_{}_{}",request.response_id(),output.len()),"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":std::mem::take(text),"annotations":[]}]}));
        }
    };
    let flush_reasoning = |output: &mut Vec<Value>, reasoning: &mut String| {
        if !reasoning.is_empty() {
            output.push(json!({"id":format!("reasoning_{}_{}",request.response_id(),output.len()),"type":"reasoning","status":"completed","summary":[{"type":"summary_text","text":std::mem::take(reasoning)}]}));
        }
    };
    for value in parts {
        let part = value
            .as_object()
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        if let Some(value) = part.get("text").and_then(Value::as_str) {
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                flush_text(&mut output, &mut text);
                reasoning.push_str(value);
            } else {
                flush_reasoning(&mut output, &mut reasoning);
                text.push_str(value);
            }
            continue;
        }
        let Some(call) = part.get("functionCall").and_then(Value::as_object) else {
            if part.get("thoughtSignature").is_some() {
                continue;
            }
            return Err(AdapterError::upstream_response_invalid());
        };
        flush_text(&mut output, &mut text);
        flush_reasoning(&mut output, &mut reasoning);
        let upstream_name = call
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| request.state.allows_tool_name(name))
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        let target = request
            .state
            .client_tool(upstream_name)
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        let call_id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("call_{}_{}", request.response_id(), call_index));
        let args =
            function_call_args(call).map_err(|_| AdapterError::upstream_response_invalid())?;
        let mut item = if target.kind == ResponsesToolKind::Custom {
            let input = args
                .get("input")
                .and_then(Value::as_str)
                .ok_or_else(AdapterError::upstream_response_invalid)?;
            json!({"id":call_id,"type":"custom_tool_call","status":"completed","call_id":call_id,"name":target.name,"input":input})
        } else {
            json!({"id":call_id,"type":"function_call","status":"completed","call_id":call_id,"name":target.name,"arguments":serde_json::to_string(&args).map_err(|_| AdapterError::upstream_response_invalid())?})
        };
        if let Some(namespace) = target.namespace.as_ref() {
            item["namespace"] = Value::String(namespace.clone());
        }
        output.push(item);
        call_index = call_index.saturating_add(1);
    }
    flush_text(&mut output, &mut text);
    flush_reasoning(&mut output, &mut reasoning);
    Ok((output, Vec::new()))
}

/// Gemini's Vertex streaming contract can split function arguments into
/// `partialArgs` patches. The regular API returns `args`, but accepting the
/// patch form here keeps the non-stream and stream bridges on one contract.
pub(super) fn function_call_args(call: &Map<String, Value>) -> AdapterResult<Value> {
    let mut args = call.get("args").cloned().unwrap_or_else(|| json!({}));
    if !args.is_object() {
        return Err(AdapterError::invalid_request());
    }
    if let Some(partial_args) = call.get("partialArgs") {
        apply_partial_args(&mut args, partial_args)?;
    }
    Ok(args)
}

pub(super) fn apply_partial_args(target: &mut Value, partial_args: &Value) -> AdapterResult<()> {
    let patches = partial_args
        .as_array()
        .ok_or_else(AdapterError::invalid_request)?;
    for patch in patches {
        let Some(patch) = patch.as_object() else {
            return Err(AdapterError::invalid_request());
        };
        let Some(path) = patch.get("jsonPath").and_then(Value::as_str) else {
            return Err(AdapterError::invalid_request());
        };
        let Some(value) = partial_arg_value(patch) else {
            continue;
        };
        // Vertex emits an empty string patch after a value while it is still
        // assembling the argument. Do not erase the last non-empty value.
        if value.as_str().is_some_and(str::is_empty) {
            continue;
        }
        set_json_path(target, path, value)?;
    }
    Ok(())
}

fn partial_arg_value(patch: &Map<String, Value>) -> Option<Value> {
    for key in [
        "stringValue",
        "numberValue",
        "boolValue",
        "booleanValue",
        "nullValue",
        "jsonValue",
        "value",
    ] {
        if let Some(value) = patch.get(key) {
            if key == "jsonValue" {
                if let Some(text) = value.as_str() {
                    return serde_json::from_str(text).ok();
                }
            }
            return Some(value.clone());
        }
    }
    None
}

#[derive(Clone, Debug)]
enum JsonPathSegment {
    Key(String),
    Index(usize),
}

fn set_json_path(target: &mut Value, path: &str, value: Value) -> AdapterResult<()> {
    let segments = parse_json_path(path).ok_or_else(AdapterError::invalid_request)?;
    if segments.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    set_json_path_segments(target, &segments, value)
}

fn parse_json_path(path: &str) -> Option<Vec<JsonPathSegment>> {
    let bytes = path.as_bytes();
    if bytes.first().copied() != Some(b'$') {
        return None;
    }
    let mut index = 1;
    let mut segments = Vec::new();
    while index < bytes.len() {
        match bytes[index] {
            b'.' => {
                index += 1;
                let start = index;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'-'))
                {
                    index += 1;
                }
                if start == index {
                    return None;
                }
                segments.push(JsonPathSegment::Key(path[start..index].to_string()));
            }
            b'[' => {
                index += 1;
                if bytes.get(index).copied() == Some(b'\"') {
                    index += 1;
                    let start = index;
                    while index < bytes.len() && bytes[index] != b'\"' {
                        index += 1;
                    }
                    if index >= bytes.len() {
                        return None;
                    }
                    let key = path[start..index].to_string();
                    index += 1;
                    if bytes.get(index).copied() != Some(b']') {
                        return None;
                    }
                    index += 1;
                    segments.push(JsonPathSegment::Key(key));
                } else {
                    let start = index;
                    while index < bytes.len() && bytes[index].is_ascii_digit() {
                        index += 1;
                    }
                    if start == index || bytes.get(index).copied() != Some(b']') {
                        return None;
                    }
                    let value = path[start..index].parse().ok()?;
                    index += 1;
                    segments.push(JsonPathSegment::Index(value));
                }
            }
            _ => return None,
        }
    }
    Some(segments)
}

fn set_json_path_segments(
    current: &mut Value,
    segments: &[JsonPathSegment],
    value: Value,
) -> AdapterResult<()> {
    let Some(segment) = segments.first() else {
        *current = value;
        return Ok(());
    };
    let last = segments.len() == 1;
    match segment {
        JsonPathSegment::Key(key) => {
            let object = current
                .as_object_mut()
                .ok_or_else(AdapterError::invalid_request)?;
            if last {
                object.insert(key.clone(), value);
                return Ok(());
            }
            let next_is_index = matches!(segments[1], JsonPathSegment::Index(_));
            let child = object.entry(key.clone()).or_insert_with(|| {
                if next_is_index {
                    Value::Array(Vec::new())
                } else {
                    Value::Object(Map::new())
                }
            });
            set_json_path_segments(child, &segments[1..], value)
        }
        JsonPathSegment::Index(index) => {
            let array = current
                .as_array_mut()
                .ok_or_else(AdapterError::invalid_request)?;
            if *index >= array.len() {
                array.resize_with(index.saturating_add(1), || Value::Null);
            }
            if last {
                array[*index] = value;
                return Ok(());
            }
            if array[*index].is_null() {
                array[*index] = if matches!(segments[1], JsonPathSegment::Index(_)) {
                    Value::Array(Vec::new())
                } else {
                    Value::Object(Map::new())
                };
            }
            set_json_path_segments(&mut array[*index], &segments[1..], value)
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_tools_images_and_tool_result_continuation() {
        let request = json!({"instructions":"Use tools.","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Inspect"},{"type":"input_image","image_url":"data:image/png;base64,YQ=="}]}],"tools":[{"type":"function","name":"run","parameters":{"type":"object"}}],"tool_choice":{"type":"function","name":"run"}});
        let prepared = prepare_responses_to_gemini_with_reasoning(
            &request,
            "gemini-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
            "route",
            "req-1",
        )
        .unwrap();
        assert_eq!(
            prepared.upstream_body["contents"][0]["parts"][1]["inlineData"]["mimeType"],
            "image/png"
        );
        assert_eq!(
            prepared.upstream_body["tools"][0]["functionDeclarations"][0]["name"],
            "run"
        );
        let response = translate_gemini_response(prepared,&json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"run","args":{"command":"pwd"},"id":"call_1"}}]}}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3}})).unwrap();
        assert_eq!(response.response_body["output"][0]["type"], "function_call");
        let continued = prepare_responses_to_gemini_with_reasoning(&json!({"previous_response_id":response.response_id,"input":[{"type":"function_call_output","call_id":"call_1","output":"/tmp"}]}),"gemini-test",false,MessagesReasoningMode::Disabled,Some(response.continuation),"route","req-2").unwrap();
        assert_eq!(
            continued.upstream_body["contents"][2]["parts"][0]["functionResponse"]["name"],
            "run"
        );
    }

    #[test]
    fn namespaces_are_flattened_with_a_stable_alias_and_restored_on_continuation() {
        let prepared = prepare_responses_to_gemini_with_reasoning(
            &json!({
                "input": "lookup",
                "tools": [{
                    "type": "namespace",
                    "name": "weather",
                    "tools": [{
                        "type": "function",
                        "name": "lookup",
                        "parameters": {"type": "object"}
                    }]
                }],
                "tool_choice": {
                    "type": "function",
                    "namespace": "weather",
                    "name": "lookup"
                }
            }),
            "gemini-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
            "route",
            "namespace-1",
        )
        .unwrap();
        let alias = prepared.upstream_body["tools"][0]["functionDeclarations"][0]["name"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(alias.starts_with("relay_ns_"));
        assert_eq!(
            prepared.upstream_body["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"]
                [0],
            alias
        );

        let response = translate_gemini_response(
            prepared,
            &json!({
                "candidates":[{"content":{"parts":[{"functionCall":{
                    "name": alias.clone(),
                    "args": {"city":"Paris"},
                    "id":"call-weather"
                }}]}}]
            }),
        )
        .unwrap();
        assert_eq!(response.response_body["output"][0]["name"], "lookup");
        assert_eq!(response.response_body["output"][0]["namespace"], "weather");
        let continued = prepare_responses_to_gemini_with_reasoning(
            &json!({
                "previous_response_id": response.response_id,
                "input": [{
                    "type":"function_call_output",
                    "call_id":"call-weather",
                    "output":"sunny"
                }]
            }),
            "gemini-test",
            false,
            MessagesReasoningMode::Disabled,
            Some(response.continuation),
            "route",
            "namespace-2",
        )
        .unwrap();
        assert_eq!(
            continued.upstream_body["contents"][2]["parts"][0]["functionResponse"]["name"],
            alias
        );
    }

    #[test]
    fn function_result_preserves_text_and_media_parts() {
        let prepared = prepare_responses_to_gemini_with_reasoning(
            &json!({
                "input": "inspect",
                "tools": [{"type":"function","name":"run"}]
            }),
            "gemini-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
            "route",
            "req-1",
        )
        .unwrap();
        let response = translate_gemini_response(
            prepared,
            &json!({
                "candidates":[{"content":{"parts":[{"functionCall":{"name":"run","args":{},"id":"call-1"}}]}}]
            }),
        )
        .unwrap();
        let continued = prepare_responses_to_gemini_with_reasoning(
            &json!({
                "previous_response_id": response.response_id,
                "input": [{"type":"function_call_output","call_id":"call-1","output":[
                    {"type":"input_text","text":"done"},
                    {"type":"input_image","image_url":"data:image/png;base64,YQ=="}
                ]}]
            }),
            "gemini-test",
            false,
            MessagesReasoningMode::Disabled,
            Some(response.continuation),
            "route",
            "req-2",
        )
        .unwrap();
        assert_eq!(
            continued.upstream_body["contents"][2]["parts"][0]["functionResponse"]["response"]
                ["output"],
            "done"
        );
        assert_eq!(
            continued.upstream_body["contents"][2]["parts"][0]["functionResponse"]["response"]
                ["parts"][0]["inlineData"]["mimeType"],
            "image/png"
        );
    }

    #[test]
    fn converts_thinking_and_usage() {
        let prepared = prepare_responses_to_gemini_with_reasoning(
            &json!({"input":"think","reasoning":{"effort":"high"}}),
            "gemini-test",
            false,
            MessagesReasoningMode::Budget,
            None,
            "route",
            "req-1",
        )
        .unwrap();
        assert_eq!(
            prepared.upstream_body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            16384
        );
        let response = translate_gemini_response(prepared,&json!({"candidates":[{"content":{"parts":[{"thought":true,"text":"private"},{"text":"done"}]}}],"usageMetadata":{"thoughtsTokenCount":4}})).unwrap();
        assert_eq!(response.response_body["output"][0]["type"], "reasoning");
        assert_eq!(
            response.response_body["usage"]["output_tokens_details"]["reasoning_tokens"],
            4
        );
    }

    #[test]
    fn bridge_sanitizes_json_schema_for_gemini_without_touching_nested_shape() {
        let prepared = prepare_responses_to_gemini_with_reasoning(
            &json!({
                "input": "structured",
                "tools": [{
                    "type": "function",
                    "name": "lookup",
                    "parameters": {
                        "type": "object",
                        "$schema": "https://json-schema.org/draft/2020-12/schema",
                        "additionalProperties": false,
                        "properties": {
                            "query": {
                                "type": "string",
                                "pattern": ".+",
                                "$ref": "#/defs/query"
                            }
                        },
                        "required": ["query"]
                    }
                }],
                "text": {
                    "format": {
                        "type": "json_schema",
                        "schema": {
                            "type": "object",
                            "title": "Answer",
                            "description": "Structured answer",
                            "$defs": {"unused": {"type": "string"}},
                            "additionalProperties": false,
                            "properties": {
                                "answer": {
                                    "type": "string",
                                    "minLength": 1,
                                    "$ref": "#/defs/answer"
                                },
                                "scores": {
                                    "type": "array",
                                    "items": {
                                        "type": "number",
                                        "minimum": 0,
                                        "maximum": 1
                                    }
                                }
                            },
                            "required": ["answer"]
                        }
                    }
                }
            }),
            "gemini-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
            "route",
            "schema-1",
        )
        .unwrap();

        let schema = &prepared.upstream_body["generationConfig"]["responseSchema"];
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["title"], "Answer");
        assert_eq!(schema["description"], "Structured answer");
        assert_eq!(schema["properties"]["answer"]["type"], "string");
        assert!(schema.get("$defs").is_none());
        assert!(schema.get("$schema").is_none());
        assert!(schema.get("additionalProperties").is_none());
        assert!(schema["properties"]["answer"].get("$ref").is_none());
        assert!(schema["properties"]["answer"].get("minLength").is_none());
        assert_eq!(schema["properties"]["scores"]["items"]["type"], "number");
        assert!(schema["properties"]["scores"]["items"]
            .get("minimum")
            .is_none());

        let parameters =
            &prepared.upstream_body["tools"][0]["functionDeclarations"][0]["parameters"];
        assert_eq!(parameters["properties"]["query"]["type"], "string");
        assert!(parameters["properties"]["query"].get("pattern").is_none());
        assert!(parameters["properties"]["query"].get("$ref").is_none());
        assert!(parameters.get("additionalProperties").is_none());
    }

    #[test]
    fn partial_function_arguments_are_translated_to_responses_arguments() {
        let request = prepare_responses_to_gemini_with_reasoning(
            &json!({
                "input": "weather",
                "tools": [{"type":"function","name":"weather","parameters":{"type":"object"}}]
            }),
            "gemini-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
            "route",
            "partial",
        )
        .unwrap();
        let response = translate_gemini_response(
            request,
            &json!({
                "candidates":[{"content":{"parts":[{"functionCall":{
                    "name":"weather",
                    "partialArgs":[
                        {"jsonPath":"$.location","stringValue":"Paris"},
                        {"jsonPath":"$.units","stringValue":"C"}
                    ]
                }}]}}]
            }),
        )
        .unwrap();
        assert_eq!(
            response.response_body["output"][0]["arguments"],
            r#"{"location":"Paris","units":"C"}"#
        );
    }
}
