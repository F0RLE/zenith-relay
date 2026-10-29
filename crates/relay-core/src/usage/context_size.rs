use serde::{Deserialize, Serialize};
use serde_json::Value;

mod conversation;
pub use conversation::ConversationSizeBreakdown;

/// Prepared JSON sizes, not token estimates. No content, names, hashes, or
/// identifiers are retained. Provider-injected context is not observable here.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestContextSize {
    pub body_bytes: u64,
    pub instruction_bytes: u64,
    pub conversation_bytes: u64,
    pub tool_catalog_bytes: u64,
    /// Disjoint serialized-byte categories within conversation_bytes. Legacy
    /// observations remain unknown rather than becoming measured zeroes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<ConversationSizeBreakdown>,
}

impl RequestContextSize {
    pub(crate) fn capture(request: &Value) -> Self {
        Self::capture_with_catalog_bytes(request, crate::tool_policy::catalog_stats(request).bytes)
    }

    pub(crate) fn capture_with_catalog_bytes(request: &Value, tool_catalog_bytes: u64) -> Self {
        use crate::tool_policy::serialized_bytes;

        let mut size = Self {
            tool_catalog_bytes,
            conversation: Some(ConversationSizeBreakdown::default()),
            ..Self::default()
        };
        let Some(fields) = request.as_object() else {
            size.body_bytes = serialized_bytes(request);
            return size;
        };
        // Reuse each value's exact JSON size for the body and its category;
        // only braces, commas, keys and colons need separate accounting.
        size.body_bytes = 2 + fields.len().saturating_sub(1) as u64;
        for (field, value) in fields {
            let value_bytes = if matches!(field.as_str(), "input" | "messages" | "contents") {
                size.capture_conversation(value)
            } else {
                let value_bytes = serialized_bytes(value);
                if !value.is_null()
                    && matches!(
                        field.as_str(),
                        "instructions" | "system" | "systemInstruction" | "system_instruction"
                    )
                {
                    size.instruction_bytes = size.instruction_bytes.saturating_add(value_bytes);
                }
                value_bytes
            };
            size.body_bytes = size
                .body_bytes
                .saturating_add(serialized_bytes(field))
                .saturating_add(1)
                .saturating_add(value_bytes);
        }
        size
    }

    fn capture_conversation(&mut self, value: &Value) -> u64 {
        let bytes = crate::tool_policy::serialized_bytes;
        let Some(items) = value.as_array() else {
            let value_bytes = if value.is_null() {
                bytes(value)
            } else {
                self.conversation.as_mut().unwrap().capture(value)
            };
            if !value.is_null() {
                self.conversation_bytes = self.conversation_bytes.saturating_add(value_bytes);
            }
            return value_bytes;
        };
        let mut array_bytes = 2 + items.len().saturating_sub(1) as u64;
        for item in items {
            let instruction = matches!(
                item.get("role").and_then(Value::as_str),
                Some("system" | "developer")
            );
            let catalog = item.get("type").and_then(Value::as_str) == Some("additional_tools");
            let item_bytes = if instruction || catalog {
                bytes(item)
            } else {
                self.conversation.as_mut().unwrap().capture(item)
            };
            array_bytes = array_bytes.saturating_add(item_bytes);
            if instruction {
                self.instruction_bytes = self.instruction_bytes.saturating_add(item_bytes);
            } else if !catalog {
                self.conversation_bytes = self.conversation_bytes.saturating_add(item_bytes);
            }
        }
        array_bytes
    }
}

#[cfg(test)]
mod tests;
