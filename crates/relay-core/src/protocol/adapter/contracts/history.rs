//! Portable Responses history and restoration of provider-owned assistant turns.

use super::{AdapterContinuationState, AdapterError, AdapterResult, request_tool_catalog};
use crate::WireApi;
use serde_json::Value;
use std::borrow::Cow;

pub(super) fn prepare_request<'a>(
    request: &'a Value,
    previous: Option<&AdapterContinuationState>,
    model: &str,
) -> AdapterResult<Cow<'a, Value>> {
    if request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.trim().is_empty())
    {
        return Ok(Cow::Borrowed(request));
    }
    if let Some(previous) = previous {
        let key = super::super::translation::continuation_key(WireApi::Responses, request, model)?
            .ok_or_else(AdapterError::continuation_mismatch)?;
        if previous.history_key.as_deref() != Some(&key) {
            return Err(AdapterError::continuation_mismatch());
        }
        let input = request
            .get("input")
            .and_then(Value::as_array)
            .ok_or_else(AdapterError::invalid_request)?;
        let end = input
            .iter()
            .rposition(is_assistant_item)
            .ok_or_else(AdapterError::continuation_mismatch)?;
        // Loaded tools can live in historical control items. Collect the
        // current catalog before removing its already-materialized prefix.
        let tools = request_tool_catalog(
            request
                .as_object()
                .ok_or_else(AdapterError::invalid_request)?,
        )?;
        let mut resumed = request.clone();
        resumed["input"] = Value::Array(input[end + 1..].to_vec());
        resumed["previous_response_id"] = key.into();
        if let Some(tools) = tools {
            resumed["tools"] = tools.into();
        }
        return Ok(Cow::Owned(resumed));
    }

    // A new ordinary user message closes the preceding assistant/tool turn.
    // Its visible history is portable; another provider cannot consume its
    // ciphertext. Keep public summaries and all call/result links. Ciphertext
    // inside the current tool loop is still rejected by bridge validation.
    let Some(input) = request.get("input").and_then(Value::as_array) else {
        return Ok(Cow::Borrowed(request));
    };
    let Some(boundary) = input.iter().rposition(|item| {
        item.get("role").and_then(Value::as_str) == Some("user")
            && matches!(
                item.get("type").and_then(Value::as_str),
                None | Some("message")
            )
    }) else {
        return Ok(Cow::Borrowed(request));
    };
    if !input[..boundary].iter().any(has_ciphertext) {
        return Ok(Cow::Borrowed(request));
    }
    let mut portable = request.clone();
    let history = portable["input"].as_array_mut().expect("input array");
    for item in &mut history[..boundary] {
        if has_ciphertext(item) {
            item.as_object_mut()
                .expect("reasoning item")
                .remove("encrypted_content");
        }
    }
    super::super::translation::validate_responses_history(&Value::Array(
        history[..boundary].to_vec(),
    ))?;
    Ok(Cow::Owned(portable))
}

pub(in crate::protocol::adapter) fn is_assistant_item(item: &Value) -> bool {
    item.get("role").and_then(Value::as_str) == Some("assistant")
        || (item.get("type").and_then(Value::as_str) == Some("tool_search_call")
            && item.get("execution").and_then(Value::as_str) == Some("client"))
        || matches!(
            item.get("type").and_then(Value::as_str),
            Some("reasoning" | "function_call" | "custom_tool_call" | "web_search_call")
        )
}

fn has_ciphertext(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("reasoning")
        && item
            .get("encrypted_content")
            .is_some_and(|value| !value.is_null())
}

impl AdapterContinuationState {
    pub(crate) fn history_key(&self) -> Option<&str> {
        self.history_key.as_deref()
    }
}
