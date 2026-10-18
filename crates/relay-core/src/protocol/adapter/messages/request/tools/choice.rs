use super::*;
use crate::protocol::adapter::contracts::{resolve_responses_tool_choice, ToolChoice};

pub(in crate::protocol::adapter::messages::request) fn translate_tool_choice(
    choice: &Value,
    bridge_state: &AdapterContinuationState,
) -> AdapterResult<TranslatedToolChoice> {
    let available = bridge_state.available_tool_targets();
    let (choice, allowed_names) = resolve_responses_tool_choice(choice, &available)?;
    let translated_choice = choice.and_then(|choice| {
        if available.is_empty() {
            return None;
        }
        Some(match choice {
            ToolChoice::Auto => json!({"type":"auto"}),
            ToolChoice::None => json!({"type":"none"}),
            ToolChoice::Required => json!({"type":"any"}),
            ToolChoice::Function(name) => json!({"type":"tool","name":name}),
        })
    });
    Ok(TranslatedToolChoice {
        translated_choice,
        allowed_names,
    })
}
