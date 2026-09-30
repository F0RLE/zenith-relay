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
    pub fn observe_output_item(&mut self, item: &Value) {
        let output = output_observation_from_item(item);
        self.tool_call_count = self.tool_call_count.saturating_add(output.tool_call_count);
        self.text_output |= output.text_output;
    }

    pub fn observe_stream_payload(&mut self, value: &Value) {
        if let Some(response) = value.get("response") {
            self.set_terminal_response(response);
            return;
        }
        if value.get("type").and_then(Value::as_str) == Some("response.output_item.done") {
            if let Some(item) = value.get("item") {
                self.observe_output_item(item);
            }
        }
        if let Some(content_block) = value.get("content_block") {
            self.observe_output_item(content_block);
        }
        let output = output_observation_from_chat_choices(value);
        self.tool_call_count = self.tool_call_count.max(output.tool_call_count);
        self.text_output |= output.text_output;
    }

    pub fn set_terminal_response(&mut self, value: &Value) {
        let output = output_observation(value);
        let terminal_output_is_empty =
            output.inspected && output.tool_call_count == 0 && !output.text_output;
        if output.inspected
            && !(terminal_output_is_empty && (self.tool_call_count > 0 || self.text_output))
        {
            self.tool_call_count = output.tool_call_count;
            self.text_output = output.text_output;
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

fn output_observation(value: &Value) -> OutputObservation {
    let response = value.get("response").unwrap_or(value);
    if let Some(items) = response.get("output").and_then(Value::as_array) {
        let mut output = OutputObservation {
            inspected: true,
            ..OutputObservation::default()
        };
        for item in items {
            merge_output_observation(&mut output, output_observation_from_item(item));
        }
        return output;
    }
    if let Some(content) = response.get("content").and_then(Value::as_array) {
        let mut output = OutputObservation {
            inspected: true,
            ..OutputObservation::default()
        };
        for item in content {
            merge_output_observation(&mut output, output_observation_from_item(item));
        }
        return output;
    }
    output_observation_from_chat_choices(response)
}

fn output_observation_from_item(item: &Value) -> OutputObservation {
    let mut output = OutputObservation::default();
    match item.get("type").and_then(Value::as_str) {
        Some("function_call" | "custom_tool_call" | "tool_use") => {
            output.tool_call_count = 1;
        }
        Some("message") => {
            output.text_output = message_has_text(item);
        }
        Some("output_text" | "text") => {
            output.text_output = true;
        }
        _ => {}
    }
    output
}

fn output_observation_from_chat_choices(value: &Value) -> OutputObservation {
    let Some(choices) = value.get("choices").and_then(Value::as_array) else {
        return OutputObservation::default();
    };
    let mut output = OutputObservation {
        inspected: true,
        ..OutputObservation::default()
    };
    for choice in choices {
        let message = choice
            .get("message")
            .or_else(|| choice.get("delta"))
            .unwrap_or(choice);
        output.tool_call_count = output.tool_call_count.saturating_add(
            message
                .get("tool_calls")
                .and_then(Value::as_array)
                .map_or(0, |calls| calls.len().min(u16::MAX as usize) as u16),
        );
        if message.get("function_call").is_some() {
            output.tool_call_count = output.tool_call_count.saturating_add(1);
        }
        output.text_output |= message_has_text(message);
    }
    output
}

fn merge_output_observation(target: &mut OutputObservation, next: OutputObservation) {
    target.inspected |= next.inspected;
    target.tool_call_count = target.tool_call_count.saturating_add(next.tool_call_count);
    target.text_output |= next.text_output;
}

fn message_has_text(value: &Value) -> bool {
    match value.get("content") {
        Some(Value::String(content)) => !content.is_empty(),
        Some(Value::Array(items)) => items.iter().any(|item| {
            matches!(
                item.get("type").and_then(Value::as_str),
                Some("output_text" | "text")
            )
        }),
        _ => false,
    }
}
