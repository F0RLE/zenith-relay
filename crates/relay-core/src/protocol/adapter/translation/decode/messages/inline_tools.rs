//! Lower client-owned Messages tool changes without changing native requests.
//! The next generation receives only active tools; historical changes remain
//! ordered system instructions, not retroactive edits to earlier turns.
use super::super::super::{AdapterError, AdapterResult, checked, optional_bool, required_text};
use crate::WireApi;
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

fn changes(block: &Value) -> bool {
    matches!(
        block["type"].as_str(),
        Some("tool_addition" | "tool_removal")
    )
}

pub(super) fn prepare(request: &Value) -> AdapterResult<Cow<'_, Value>> {
    let Some(messages) = request.get("messages").and_then(Value::as_array) else {
        return super::tool_search::prepare(request);
    };
    if !messages
        .iter()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .any(changes)
    {
        return super::tool_search::prepare(request);
    }
    let mut catalog = BTreeMap::new();
    let mut active = BTreeSet::new();
    let mut withdrawn = BTreeSet::new();
    if let Some(tools) = request.get("tools").filter(|tools| !tools.is_null()) {
        for tool in tools.as_array().ok_or_else(AdapterError::invalid_request)? {
            let (name, definition, deferred) = definition(tool)?;
            if catalog.insert(name.clone(), definition).is_some() {
                return Err(invalid("tools.name"));
            }
            if !deferred {
                active.insert(name);
            }
        }
    }
    let mut normalized = request.clone();
    let normalized_messages = normalized["messages"].as_array_mut().unwrap();
    for (index, message) in normalized_messages.iter_mut().enumerate() {
        let system = message["role"] == "system";
        let assistant = message["role"] == "assistant";
        let user = message["role"] == "user";
        let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        for block in blocks {
            if changes(block) {
                if !system || !valid_position(messages, index) {
                    return Err(invalid("messages[].content[].tool_addition"));
                }
                checked(block, &["type", "tool", "cache_control"])?;
                let addition = block["type"] == "tool_addition";
                let tool = block
                    .get("tool")
                    .ok_or_else(|| invalid("tool_addition.tool"))?;
                let name = match required_text(tool, "type")? {
                    "tool_reference" => {
                        checked(tool, &["type", "name"])?;
                        let name = required_text(tool, "name")?.to_owned();
                        if !catalog.contains_key(&name) {
                            return Err(invalid("tool_reference.name"));
                        }
                        if addition {
                            active.insert(name.clone());
                        }
                        name
                    }
                    "tool_definition" if addition => {
                        checked(tool, &["type", "definition"])?;
                        let (name, value, deferred) = definition(
                            tool.get("definition")
                                .ok_or_else(|| invalid("tool_definition"))?,
                        )?;
                        catalog.insert(name.clone(), value);
                        if deferred {
                            active.remove(&name);
                        } else {
                            active.insert(name.clone());
                        }
                        name
                    }
                    _ => {
                        return Err(AdapterError::parameter_unsupported_for(
                            "tool_addition.tool.type",
                        ));
                    }
                };
                if addition {
                    withdrawn.remove(&name);
                } else {
                    active.remove(&name);
                    withdrawn.insert(name);
                }
                // Other APIs have no native inline-tool block. Carry its exact
                // declaration at the original system-message position, while
                // the callable catalog enforces the final availability/schema.
                *block = json!({"type":"text","text":format!("Tool availability update: {block}")});
            } else if assistant && block["type"] == "tool_use" {
                if let Some(name) = block.get("name").and_then(Value::as_str)
                    && catalog.contains_key(name)
                    && !withdrawn.contains(name)
                {
                    active.insert(name.to_owned());
                }
            } else if user
                && block["type"] == "tool_result"
                && optional_bool(block, "is_error")? != Some(true)
            {
                for reference in block
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if reference["type"] == "tool_reference" {
                        let name = required_text(reference, "tool_name")?;
                        if !catalog.contains_key(name) {
                            return Err(invalid("tool_reference"));
                        }
                        active.insert(name.to_owned());
                        withdrawn.remove(name);
                    }
                }
            }
        }
    }
    if request.pointer("/tool_choice/type").and_then(Value::as_str) == Some("tool") {
        let name = required_text(&request["tool_choice"], "name")?;
        if !catalog.contains_key(name) || withdrawn.contains(name) {
            return Err(invalid("tool_choice.name"));
        }
        active.insert(name.to_owned());
    }
    // Resolve existing client discovery references against the complete known
    // catalog before excluding tools withdrawn later in the conversation.
    normalized["tools"] = catalog.values().cloned().collect::<Vec<_>>().into();
    let mut normalized = super::tool_search::prepare(&normalized)?.into_owned();
    normalized["tools"] = active
        .into_iter()
        .filter_map(|name| catalog.remove(&name))
        .collect::<Vec<_>>()
        .into();
    Ok(Cow::Owned(normalized))
}

fn definition(tool: &Value) -> AdapterResult<(String, Value, bool)> {
    let deferred = optional_bool(tool, "defer_loading")?.unwrap_or(false);
    let mut normalized = tool.clone();
    normalized
        .as_object_mut()
        .ok_or_else(AdapterError::invalid_request)?
        .remove("defer_loading");
    let functions = super::super::content::tools(Some(&json!([normalized])), WireApi::Messages)?;
    let name = functions
        .first()
        .ok_or_else(AdapterError::invalid_request)?
        .name
        .clone();
    Ok((name, normalized, deferred))
}

fn valid_position(messages: &[Value], index: usize) -> bool {
    let previous = index.checked_sub(1).and_then(|index| messages.get(index));
    let follows_turn = previous.is_some_and(|message| {
        message["role"] == "user"
            || message["role"] == "assistant"
                && message
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|blocks| {
                        blocks.iter().any(|block| {
                            matches!(
                                block["type"].as_str(),
                                Some(
                                    "web_search_tool_result"
                                        | "web_fetch_tool_result"
                                        | "code_execution_tool_result"
                                        | "bash_code_execution_tool_result"
                                        | "text_editor_code_execution_tool_result"
                                )
                            )
                        })
                    })
    });
    follows_turn
        && messages
            .get(index + 1)
            .is_none_or(|message| message["role"] == "assistant")
}

fn invalid(parameter: &'static str) -> AdapterError {
    AdapterError::invalid_request().with_parameter(parameter)
}
