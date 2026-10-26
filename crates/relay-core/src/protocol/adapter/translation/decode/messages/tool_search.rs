//! Lower client-owned Messages discovery, not a provider-hosted search engine.
//! The client chooses what to load. Native Messages routes never enter here.
use super::super::super::{AdapterError, AdapterResult, optional_bool, required_text};
use crate::WireApi;
use serde_json::{Value, json};
use std::{borrow::Cow, collections::BTreeSet};

pub(super) fn prepare(request: &Value) -> AdapterResult<Cow<'_, Value>> {
    let has_deferral = request
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| tools.iter().any(|tool| tool.get("defer_loading").is_some()));
    let has_references = request
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|block| block["type"] == "tool_result")
        .filter_map(|block| block.get("content").and_then(Value::as_array))
        .flatten()
        .any(|block| block["type"] == "tool_reference");
    if !has_deferral && !has_references {
        return Ok(Cow::Borrowed(request));
    }

    let mut normalized = request.clone();
    let mut deferred = BTreeSet::new();
    if let Some(tools) = normalized.get_mut("tools").and_then(Value::as_array_mut) {
        for tool in tools {
            if optional_bool(tool, "defer_loading")? == Some(true) {
                if tool
                    .get("cache_control")
                    .is_some_and(|cache| !cache.is_null())
                {
                    return Err(
                        AdapterError::invalid_request().with_parameter("tools.cache_control")
                    );
                }
                deferred.insert(required_text(tool, "name")?.to_owned());
            }
            if let Some(fields) = tool.as_object_mut() {
                fields.remove("defer_loading");
            }
        }
    }
    // Validate the entire catalog before selecting: unsupported hosted tools,
    // duplicate names and malformed deferred definitions must still fail.
    let functions = super::super::content::tools(normalized.get("tools"), WireApi::Messages)?;
    let names: BTreeSet<_> = functions.iter().map(|tool| tool.name.clone()).collect();
    if !deferred.is_empty() && deferred.len() == names.len() {
        return Err(AdapterError::invalid_request().with_parameter("tools.defer_loading"));
    }
    let mut loaded = BTreeSet::new();
    if let Some(name) = request.get("tool_choice").and_then(|choice| {
        (choice["type"] == "tool")
            .then(|| choice.get("name").and_then(Value::as_str))
            .flatten()
    }) {
        loaded.insert(name.to_owned());
    }
    for message in normalized
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let user_result = message["role"] == "user";
        let assistant_call = message["role"] == "assistant";
        for block in message
            .get_mut("content")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            // A retained call also establishes that the client already loaded
            // the tool, including after its earlier discovery was compacted.
            if assistant_call
                && block["type"] == "tool_use"
                && let Some(name) = block.get("name").and_then(Value::as_str)
                && names.contains(name)
            {
                loaded.insert(name.to_owned());
            }
            if block["type"] != "tool_result" {
                continue;
            }
            let successful = optional_bool(block, "is_error")? != Some(true);
            for reference in block
                .get_mut("content")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if reference["type"] != "tool_reference" {
                    continue;
                }
                if !user_result || !successful {
                    return Err(AdapterError::invalid_request().with_parameter("tool_reference"));
                }
                super::super::super::options::messages_fields(reference, &["type", "tool_name"])?;
                let name = required_text(reference, "tool_name")?;
                if !names.contains(name) {
                    return Err(
                        AdapterError::unsupported_tool().with_parameter("tool_reference.tool_name")
                    );
                }
                loaded.insert(name.to_owned());
                // Foreign APIs get an ordinary result with the same discovery
                // identity, and the exact schema in their callable catalog.
                // Never duplicate the full schema into conversation text.
                *reference = json!({"type":"text", "text":reference.to_string()});
            }
        }
    }
    if let Some(tools) = normalized.get_mut("tools").and_then(Value::as_array_mut) {
        tools.retain(|tool| {
            let name = tool["name"].as_str().unwrap_or_default();
            !deferred.contains(name) || loaded.contains(name)
        });
    }
    Ok(Cow::Owned(normalized))
}
