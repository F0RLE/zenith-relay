use super::contracts::{
    custom_tool_item_id, request_tool_catalog, AdapterError, AdapterResult, ClientToolTarget,
    MessagesBridgeRequest, MessagesBridgeResponse, MessagesBridgeState, MessagesReasoningMode,
    ResponsesToolKind, TranslatedTools,
};
use crate::CacheWriteTtl;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const SUPPORTED_IMAGE_TYPES: &[&str] = &["image/gif", "image/jpeg", "image/png", "image/webp"];
/// Converts a Codex Responses request to the Anthropic Messages contract.
///
/// JSON-schema functions retain their object input. Direct custom tools are
/// represented as a function with one raw-text field and are translated back
/// to the exact Responses custom-call shape before the client sees them.
/// Provider-hosted and dynamic-discovery tools are omitted from the upstream
/// Messages catalog. They remain in the client request; the bridge only sends
/// the subset it can represent without inventing a provider capability.
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
    let object = request
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
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
            append_system_value(&mut state, instructions)?;
        }
    } else if object.contains_key("instructions") {
        return Err(AdapterError::continuation_mismatch());
    }

    if let Some(tools) = request_tool_catalog(object)? {
        let TranslatedTools {
            upstream,
            client_tools,
        } = translate_tools(&tools)?;
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
        // A Responses choice may target a hosted or future tool which is not
        // present in the translated Messages catalog. Dropping that choice
        // lets the upstream model answer normally instead of turning a
        // representable request into a Relay-side 400.
        let translated = translate_tool_choice(tool_choice, &state)?;
        state.tool_choice = translated.value;
        state.tool_allow_list = translated.allowed_names;
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
    if let Some(temperature) = object.get("temperature") {
        body.insert("temperature".to_string(), temperature.clone());
    }
    if let Some(top_p) = object.get("top_p") {
        body.insert("top_p".to_string(), top_p.clone());
    }
    if let Some(stop_sequences) = object.get("stop") {
        body.insert("stop_sequences".to_string(), stop_sequences.clone());
    }
    apply_reasoning(&mut body, object.get("reasoning"), reasoning_mode)?;
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

/// Converts a complete Anthropic Messages response to a complete Responses
/// object and captures the exact native assistant blocks for the next turn.
pub fn translate_messages_response(
    request: MessagesBridgeRequest,
    upstream: &Value,
) -> AdapterResult<MessagesBridgeResponse> {
    let upstream_id = upstream
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(AdapterError::upstream_response_invalid)?;
    let content = upstream
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::upstream_response_invalid)?
        .clone();
    validate_messages_tool_calls(&request.state, &content)?;
    let (mut output, _) = responses_output_from_messages_content(&content, &request.state)?;
    let response_id = bridged_response_id_scoped(request.response_scope(), upstream_id);
    set_message_output_id(&mut output, &response_id);
    let usage = responses_usage(upstream.get("usage"));
    let response_body = json!({
        "id": response_id,
        "object": "response",
        "created_at": upstream
            .get("created_at")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        "status": "completed",
        "model": request.state.model,
        "output": output,
        "usage": usage,
    });
    let mut continuation = request.state;
    continuation.append_assistant_content(content);
    Ok(MessagesBridgeResponse {
        response_body,
        response_id,
        continuation,
    })
}

pub fn bridged_response_id(upstream_id: &str) -> String {
    bridged_response_id_scoped("", upstream_id)
}

/// Derives a deterministic client-facing id from both the upstream id and the
/// connector route that produced it. Length-prefixing the scope prevents
/// ambiguous concatenations such as `ab` + `c` versus `a` + `bc`.
pub fn bridged_response_id_scoped(scope: &str, upstream_id: &str) -> String {
    if scope.is_empty() {
        let digest = Sha256::digest(upstream_id.as_bytes());
        return format!("resp_bridge_{}", hex::encode(&digest[..12]));
    }
    let mut hasher = Sha256::new();
    hasher.update((scope.len() as u64).to_le_bytes());
    hasher.update(scope.as_bytes());
    hasher.update((upstream_id.len() as u64).to_le_bytes());
    hasher.update(upstream_id.as_bytes());
    let digest = hasher.finalize();
    format!("resp_bridge_{}", hex::encode(&digest[..12]))
}

pub(super) fn set_message_output_id(output: &mut [Value], response_id: &str) {
    let mut message_index = 0_usize;
    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        if let Some(object) = item.as_object_mut() {
            let id = if message_index == 0 {
                format!("msg_{response_id}")
            } else {
                format!("msg_{response_id}_{message_index}")
            };
            object.insert("id".to_string(), Value::String(id));
            message_index = message_index.saturating_add(1);
        }
    }
}

fn append_system_value(state: &mut MessagesBridgeState, value: &Value) -> AdapterResult<()> {
    let text = content_to_messages_blocks(value)?;
    if text.is_empty() {
        return Ok(());
    }
    let next = Value::Array(text);
    match state.system.take() {
        None => state.system = Some(next),
        Some(Value::Array(mut current)) => {
            current.extend(next.as_array().into_iter().flatten().cloned());
            state.system = Some(Value::Array(current));
        }
        Some(_) => return Err(AdapterError::invalid_request()),
    }
    Ok(())
}

fn append_responses_input(state: &mut MessagesBridgeState, input: &Value) -> AdapterResult<()> {
    match input {
        Value::String(text) => append_user_blocks(state, vec![text_block(text)]),
        Value::Array(items) => {
            let mut tool_results = Vec::new();
            for item in items {
                let item = item.as_object().ok_or_else(AdapterError::invalid_request)?;
                if matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("function_call_output" | "custom_tool_call_output")
                ) {
                    tool_results.push(tool_result_block(state, item)?);
                    continue;
                }
                flush_tool_results(state, &mut tool_results)?;
                if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                    // Tool definitions were collected before the Messages body was
                    // built. This Responses control item has no conversation
                    // equivalent and must not be emitted as a user message.
                    continue;
                }
                if let Some(role) = item.get("role").and_then(Value::as_str) {
                    match role {
                        "system" | "developer" => {
                            append_system_value(
                                state,
                                item.get("content")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?;
                        }
                        "user" => append_user_blocks(
                            state,
                            content_to_messages_blocks(
                                item.get("content")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?,
                        )?,
                        "assistant" => append_assistant_from_responses_item(state, item)?,
                        _ => return Err(AdapterError::invalid_request()),
                    }
                    continue;
                }
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call" | "custom_tool_call") => {
                        append_assistant_tool_use(state, item)?
                    }
                    Some("reasoning") => {
                        // A bridge continuation retains native thinking blocks locally. A
                        // standalone Responses reasoning item has no Anthropic signature and
                        // cannot be replayed safely.
                        if state.messages.is_empty() {
                            return Err(AdapterError::continuation_missing());
                        }
                    }
                    Some("message") => {
                        let role = item
                            .get("role")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?;
                        if role != "user" {
                            return Err(AdapterError::invalid_request());
                        }
                        append_user_blocks(
                            state,
                            content_to_messages_blocks(
                                item.get("content")
                                    .ok_or_else(AdapterError::invalid_request)?,
                            )?,
                        )?;
                    }
                    _ => return Err(AdapterError::invalid_request()),
                }
            }
            flush_tool_results(state, &mut tool_results)
        }
        _ => Err(AdapterError::invalid_request()),
    }
}

fn append_assistant_from_responses_item(
    state: &mut MessagesBridgeState,
    item: &Map<String, Value>,
) -> AdapterResult<()> {
    let content = item
        .get("content")
        .map(content_to_messages_blocks)
        .transpose()?;
    if let Some(content) = content.filter(|content| !content.is_empty()) {
        state
            .messages
            .push(json!({"role": "assistant", "content": content}));
    }
    Ok(())
}

fn append_assistant_tool_use(
    state: &mut MessagesBridgeState,
    item: &Map<String, Value>,
) -> AdapterResult<()> {
    let kind = ResponsesToolKind::from_call_item(item)?;
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    let namespace = match item.get("namespace") {
        None => None,
        Some(namespace) => Some(
            namespace
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(AdapterError::invalid_request)?,
        ),
    };
    let upstream_name = state
        .upstream_tool_name(namespace, name)
        .map(str::to_string)
        .ok_or_else(AdapterError::invalid_request)?;
    if state.client_tool_kind(&upstream_name) != Some(kind) {
        return Err(AdapterError::invalid_request());
    }
    let input = match kind {
        ResponsesToolKind::Function => {
            let arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            serde_json::from_str::<Value>(arguments)
                .ok()
                .filter(Value::is_object)
                .ok_or_else(AdapterError::invalid_request)?
        }
        ResponsesToolKind::Custom => {
            let input = item
                .get("input")
                .and_then(Value::as_str)
                .ok_or_else(AdapterError::invalid_request)?;
            json!({"input": input})
        }
    };
    state.messages.push(json!({
        "role": "assistant",
        "content": [{"type": "tool_use", "id": call_id, "name": upstream_name, "input": input}],
    }));
    Ok(())
}

fn append_user_blocks(state: &mut MessagesBridgeState, blocks: Vec<Value>) -> AdapterResult<()> {
    if blocks.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    state
        .messages
        .push(json!({"role": "user", "content": blocks}));
    Ok(())
}

fn flush_tool_results(
    state: &mut MessagesBridgeState,
    results: &mut Vec<Value>,
) -> AdapterResult<()> {
    if results.is_empty() {
        return Ok(());
    }
    let Some(last) = state.messages.last() else {
        return Err(AdapterError::continuation_missing());
    };
    if last.get("role").and_then(Value::as_str) != Some("assistant")
        || !last
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            })
    {
        return Err(AdapterError::continuation_mismatch());
    }
    let known_call_ids = last
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| block.get("id").and_then(Value::as_str))
        .map(|id| (id.to_string(), ()))
        .collect::<BTreeMap<_, _>>();
    let mut returned_call_ids = BTreeMap::new();
    for result in results.iter() {
        let call_id = result
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(AdapterError::continuation_mismatch)?;
        if !known_call_ids.contains_key(call_id)
            || returned_call_ids.insert(call_id.to_string(), ()).is_some()
        {
            return Err(AdapterError::continuation_mismatch());
        }
    }
    let content = std::mem::take(results);
    state
        .messages
        .push(json!({"role": "user", "content": content}));
    Ok(())
}

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

fn content_to_messages_blocks(content: &Value) -> AdapterResult<Vec<Value>> {
    match content {
        Value::String(text) if !text.is_empty() => Ok(vec![text_block(text)]),
        Value::String(_) => Ok(Vec::new()),
        Value::Array(parts) => parts
            .iter()
            .map(|part| {
                if let Some(text) = part.as_str() {
                    return Ok(text_block(text));
                }
                let part = part.as_object().ok_or_else(AdapterError::invalid_request)?;
                match part.get("type").and_then(Value::as_str) {
                    Some("input_text" | "output_text" | "text") => part
                        .get("text")
                        .and_then(Value::as_str)
                        .map(text_block)
                        .ok_or_else(AdapterError::invalid_request),
                    Some("input_image") => image_block_from_data_uri(
                        part.get("image_url")
                            .and_then(Value::as_str)
                            .ok_or_else(AdapterError::invalid_request)?,
                    ),
                    _ => Err(AdapterError::invalid_request()),
                }
            })
            .collect(),
        _ => Err(AdapterError::invalid_request()),
    }
}

fn tool_result_block(
    state: &MessagesBridgeState,
    item: &Map<String, Value>,
) -> AdapterResult<Value> {
    let kind = ResponsesToolKind::from_output_item(item)?;
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    let expected = state
        .messages
        .last()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks.iter().find(|block| {
                block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && block.get("id").and_then(Value::as_str) == Some(call_id)
            })
        })
        .and_then(|block| block.get("name"))
        .and_then(Value::as_str)
        .and_then(|name| state.client_tool(name))
        .ok_or_else(AdapterError::continuation_mismatch)?;
    if kind != expected.kind {
        return Err(AdapterError::continuation_mismatch());
    }
    if let Some(namespace) = item.get("namespace") {
        let namespace = namespace
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(AdapterError::invalid_request)?;
        if expected.namespace.as_deref() != Some(namespace) {
            return Err(AdapterError::continuation_mismatch());
        }
    }
    let content = match kind {
        ResponsesToolKind::Custom => match item.get("output") {
            Some(Value::String(output)) => Value::String(output.clone()),
            _ => return Err(AdapterError::invalid_request()),
        },
        ResponsesToolKind::Function => function_tool_result_content(item.get("output"))?,
    };
    Ok(json!({
        "type": "tool_result",
        "tool_use_id": call_id,
        "content": content,
    }))
}

fn function_tool_result_content(output: Option<&Value>) -> AdapterResult<Value> {
    match output {
        None => Ok(Value::String(String::new())),
        Some(Value::String(output)) => {
            let Some(parts) = serde_json::from_str::<Value>(output)
                .ok()
                .and_then(|value| value.as_array().cloned())
            else {
                return Ok(Value::String(output.clone()));
            };
            match function_tool_output_parts(&parts)? {
                Some(content) => Ok(content),
                None => Ok(Value::String(output.clone())),
            }
        }
        Some(Value::Array(parts)) => match function_tool_output_parts(parts)? {
            Some(content) => Ok(content),
            None => Ok(Value::String(
                serde_json::to_string(parts).map_err(|_| AdapterError::invalid_request())?,
            )),
        },
        Some(value) => Ok(Value::String(
            serde_json::to_string(value).map_err(|_| AdapterError::invalid_request())?,
        )),
    }
}

/// Converts a Responses tool output array only when it contains an image.
/// Ordinary JSON arrays keep their previous string representation.
fn function_tool_output_parts(parts: &[Value]) -> AdapterResult<Option<Value>> {
    let contains_image = parts.iter().any(|part| {
        part.get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "input_image")
    });
    if !contains_image {
        return Ok(None);
    }

    let mut blocks = Vec::with_capacity(parts.len());
    for part in parts {
        let Some(part) = part.as_object() else {
            return Err(AdapterError::invalid_request());
        };
        match part.get("type").and_then(Value::as_str) {
            Some("input_text" | "output_text" | "text") => {
                let text = part
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::invalid_request)?;
                if !text.is_empty() {
                    blocks.push(text_block(text));
                }
            }
            Some("input_image") => blocks.push(image_block_from_data_uri(
                part.get("image_url")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::invalid_request)?,
            )?),
            _ => return Err(AdapterError::invalid_request()),
        }
    }
    if blocks.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    Ok(Some(Value::Array(blocks)))
}

fn image_block_from_data_uri(data_uri: &str) -> AdapterResult<Value> {
    let data_uri = data_uri.trim();
    let Some(rest) = data_uri.strip_prefix("data:") else {
        return Err(AdapterError::invalid_request());
    };
    let Some((metadata, data)) = rest.split_once(',') else {
        return Err(AdapterError::invalid_request());
    };
    let mut metadata_parts = metadata.split(';');
    let media_type = metadata_parts
        .next()
        .map(str::trim)
        .filter(|value| {
            SUPPORTED_IMAGE_TYPES
                .iter()
                .any(|supported| value.eq_ignore_ascii_case(supported))
        })
        .ok_or_else(AdapterError::invalid_request)?
        .to_ascii_lowercase();
    if !metadata_parts.any(|part| part.trim().eq_ignore_ascii_case("base64")) {
        return Err(AdapterError::invalid_request());
    }
    let data = data.trim();
    if data.is_empty()
        || data.bytes().any(|byte| byte.is_ascii_whitespace())
        || data.len() > (MAX_IMAGE_BYTES * 4 / 3).saturating_add(4)
    {
        return Err(AdapterError::invalid_request());
    }
    let decoded = STANDARD
        .decode(data)
        .map_err(|_| AdapterError::invalid_request())?;
    if decoded.is_empty() || decoded.len() > MAX_IMAGE_BYTES {
        return Err(AdapterError::invalid_request());
    }

    Ok(json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": media_type,
            "data": data,
        }
    }))
}

/// An upstream Messages response may only invoke a function that Relay sent in
/// the translated client catalog. This protects the client tool router from an
/// upstream-invented tool name while preserving the exact name supplied by the
/// client when the call is valid.
pub(super) fn validate_messages_tool_calls(
    state: &MessagesBridgeState,
    content: &[Value],
) -> AdapterResult<()> {
    let mut call_ids = BTreeSet::new();
    for block in content {
        let block = block
            .as_object()
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let id = block
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        if !block.get("input").is_some_and(Value::is_object)
            || !state.allows_tool_name(name)
            || !call_ids.insert(id.to_string())
        {
            return Err(AdapterError::upstream_response_invalid());
        }
    }
    Ok(())
}

fn translate_tools(tools: &[Value]) -> AdapterResult<TranslatedTools> {
    let mut upstream = Vec::with_capacity(tools.len());
    let mut client_tools = BTreeMap::new();
    for tool in tools {
        let Some(tool) = tool.as_object() else {
            continue;
        };
        match tool.get("type").and_then(Value::as_str) {
            Some("function" | "custom") => {
                if let Err(error) =
                    translate_client_tool(&mut upstream, &mut client_tools, tool, None, None)
                {
                    if !error.is_route_incompatible() {
                        return Err(error);
                    }
                }
            }
            Some("namespace") => {
                let Some(namespace) = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                let namespace_description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
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
                        if let Err(error) = translate_client_tool(
                            &mut upstream,
                            &mut client_tools,
                            child,
                            Some(namespace),
                            namespace_description,
                        ) {
                            if !error.is_route_incompatible() {
                                return Err(error);
                            }
                        }
                    }
                    // The Responses namespace can carry tools that require
                    // server execution or a separate adapter. Do not make a
                    // Messages source advertise them under a fake contract.
                }
            }
            // Hosted and dynamic Responses tools (for example web search and
            // tool search) cannot be executed by an Anthropic Messages source.
            // Omitting them preserves every representable client tool instead
            // of rejecting the complete Codex request.
            _ => {}
        }
    }
    Ok(TranslatedTools {
        upstream,
        client_tools,
    })
}

fn translate_client_tool(
    upstream: &mut Vec<Value>,
    client_tools: &mut BTreeMap<String, ClientToolTarget>,
    tool: &Map<String, Value>,
    namespace: Option<&str>,
    namespace_description: Option<&str>,
) -> AdapterResult<()> {
    let kind = ResponsesToolKind::from_definition(tool)?;
    let name = tool
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(AdapterError::unsupported_tool)?;
    let upstream_name = namespace
        .map(|namespace| bridged_namespace_tool_name(namespace, name))
        .unwrap_or_else(|| name.to_string());
    if client_tools.contains_key(&upstream_name) {
        return Err(AdapterError::unsupported_tool());
    }

    let mut translated =
        Map::from_iter([("name".to_string(), Value::String(upstream_name.clone()))]);
    match kind {
        ResponsesToolKind::Function => {
            let mut schema = tool
                .get("parameters")
                .or_else(|| tool.get("input_schema"))
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            let schema = schema
                .as_object_mut()
                .ok_or_else(AdapterError::unsupported_tool)?;
            match schema.get("type").and_then(Value::as_str) {
                Some("object") => {}
                None => {
                    schema.insert("type".to_string(), Value::String("object".to_string()));
                }
                _ => return Err(AdapterError::unsupported_tool()),
            }
            translated.insert("input_schema".to_string(), Value::Object(schema.clone()));
        }
        ResponsesToolKind::Custom => {
            if tool
                .get("defer_loading")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || tool
                    .get("allowed_callers")
                    .is_some_and(|callers| !callers.is_null())
            {
                return Err(AdapterError::unsupported_tool());
            }
            translated.insert("input_schema".to_string(), custom_tool_input_schema(tool)?);
        }
    }
    let tool_description = tool
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(namespace) = namespace {
        let mut description = format!("Codex namespace `{namespace}` tool `{name}`.");
        if let Some(namespace_description) = namespace_description {
            description.push_str(&format!(" {namespace_description}"));
        }
        if let Some(tool_description) = tool_description {
            description.push_str(&format!(" {tool_description}"));
        }
        translated.insert("description".to_string(), Value::String(description));
    } else if let Some(description) = tool_description {
        translated.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
    }
    client_tools.insert(
        upstream_name,
        ClientToolTarget {
            kind,
            name: name.to_string(),
            namespace: namespace.map(str::to_string),
        },
    );
    upstream.push(Value::Object(translated));
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

fn custom_tool_input_schema(tool: &Map<String, Value>) -> AdapterResult<Value> {
    let mut input = Map::from_iter([("type".to_string(), Value::String("string".to_string()))]);
    if let Some(format) = tool.get("format") {
        let format = format
            .as_object()
            .ok_or_else(AdapterError::unsupported_tool)?;
        match format.get("type").and_then(Value::as_str) {
            Some("text") => {}
            Some("grammar") => {
                let syntax = format
                    .get("syntax")
                    .and_then(Value::as_str)
                    .filter(|syntax| matches!(*syntax, "lark" | "regex"))
                    .ok_or_else(AdapterError::unsupported_tool)?;
                let definition = format
                    .get("definition")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|definition| !definition.is_empty())
                    .ok_or_else(AdapterError::unsupported_tool)?;
                input.insert(
                    "description".to_string(),
                    Value::String(format!(
                        "Raw tool input. It must satisfy this {syntax} grammar:\n{definition}"
                    )),
                );
            }
            _ => return Err(AdapterError::unsupported_tool()),
        }
    }
    Ok(json!({
        "type": "object",
        "properties": {"input": Value::Object(input)},
        "required": ["input"],
        "additionalProperties": false,
    }))
}

#[derive(Debug)]
struct TranslatedToolChoice {
    value: Option<Value>,
    allowed_names: Option<BTreeSet<String>>,
}

fn translate_tool_choice(
    tool_choice: &Value,
    state: &MessagesBridgeState,
) -> AdapterResult<TranslatedToolChoice> {
    let has_tools = state.upstream_tools().is_some();
    match tool_choice {
        Value::String(value) => match value.as_str() {
            "auto" => Ok(TranslatedToolChoice {
                value: has_tools.then(|| json!({"type": "auto"})),
                allowed_names: None,
            }),
            "none" => Ok(TranslatedToolChoice {
                value: has_tools.then(|| json!({"type": "none"})),
                allowed_names: None,
            }),
            "required" if has_tools => Ok(TranslatedToolChoice {
                value: Some(json!({"type": "any"})),
                allowed_names: None,
            }),
            "required" => Ok(TranslatedToolChoice {
                value: None,
                allowed_names: None,
            }),
            _ => Ok(TranslatedToolChoice {
                value: None,
                allowed_names: None,
            }),
        },
        Value::Object(value)
            if matches!(
                value.get("type").and_then(Value::as_str),
                Some("function" | "custom")
            ) =>
        {
            let Some(name) = selected_upstream_tool_name(state, value) else {
                return Ok(TranslatedToolChoice {
                    value: None,
                    allowed_names: None,
                });
            };
            Ok(TranslatedToolChoice {
                value: Some(json!({"type": "tool", "name": name})),
                allowed_names: None,
            })
        }
        Value::Object(value) if value.get("type").and_then(Value::as_str) == Some("namespace") => {
            let namespace = value
                .get("name")
                .or_else(|| value.get("namespace"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|namespace| !namespace.is_empty())
                .ok_or_else(AdapterError::unsupported_tool)?;
            let allowed_names = state
                .tool_targets
                .iter()
                .filter_map(|(upstream_name, target)| {
                    (target.namespace.as_deref() == Some(namespace)
                        && state.allows_tool_name(upstream_name))
                    .then_some(upstream_name.clone())
                })
                .collect::<BTreeSet<_>>();
            if allowed_names.is_empty() {
                return Ok(TranslatedToolChoice {
                    value: None,
                    allowed_names: None,
                });
            }
            Ok(TranslatedToolChoice {
                value: Some(json!({"type": "any"})),
                allowed_names: Some(allowed_names),
            })
        }
        Value::Object(value)
            if value.get("type").and_then(Value::as_str) == Some("allowed_tools") =>
        {
            let Some(configured_tools) = value.get("tools").and_then(Value::as_array) else {
                return Ok(TranslatedToolChoice {
                    value: None,
                    allowed_names: None,
                });
            };
            let mut allowed_names = BTreeSet::new();
            for tool in configured_tools {
                let Some(tool) = tool.as_object() else {
                    continue;
                };
                match tool.get("type").and_then(Value::as_str) {
                    Some("function" | "custom") => {
                        if let Some(name) = selected_upstream_tool_name(state, tool) {
                            allowed_names.insert(name);
                        }
                    }
                    Some("namespace") => {
                        let namespace = tool
                            .get("name")
                            .or_else(|| tool.get("namespace"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|namespace| !namespace.is_empty())
                            .unwrap_or_default();
                        if namespace.is_empty() {
                            continue;
                        }
                        allowed_names.extend(state.tool_targets.iter().filter_map(
                            |(upstream_name, target)| {
                                (target.namespace.as_deref() == Some(namespace)
                                    && state.allows_tool_name(upstream_name))
                                .then_some(upstream_name.clone())
                            },
                        ));
                    }
                    // A hosted-only tool cannot be selected through a
                    // Messages bridge. Keep the representable client tools in
                    // an allowed-tools set; fail below when none remain.
                    _ => {}
                }
            }
            if allowed_names.is_empty() {
                return Ok(TranslatedToolChoice {
                    value: None,
                    allowed_names: None,
                });
            }
            let value = match value.get("mode").and_then(Value::as_str).unwrap_or("auto") {
                "auto" => json!({"type": "auto"}),
                "required" => json!({"type": "any"}),
                _ => {
                    return Ok(TranslatedToolChoice {
                        value: None,
                        allowed_names: None,
                    })
                }
            };
            Ok(TranslatedToolChoice {
                value: Some(value),
                allowed_names: Some(allowed_names),
            })
        }
        _ => Ok(TranslatedToolChoice {
            value: None,
            allowed_names: None,
        }),
    }
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
    let namespace = match tool.get("namespace") {
        None => None,
        Some(namespace) => Some(
            namespace
                .as_str()
                .map(str::trim)
                .filter(|namespace| !namespace.is_empty())?,
        ),
    };
    let upstream_name = state.upstream_tool_name(namespace, name)?;
    (state.client_tool_kind(upstream_name) == Some(kind)).then(|| upstream_name.to_string())
}

fn apply_reasoning(
    body: &mut Map<String, Value>,
    reasoning: Option<&Value>,
    mode: MessagesReasoningMode,
) -> AdapterResult<()> {
    let effort = reasoning
        .and_then(Value::as_object)
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(Value::as_str)
        .map(|effort| effort.trim().to_ascii_lowercase())
        .filter(|effort| !effort.is_empty() && effort != "none");
    let Some(effort) = effort else {
        return Ok(());
    };
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
            body.remove("temperature");
            body.remove("top_p");
            Ok(())
        }
        MessagesReasoningMode::Adaptive => {
            let effort = match effort.as_str() {
                "minimal" => "low",
                "ultra" => "max",
                "low" | "medium" | "high" | "xhigh" | "max" => effort.as_str(),
                _ => return Err(AdapterError::reasoning_unsupported()),
            };
            body.insert("thinking".to_string(), json!({"type": "adaptive"}));
            body.insert("output_config".to_string(), json!({"effort": effort}));
            body.remove("temperature");
            body.remove("top_p");
            Ok(())
        }
    }
}

pub(super) fn responses_output_from_messages_content(
    content: &[Value],
    state: &MessagesBridgeState,
) -> AdapterResult<(Vec<Value>, Vec<Value>)> {
    let mut output = Vec::new();
    let mut preserved = Vec::new();
    let mut text = Vec::new();
    let mut text_message_index = 0_usize;
    let flush_text =
        |output: &mut Vec<Value>, text: &mut Vec<Value>, text_message_index: &mut usize| {
            if text.is_empty() {
                return;
            }
            let index = *text_message_index;
            *text_message_index = (*text_message_index).saturating_add(1);
            output.push(json!({
                "id": if index == 0 {
                    "msg_bridge_output".to_string()
                } else {
                    format!("msg_bridge_output_{index}")
                },
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": std::mem::take(text),
            }));
        };
    for block in content {
        let block = block
            .as_object()
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let value = block
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                if !value.is_empty() {
                    text.push(json!({"type": "output_text", "text": value, "annotations": []}));
                }
                preserved.push(Value::Object(block.clone()));
            }
            Some("tool_use") => {
                flush_text(&mut output, &mut text, &mut text_message_index);
                let call_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                let target = state
                    .client_tool(name)
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                let kind = target.kind;
                let client_name = target.name.clone();
                let client_namespace = target.namespace.clone();
                let input = block
                    .get("input")
                    .filter(|value| value.is_object())
                    .ok_or_else(AdapterError::upstream_response_invalid)?;
                let mut item = match kind {
                    ResponsesToolKind::Function => json!({
                        "id": call_id,
                        "type": kind.response_item_type(),
                        "status": "completed",
                        "call_id": call_id,
                        "name": client_name.clone(),
                        "arguments": serde_json::to_string(input).map_err(|_| AdapterError::upstream_response_invalid())?,
                    }),
                    ResponsesToolKind::Custom => json!({
                        "id": custom_tool_item_id(call_id),
                        "type": kind.response_item_type(),
                        "status": "completed",
                        "call_id": call_id,
                        "name": client_name.clone(),
                        "input": custom_tool_input(input)?,
                    }),
                };
                if let Some(namespace) = client_namespace {
                    item.as_object_mut()
                        .expect("Responses output item is an object")
                        .insert("namespace".to_string(), Value::String(namespace));
                }
                output.push(item);
                preserved.push(Value::Object(block.clone()));
            }
            Some("thinking" | "redacted_thinking") => {
                // The native block (including its signature) must survive in bridge
                // state, but it is intentionally not exposed as fake Responses
                // encrypted content.
                preserved.push(Value::Object(block.clone()));
            }
            _ => return Err(AdapterError::upstream_response_invalid()),
        }
    }
    flush_text(&mut output, &mut text, &mut text_message_index);
    if output.is_empty() {
        return Err(AdapterError::upstream_response_invalid());
    }
    Ok((output, preserved))
}

pub(super) fn custom_tool_input(input: &Value) -> AdapterResult<&str> {
    let input = input
        .as_object()
        .ok_or_else(AdapterError::upstream_response_invalid)?;
    if input.len() != 1 {
        return Err(AdapterError::upstream_response_invalid());
    }
    input
        .get("input")
        .and_then(Value::as_str)
        .ok_or_else(AdapterError::upstream_response_invalid)
}

pub(super) fn responses_usage(usage: Option<&Value>) -> Value {
    let input_tokens = usage
        .and_then(|usage| usage.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output_tokens = usage
        .and_then(|usage| usage.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let mut result = Map::from_iter([
        ("input_tokens".to_string(), Value::from(input_tokens)),
        ("output_tokens".to_string(), Value::from(output_tokens)),
        (
            "total_tokens".to_string(),
            Value::from(input_tokens.saturating_add(output_tokens)),
        ),
    ]);
    if let Some(cache_read) = usage
        .and_then(|usage| usage.get("cache_read_input_tokens"))
        .and_then(Value::as_u64)
    {
        result.insert(
            "input_tokens_details".to_string(),
            json!({"cached_tokens": cache_read}),
        );
    }
    if let Some(cache_write) = usage
        .and_then(|usage| usage.get("cache_creation_input_tokens"))
        .and_then(Value::as_u64)
    {
        result
            .entry("input_tokens_details".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("usage details is an object")
            .insert("cache_write_tokens".to_string(), Value::from(cache_write));
    }
    if let Some(cache_write_ttl) = usage.and_then(cache_write_ttl_from_usage) {
        result
            .entry("input_tokens_details".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("usage details is an object")
            .insert(
                "cache_write_ttl".to_string(),
                Value::String(cache_write_ttl.to_string()),
            );
    }
    Value::Object(result)
}

fn cache_write_ttl_from_usage(usage: &Value) -> Option<&'static str> {
    let creation = usage.get("cache_creation")?;
    if creation
        .get("ephemeral_1h_input_tokens")
        .and_then(Value::as_u64)
        .is_some_and(|tokens| tokens > 0)
    {
        return Some("1h");
    }
    creation
        .get("ephemeral_5m_input_tokens")
        .and_then(Value::as_u64)
        .is_some_and(|tokens| tokens > 0)
        .then_some("5m")
}
