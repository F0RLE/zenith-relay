use super::super::super::{AdapterError, AdapterResult, Block};
use crate::WireApi;
use serde_json::Value;

/// Validate the source contract before lowering inline tool changes. Consecutive
/// system messages form one placement group, regardless of their block types.
/// Initial client attachments retain their pre-assistant startup boundary.
pub(super) fn validate_positions(messages: &[Value]) -> AdapterResult<()> {
    let mut index = 0;
    let mut assistant_started = false;
    while let Some(message) = messages.get(index) {
        if message["role"] != "system" {
            assistant_started |= message["role"] == "assistant";
            index += 1;
            continue;
        }
        let start = index;
        let mut text_only = true;
        while messages
            .get(index)
            .is_some_and(|message| message["role"] == "system")
        {
            text_only &= messages[index].get("content").is_some_and(is_text_content);
            index += 1;
        }
        let follows_turn = start
            .checked_sub(1)
            .and_then(|previous| messages.get(previous))
            .is_some_and(|previous| previous["role"] == "user" || ends_in_server_result(previous));
        if !follows_turn
            || (assistant_started || !text_only)
                && messages
                    .get(index)
                    .is_some_and(|message| message["role"] != "assistant")
        {
            return Err(AdapterError::invalid_request().with_parameter("messages[].role"));
        }
    }
    Ok(())
}

fn ends_in_server_result(message: &Value) -> bool {
    message["role"] == "assistant"
        && message
            .get("content")
            .and_then(Value::as_array)
            .and_then(|blocks| blocks.last())
            .is_some_and(|block| {
                matches!(
                    block["type"].as_str(),
                    Some(
                        "web_search_tool_result"
                            | "web_fetch_tool_result"
                            | "code_execution_tool_result"
                            | "bash_code_execution_tool_result"
                            | "text_editor_code_execution_tool_result"
                            | "tool_search_tool_result"
                    )
                )
            })
}

fn is_text_content(content: &Value) -> bool {
    content.is_string()
        || content
            .as_array()
            .is_some_and(|blocks| blocks.iter().all(|block| block["type"] == "text"))
}

pub(super) fn text_blocks(
    content: &Value,
    parameter: &'static str,
    allow_empty: bool,
) -> AdapterResult<Vec<Block>> {
    let invalid = || AdapterError::invalid_request().with_parameter(parameter);
    if !is_text_content(content) {
        return Err(invalid());
    }
    let blocks =
        super::super::content::text_blocks(content, WireApi::Messages).map_err(|error| {
            if error.parameter().is_some() {
                error
            } else {
                error.with_parameter(parameter)
            }
        })?;
    if !allow_empty && blocks.is_empty() {
        return Err(invalid());
    }
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn system_groups_require_a_server_result_at_the_end_of_an_assistant_turn() {
        let result = json!({"type":"web_search_tool_result"});
        let text = json!({"type":"text","text":"Answer"});
        for (content, valid) in [
            (json!([text, result]), true),
            (json!([result, text]), false),
            (json!([text]), false),
            (json!([]), false),
        ] {
            let messages = json!([
                {"role":"user","content":"Task"},
                {"role":"assistant","content":content},
                {"role":"system","content":"Instruction"},
                {"role":"system","content":"More context"}
            ]);
            assert_eq!(
                validate_positions(messages.as_array().unwrap()).is_ok(),
                valid
            );
        }
    }
}
