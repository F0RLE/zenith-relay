use crate::tool_policy::serialized_bytes;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Categories sum to conversation_bytes, excluding outer history delimiters.
/// Nested typed attachments/blocks own their bytes; message/result wrappers
/// remain in the parent category. Arbitrary tool arguments are never inspected.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSizeBreakdown {
    pub message_bytes: u64,
    pub tool_call_bytes: u64,
    pub tool_result_bytes: u64,
    pub reasoning_bytes: u64,
    pub attachment_bytes: u64,
    pub other_bytes: u64,
    /// Top-level non-instruction/non-catalog history items, including nulls.
    pub item_count: u64,
    /// Recognized top-level or nested attachment blocks, not unique files.
    pub attachment_count: u64,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Kind {
    Message,
    ToolCall,
    ToolResult,
    Reasoning,
    Attachment,
    Other,
}

impl ConversationSizeBreakdown {
    pub(super) fn capture(&mut self, item: &Value) -> u64 {
        self.item_count += 1;
        self.measure(
            item,
            if item.is_string() {
                Kind::Message
            } else {
                Kind::Other
            },
            0,
        )
    }

    fn add(&mut self, kind: Kind, bytes: u64) {
        let counter = match kind {
            Kind::Message => &mut self.message_bytes,
            Kind::ToolCall => &mut self.tool_call_bytes,
            Kind::ToolResult => &mut self.tool_result_bytes,
            Kind::Reasoning => &mut self.reasoning_bytes,
            Kind::Attachment => &mut self.attachment_bytes,
            Kind::Other => &mut self.other_bytes,
        };
        *counter = counter.saturating_add(bytes);
    }

    fn measure(&mut self, item: &Value, parent: Kind, depth: usize) -> u64 {
        let mut kind = classify(item).unwrap_or(parent);
        if parent == Kind::ToolResult && kind == Kind::Message {
            kind = Kind::ToolResult;
        }
        if kind == Kind::Attachment {
            self.attachment_count += 1;
        }
        // Only split protocol content arrays, never arbitrary JSON within
        // arguments/results or provider ciphertext. Bound classification depth.
        let fields = item
            .as_object()
            .filter(|_| depth < 32 && matches!(kind, Kind::Message | Kind::ToolResult));
        let Some(fields) = fields else {
            let bytes = serialized_bytes(item);
            self.add(kind, bytes);
            return bytes;
        };
        let mut own = 2 + fields.len().saturating_sub(1) as u64;
        let mut nested = 0;
        for (key, value) in fields {
            own += serialized_bytes(key) + 1;
            if matches!(key.as_str(), "content" | "parts" | "output" | "tool_calls")
                && let Some(parts) = value.as_array()
            {
                own += 2 + parts.len().saturating_sub(1) as u64;
                for part in parts {
                    let parent = if key == "tool_calls" {
                        Kind::ToolCall
                    } else {
                        kind
                    };
                    nested += self.measure(part, parent, depth + 1);
                }
            } else if key == "function_call" && kind == Kind::Message && value.is_object() {
                nested += self.measure(value, Kind::ToolCall, depth + 1);
            } else {
                own += serialized_bytes(value);
            }
        }
        self.add(kind, own);
        own + nested
    }
}

fn classify(item: &Value) -> Option<Kind> {
    let fields = item.as_object()?;
    match fields.get("type").and_then(Value::as_str) {
        Some("function_call" | "custom_tool_call" | "tool_use") => Some(Kind::ToolCall),
        Some("function_call_output" | "custom_tool_call_output" | "tool_result") => {
            Some(Kind::ToolResult)
        }
        Some("reasoning" | "thinking" | "redacted_thinking") => Some(Kind::Reasoning),
        Some(
            "input_image" | "output_image" | "image" | "image_url" | "input_file" | "file"
            | "input_audio" | "audio" | "video" | "video_url",
        ) => Some(Kind::Attachment),
        Some("message" | "agent_message" | "input_text" | "output_text" | "text") => {
            Some(Kind::Message)
        }
        _ if fields.contains_key("functionCall") => Some(Kind::ToolCall),
        _ if fields.contains_key("functionResponse") => Some(Kind::ToolResult),
        _ if fields.contains_key("inlineData") || fields.contains_key("fileData") => {
            Some(Kind::Attachment)
        }
        _ if fields.get("thought").and_then(Value::as_bool) == Some(true) => Some(Kind::Reasoning),
        _ => match fields.get("role").and_then(Value::as_str) {
            Some("tool" | "function") => Some(Kind::ToolResult),
            Some("user" | "assistant" | "model") => Some(Kind::Message),
            _ => None,
        },
    }
}
