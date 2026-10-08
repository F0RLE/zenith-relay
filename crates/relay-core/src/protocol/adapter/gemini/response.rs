//! Gemini responses translated back into Responses bodies.

use super::{
    custom_tool_item_id, AdapterError, AdapterResult, GeminiBridgeRequest, GeminiBridgeResponse,
    ResponsesToolKind,
};
use serde_json::{json, Map, Value};

pub fn translate_gemini_response(
    request: GeminiBridgeRequest,
    upstream: &Value,
) -> AdapterResult<GeminiBridgeResponse> {
    if prompt_blocked(upstream).map_err(|()| AdapterError::upstream_response_invalid())? {
        let response_id = request.response_id.clone();
        let mut response_body = responses_body_from_output(
            &response_id,
            &request.model,
            Vec::new(),
            upstream.get("usageMetadata"),
        );
        response_body["status"] = Value::String("incomplete".into());
        response_body["incomplete_details"] = json!({"reason":"content_filter"});
        return Ok(GeminiBridgeResponse {
            response_body,
            response_id,
            continuation: request.bridge_state,
        });
    }
    let candidate = first_candidate(upstream)?;
    let incomplete_reason =
        candidate_incomplete_reason(candidate.get("finishReason").and_then(Value::as_str))?;
    let parts = match candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
    {
        Some(parts) => parts.as_slice(),
        None if incomplete_reason.is_some() => &[],
        None => return Err(AdapterError::upstream_response_invalid()),
    };
    let (response_items, _) = responses_output_from_gemini_parts(&request, parts)?;
    if response_items.is_empty() && incomplete_reason.is_none() {
        return Err(AdapterError::upstream_response_invalid());
    }
    let response_id = request.response_id.clone();
    let mut response_body = responses_body_from_output(
        &response_id,
        &request.model,
        response_items,
        upstream.get("usageMetadata"),
    );
    if let Some(reason) = incomplete_reason {
        response_body["status"] = Value::String("incomplete".to_string());
        response_body["incomplete_details"] = json!({"reason": reason});
    }
    let mut continuation = request.bridge_state.clone();
    super::request::append_message(&mut continuation, "model", parts.to_vec());
    Ok(GeminiBridgeResponse {
        response_body,
        response_id,
        continuation,
    })
}

pub(in crate::protocol::adapter) fn candidate_incomplete_reason(
    reason: Option<&str>,
) -> AdapterResult<Option<&'static str>> {
    match reason {
        None | Some("STOP") => Ok(None),
        Some("MAX_TOKENS") => Ok(Some("max_output_tokens")),
        Some(
            "SAFETY"
            | "RECITATION"
            | "LANGUAGE"
            | "BLOCKLIST"
            | "PROHIBITED_CONTENT"
            | "SPII"
            | "IMAGE_SAFETY"
            | "IMAGE_PROHIBITED_CONTENT"
            | "IMAGE_RECITATION"
            | "ESCALATION",
        ) => Ok(Some("content_filter")),
        _ => Err(AdapterError::upstream_response_invalid()),
    }
}

/// Gemini can block a prompt before producing candidates. Only an explicit
/// block reason is a filtered terminal; missing candidates alone are not.
pub(in crate::protocol::adapter) fn prompt_blocked(upstream_response: &Value) -> Result<bool, ()> {
    let Some(reason) = upstream_response.pointer("/promptFeedback/blockReason") else {
        return Ok(false);
    };
    if !matches!(
        reason.as_str(),
        Some("SAFETY" | "OTHER" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "IMAGE_SAFETY")
    ) || upstream_response
        .get("candidates")
        .is_some_and(|candidates| {
            candidates
                .as_array()
                .is_none_or(|candidate_items| !candidate_items.is_empty())
        })
    {
        return Err(());
    }
    Ok(true)
}

/// Recognize only Gemini terminal reasons the response adapter can translate.
/// The gateway uses this before releasing a stream with no generated output.
pub(crate) fn gemini_incomplete(upstream_response: &Value) -> bool {
    let Ok(blocked) = prompt_blocked(upstream_response) else {
        return false;
    };
    blocked
        || upstream_response
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
            .and_then(|candidate| candidate.get("finishReason"))
            .and_then(Value::as_str)
            .is_some_and(|reason| {
                candidate_incomplete_reason(Some(reason)).is_ok_and(|reason| reason.is_some())
            })
}

fn responses_body_from_output(
    response_id: &str,
    model: &str,
    response_items: Vec<Value>,
    usage_metadata: Option<&Value>,
) -> Value {
    json!({"id": response_id, "object": "response", "created_at": 0, "status": "completed",
        "model": model, "output": response_items, "usage": responses_usage(usage_metadata)})
}

fn responses_usage(usage_metadata: Option<&Value>) -> Value {
    let mut usage_object = Map::new();
    if let Some(input_tokens) = usage_metadata
        .and_then(|u| u.get("promptTokenCount"))
        .and_then(Value::as_u64)
    {
        usage_object.insert("input_tokens".to_string(), Value::from(input_tokens));
    }
    if let Some(output_tokens) = usage_metadata
        .and_then(|u| u.get("candidatesTokenCount"))
        .and_then(Value::as_u64)
    {
        usage_object.insert("output_tokens".to_string(), Value::from(output_tokens));
    }
    if let Some(total_tokens) = usage_metadata
        .and_then(|u| u.get("totalTokenCount"))
        .and_then(Value::as_u64)
    {
        usage_object.insert("total_tokens".to_string(), Value::from(total_tokens));
    }
    if let Some(cached_tokens) = usage_metadata
        .and_then(|u| u.get("cachedContentTokenCount"))
        .and_then(Value::as_u64)
    {
        usage_object.insert(
            "input_tokens_details".to_string(),
            json!({"cached_tokens": cached_tokens}),
        );
    }
    if let Some(reasoning_tokens) = usage_metadata
        .and_then(|u| u.get("thoughtsTokenCount"))
        .and_then(Value::as_u64)
    {
        usage_object.insert(
            "output_tokens_details".to_string(),
            json!({"reasoning_tokens": reasoning_tokens}),
        );
    }
    Value::Object(usage_object)
}

fn first_candidate(upstream: &Value) -> AdapterResult<&Value> {
    upstream
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .ok_or_else(AdapterError::upstream_response_invalid)
}

fn responses_output_from_gemini_parts(
    request: &GeminiBridgeRequest,
    parts: &[Value],
) -> AdapterResult<(Vec<Value>, Vec<Value>)> {
    let mut response_items = Vec::new();
    let mut text_buffer = String::new();
    let mut reasoning_buffer = String::new();
    let mut call_index = 0_usize;
    let flush_text = |response_items: &mut Vec<Value>, text_buffer: &mut String| {
        if !text_buffer.is_empty() {
            response_items.push(json!({"id":format!("msg_{}_{}",request.response_id(),response_items.len()),"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":std::mem::take(text_buffer),"annotations":[]}]}));
        }
    };
    let flush_reasoning = |response_items: &mut Vec<Value>, reasoning_buffer: &mut String| {
        if !reasoning_buffer.is_empty() {
            response_items.push(json!({"id":format!("reasoning_{}_{}",request.response_id(),response_items.len()),"type":"reasoning","status":"completed","summary":[{"type":"summary_text","text":std::mem::take(reasoning_buffer)}]}));
        }
    };
    for part_value in parts {
        let part = part_value
            .as_object()
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        if let Some(text_value) = part.get("text").and_then(Value::as_str) {
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                flush_text(&mut response_items, &mut text_buffer);
                reasoning_buffer.push_str(text_value);
            } else {
                flush_reasoning(&mut response_items, &mut reasoning_buffer);
                text_buffer.push_str(text_value);
            }
            continue;
        }
        let Some(call) = part.get("functionCall").and_then(Value::as_object) else {
            if part.get("thoughtSignature").is_some() {
                continue;
            }
            return Err(AdapterError::upstream_response_invalid());
        };
        flush_text(&mut response_items, &mut text_buffer);
        flush_reasoning(&mut response_items, &mut reasoning_buffer);
        let upstream_name = call
            .get("name")
            .and_then(Value::as_str)
            .filter(|tool_name| request.bridge_state.allows_tool_name(tool_name))
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        let target = request
            .bridge_state
            .client_tool(upstream_name)
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        let call_id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|call_id| !call_id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("call_{}_{}", request.response_id(), call_index));
        let function_arguments = super::json_path::function_call_args(call)
            .map_err(|_| AdapterError::upstream_response_invalid())?;
        let mut response_item = if target.kind == ResponsesToolKind::Custom {
            let custom_input = function_arguments
                .get("input")
                .and_then(Value::as_str)
                .ok_or_else(AdapterError::upstream_response_invalid)?;
            json!({"id":custom_tool_item_id(&call_id),"type":"custom_tool_call","status":"completed","call_id":call_id,"name":target.name,"input":custom_input})
        } else {
            json!({"id":call_id,"type":"function_call","status":"completed","call_id":call_id,"name":target.name,"arguments":serde_json::to_string(&function_arguments).map_err(|_| AdapterError::upstream_response_invalid())?})
        };
        if let Some(namespace) = target.namespace.as_ref() {
            response_item["namespace"] = Value::String(namespace.clone());
        }
        response_items.push(response_item);
        call_index = call_index.saturating_add(1);
    }
    flush_text(&mut response_items, &mut text_buffer);
    flush_reasoning(&mut response_items, &mut reasoning_buffer);
    Ok((response_items, Vec::new()))
}
