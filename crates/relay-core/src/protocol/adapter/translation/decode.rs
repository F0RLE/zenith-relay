mod chat;
mod content;
mod gemini;
mod messages;
mod responses;

use super::*;
use std::collections::BTreeMap;

pub(super) fn request(protocol: WireApi, value: &Value) -> AdapterResult<Request> {
    let request = match protocol {
        WireApi::Responses => responses::decode(value)?,
        WireApi::ChatCompletions => chat::decode(value)?,
        WireApi::Messages => messages::decode(value)?,
        WireApi::Gemini => gemini::decode(value)?,
    };
    if request
        .messages
        .iter()
        .all(|message| message.role == Role::System)
    {
        return Err(AdapterError::invalid_request());
    }
    if let Some(ToolChoice::Function(name)) = &request.tool_choice {
        if !request.tools.iter().any(|tool| tool.name == *name) {
            return Err(AdapterError::unsupported_tool());
        }
    }
    Ok(request)
}

pub(super) fn resolve_tool_history(messages: &mut [Message]) -> AdapterResult<()> {
    // Gemini function results are linked by name; assign their original call
    // identifiers when the history supplies them, without dropping any result.
    let mut pending = Vec::<(String, String)>::new();
    let mut ids = messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| {
            if let Block::ToolCall { id, .. } = block {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect::<std::collections::HashSet<_>>();
    let mut generated = 0;
    for message in messages.iter_mut() {
        for block in &mut message.blocks {
            match block {
                Block::ToolCall { id, name, .. } => {
                    if id.is_empty() {
                        loop {
                            *id = format!("call_relay_{generated}");
                            generated += 1;
                            if ids.insert(id.clone()) {
                                break;
                            }
                        }
                    }
                    pending.push((id.clone(), name.clone()));
                }
                Block::ToolResult { id, name, .. } => {
                    let index = pending
                        .iter()
                        .position(|(call_id, call_name)| {
                            if id.is_empty() {
                                call_name == name
                            } else {
                                call_id == id
                            }
                        })
                        .ok_or_else(AdapterError::invalid_request)?;
                    let (call_id, call_name) = pending.remove(index);
                    *id = call_id;
                    *name = call_name;
                }
                _ => {}
            }
        }
    }
    validate_tool_history(messages)
}

fn validate_tool_history(messages: &[Message]) -> AdapterResult<()> {
    let mut pending = BTreeMap::new();
    for message in messages {
        for block in &message.blocks {
            match block {
                Block::ToolCall { id, name, .. } => {
                    if pending.insert(id, name).is_some() {
                        return Err(AdapterError::invalid_request());
                    }
                }
                Block::ToolResult { id, name, .. }
                    if pending
                        .remove(id)
                        .is_none_or(|call_name| !name.is_empty() && call_name != name) =>
                {
                    return Err(AdapterError::invalid_request());
                }
                _ => {}
            }
        }
    }
    Ok(())
}
