use serde::{Deserialize, Serialize};
use serde_json::Value;
/// Privacy-safe evidence about tool handling for one request. This deliberately
/// excludes tool names, arguments, prompt text, and response text.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoiceMode {
    #[default]
    Unspecified,
    Auto,
    Required,
    None,
    AllowedTools,
    Specific,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutputKind {
    #[default]
    Unknown,
    Empty,
    Text,
    ToolCall,
    Mixed,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUseDiagnostics {
    pub client_tool_count: u16,
    pub forwarded_tool_count: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_schema_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded_schema_bytes: Option<u64>,
    #[serde(default)]
    pub filtered_tool_count: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_mode: Option<crate::ToolPolicyMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_outcome: Option<crate::ToolPolicyOutcome>,
    /// A single pre-output retry restored the original catalog after Relay's
    /// own stream parser rejected a filtered native Responses attempt.
    #[serde(default)]
    pub policy_fallback: bool,
    /// The native Responses request used provider-hosted deferred tool search.
    /// The complete trusted catalog remains available to the provider; only
    /// the tool schemas are loaded into model context on demand.
    #[serde(default)]
    pub deferred_tool_search: bool,
    #[serde(default)]
    pub tool_choice: ToolChoiceMode,
    pub tool_call_count: u16,
    pub text_output: bool,
    #[serde(default)]
    pub terminal_output: TerminalOutputKind,
}

impl ToolUseDiagnostics {
    pub fn observe_output_item(&mut self, output_item: &Value) {
        let item_observation = output_observation_from_item(output_item);
        self.tool_call_count = self
            .tool_call_count
            .saturating_add(item_observation.tool_call_count);
        self.text_output |= item_observation.text_output;
    }

    pub fn observe_stream_payload(&mut self, event_payload: &Value) {
        if let Some(response_payload) = event_payload.get("response") {
            self.set_terminal_response(response_payload);
            return;
        }
        if event_payload.get("type").and_then(Value::as_str) == Some("response.output_item.done") {
            if let Some(output_item) = event_payload.get("item") {
                self.observe_output_item(output_item);
            }
        }
        if let Some(content_block) = event_payload.get("content_block") {
            self.observe_output_item(content_block);
        }
        let choice_observation = output_observation_from_chat_choices(event_payload);
        self.tool_call_count = self.tool_call_count.max(choice_observation.tool_call_count);
        self.text_output |= choice_observation.text_output;
    }

    pub fn set_terminal_response(&mut self, response_payload: &Value) {
        let response_observation = output_observation(response_payload);
        let terminal_output_is_empty = response_observation.inspected
            && response_observation.tool_call_count == 0
            && !response_observation.text_output;
        if response_observation.inspected
            && !(terminal_output_is_empty && (self.tool_call_count > 0 || self.text_output))
        {
            self.tool_call_count = response_observation.tool_call_count;
            self.text_output = response_observation.text_output;
        }
        self.finish();
    }

    pub fn finish(&mut self) {
        self.terminal_output = match (self.tool_call_count > 0, self.text_output) {
            (false, false) => TerminalOutputKind::Empty,
            (false, true) => TerminalOutputKind::Text,
            (true, false) => TerminalOutputKind::ToolCall,
            (true, true) => TerminalOutputKind::Mixed,
        };
    }

    pub fn tools_were_available_but_not_called(&self) -> bool {
        self.forwarded_tool_count > 0
            && self.tool_call_count == 0
            && matches!(self.terminal_output, TerminalOutputKind::Text)
    }

    /// Usage storage is sparse: an ordinary text request without a tools
    /// configuration should not grow a meaningless diagnostics section.
    pub fn has_evidence(&self) -> bool {
        self.client_tool_count > 0
            || self.forwarded_tool_count > 0
            || self.filtered_tool_count > 0
            || self.policy_fallback
            || self.deferred_tool_search
            || self.tool_call_count > 0
            || !matches!(self.tool_choice, ToolChoiceMode::Unspecified)
    }
}

#[derive(Default)]
struct OutputObservation {
    inspected: bool,
    tool_call_count: u16,
    text_output: bool,
}

fn output_observation(response_payload: &Value) -> OutputObservation {
    let response_object = response_payload.get("response").unwrap_or(response_payload);
    if let Some(output_items) = response_object.get("output").and_then(Value::as_array) {
        let mut response_observation = OutputObservation {
            inspected: true,
            ..OutputObservation::default()
        };
        for output_item in output_items {
            merge_output_observation(
                &mut response_observation,
                output_observation_from_item(output_item),
            );
        }
        return response_observation;
    }
    if let Some(content_blocks) = response_object.get("content").and_then(Value::as_array) {
        let mut response_observation = OutputObservation {
            inspected: true,
            ..OutputObservation::default()
        };
        for content_block in content_blocks {
            merge_output_observation(
                &mut response_observation,
                output_observation_from_item(content_block),
            );
        }
        return response_observation;
    }
    output_observation_from_chat_choices(response_object)
}

fn output_observation_from_item(output_item: &Value) -> OutputObservation {
    let mut item_observation = OutputObservation::default();
    match output_item.get("type").and_then(Value::as_str) {
        Some("function_call" | "custom_tool_call" | "tool_use") => {
            item_observation.tool_call_count = 1;
        }
        Some("message") => {
            item_observation.text_output = message_has_text(output_item);
        }
        Some("output_text" | "text") => {
            item_observation.text_output = true;
        }
        _ => {}
    }
    item_observation
}

fn output_observation_from_chat_choices(response_payload: &Value) -> OutputObservation {
    let Some(choices) = response_payload.get("choices").and_then(Value::as_array) else {
        return OutputObservation::default();
    };
    let mut response_observation = OutputObservation {
        inspected: true,
        ..OutputObservation::default()
    };
    for choice in choices {
        let message_payload = choice
            .get("message")
            .or_else(|| choice.get("delta"))
            .unwrap_or(choice);
        response_observation.tool_call_count = response_observation.tool_call_count.saturating_add(
            message_payload
                .get("tool_calls")
                .and_then(Value::as_array)
                .map_or(0, |calls| calls.len().min(u16::MAX as usize) as u16),
        );
        if message_payload.get("function_call").is_some() {
            response_observation.tool_call_count =
                response_observation.tool_call_count.saturating_add(1);
        }
        response_observation.text_output |= message_has_text(message_payload);
    }
    response_observation
}

fn merge_output_observation(target: &mut OutputObservation, incoming: OutputObservation) {
    target.inspected |= incoming.inspected;
    target.tool_call_count = target
        .tool_call_count
        .saturating_add(incoming.tool_call_count);
    target.text_output |= incoming.text_output;
}

fn message_has_text(message_payload: &Value) -> bool {
    match message_payload.get("content") {
        Some(Value::String(content)) => !content.is_empty(),
        Some(Value::Array(content_blocks)) => content_blocks.iter().any(|content_block| {
            matches!(
                content_block.get("type").and_then(Value::as_str),
                Some("output_text" | "text")
            )
        }),
        _ => false,
    }
}
