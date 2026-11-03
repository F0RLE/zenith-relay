//! Convenience entry points and native Messages cache/token controls.
use super::{
    AdapterContinuationState, AdapterError, AdapterResult, MessagesReasoningMode,
    TranslationRequest,
};
use crate::{AdapterRequestContext, CacheWriteTtl, PreparedAdapterRequest, SourceAdapter, WireApi};
use serde_json::{Map, Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 8_192;
static DIRECT_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn prepare_responses_to_messages(
    request: &Value,
    model: &str,
    stream: bool,
    reasoning_mode: MessagesReasoningMode,
    previous: Option<AdapterContinuationState>,
) -> AdapterResult<TranslationRequest> {
    prepare_responses_to_messages_scoped(request, model, stream, reasoning_mode, previous, "")
}

/// This convenience API delegates to the same dispatcher as all twelve routes.
pub fn prepare_responses_to_messages_scoped(
    request: &Value,
    model: &str,
    stream: bool,
    reasoning_mode: MessagesReasoningMode,
    previous: Option<AdapterContinuationState>,
    response_scope: &str,
) -> AdapterResult<TranslationRequest> {
    let seed = format!(
        "direct-request-{}",
        DIRECT_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let prepared = SourceAdapter::ResponsesToMessages.prepare_request(AdapterRequestContext {
        client_wire_api: WireApi::Responses,
        request,
        model,
        stream,
        reasoning_mode,
        cache_write_ttl: CacheWriteTtl::Provider,
        previous,
        response_scope,
        response_id_seed: &seed,
    })?;
    let PreparedAdapterRequest::Translated { request } = prepared else {
        unreachable!()
    };
    Ok(*request)
}

pub(in crate::protocol::adapter) fn message_max_output_tokens(
    requested: Option<u64>,
    capabilities: Option<&crate::model_metadata::ModelCapabilities>,
) -> AdapterResult<u64> {
    let limit = capabilities
        .and_then(|value| value.output_limit)
        .filter(|value| *value > 0);
    if requested
        .zip(limit)
        .is_some_and(|(requested, limit)| requested > limit)
    {
        return Err(AdapterError::parameter_unsupported_for("max_output_tokens"));
    }
    Ok(requested.unwrap_or_else(|| {
        limit.map_or(DEFAULT_MAX_OUTPUT_TOKENS, |limit| {
            limit.min(DEFAULT_MAX_OUTPUT_TOKENS)
        })
    }))
}

pub(crate) fn apply_cache_write_ttl(
    request: &mut Value,
    cache_write_ttl: CacheWriteTtl,
) -> AdapterResult<()> {
    let Some(ttl) = cache_write_ttl.anthropic_ttl() else {
        return Ok(());
    };
    let fields = request
        .as_object_mut()
        .ok_or_else(AdapterError::invalid_request)?;
    let mut updated = false;
    for key in ["system", "tools"] {
        for block in fields
            .get_mut(key)
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if block.get("cache_control").is_some() {
                updated |= set_cache_control(block, ttl);
            }
        }
    }
    for message in fields
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        for block in message
            .get_mut("content")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if block.get("cache_control").is_some() {
                updated |= set_cache_control(block, ttl);
            }
        }
    }
    if updated {
        return Ok(());
    }
    let prefix_marked = fields
        .get_mut("system")
        .is_some_and(|value| set_last_cache_control(value, ttl));
    if !prefix_marked && let Some(tools) = fields.get_mut("tools") {
        set_last_cache_control(tools, ttl);
    }
    if let Some(content) = fields
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .and_then(|messages| {
            messages
                .iter_mut()
                .rev()
                .find_map(|message| message.get_mut("content"))
        })
    {
        set_last_cache_control(content, ttl);
    }
    Ok(())
}

fn set_last_cache_control(content: &mut Value, ttl: &str) -> bool {
    if let Some(blocks) = content.as_array_mut() {
        return blocks
            .iter_mut()
            .rev()
            .any(|block| set_cache_control(block, ttl));
    }
    let Some(text) = content
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
    else {
        return false;
    };
    *content = json!([{"type":"text","text":text,"cache_control":{"type":"ephemeral","ttl":ttl}}]);
    true
}

fn set_cache_control(block: &mut Value, ttl: &str) -> bool {
    let Some(block) = block.as_object_mut() else {
        return false;
    };
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
        "cache_control".into(),
        json!({"type":"ephemeral","ttl":ttl}),
    );
    true
}

pub(in crate::protocol::adapter) fn custom_tool_input_schema(
    tool: &Map<String, Value>,
) -> AdapterResult<Value> {
    let mut input = json!({"type":"string"});
    if let Some(format) = tool.get("format").filter(|value| !value.is_null()) {
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
                    .filter(|text| !text.is_empty())
                    .ok_or_else(AdapterError::unsupported_tool)?;
                input["description"] =
                    format!("Raw tool input. It must satisfy this {syntax} grammar:\n{definition}")
                        .into();
            }
            _ => return Err(AdapterError::unsupported_tool()),
        }
    }
    Ok(
        json!({"type":"object","properties":{"input":input},"required":["input"],"additionalProperties":false}),
    )
}
