use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Prepared JSON sizes, not token estimates. No content, names, hashes, or
/// identifiers are retained. Provider-injected context is not observable here.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestContextSize {
    pub body_bytes: u64,
    pub instruction_bytes: u64,
    pub conversation_bytes: u64,
    pub tool_catalog_bytes: u64,
}

impl RequestContextSize {
    pub(crate) fn capture(request: &Value) -> Self {
        let bytes = crate::tool_policy::serialized_bytes;
        let mut size = Self {
            body_bytes: bytes(request),
            tool_catalog_bytes: crate::tool_policy::catalog_stats(request).bytes,
            ..Self::default()
        };
        for field in [
            "instructions",
            "system",
            "systemInstruction",
            "system_instruction",
        ] {
            if let Some(value) = request.get(field).filter(|value| !value.is_null()) {
                size.instruction_bytes = size.instruction_bytes.saturating_add(bytes(value));
            }
        }
        for field in ["input", "messages", "contents"] {
            let Some(value) = request.get(field).filter(|value| !value.is_null()) else {
                continue;
            };
            if let Some(items) = value.as_array() {
                for item in items {
                    if matches!(
                        item.get("role").and_then(Value::as_str),
                        Some("system" | "developer")
                    ) {
                        size.instruction_bytes = size.instruction_bytes.saturating_add(bytes(item));
                    } else if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
                        size.conversation_bytes =
                            size.conversation_bytes.saturating_add(bytes(item));
                    }
                }
            } else {
                size.conversation_bytes = size.conversation_bytes.saturating_add(bytes(value));
            }
        }
        size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn all_protocols_separate_instructions_history_and_tools_without_mutation() {
        let requests = [
            json!({"instructions":"synthetic policy","input":"hello","tools":[{"type":"function","name":"fixture","parameters":{"type":"object"}}]}),
            json!({"messages":[{"role":"system","content":"synthetic policy"},{"role":"user","content":"hello"}],"tools":[{"type":"function","function":{"name":"fixture","parameters":{"type":"object"}}}]}),
            json!({"system":[{"type":"text","text":"synthetic policy"}],"messages":[{"role":"user","content":"hello"}],"tools":[{"name":"fixture","input_schema":{"type":"object"}}]}),
            json!({"systemInstruction":{"parts":[{"text":"synthetic policy"}]},"contents":[{"role":"user","parts":[{"text":"hello"}]}],"tools":[{"functionDeclarations":[{"name":"fixture","parameters":{"type":"OBJECT"}}]}]}),
        ];
        for request in requests {
            let original = request.clone();
            let size = RequestContextSize::capture(&request);
            assert_eq!(
                size.body_bytes,
                serde_json::to_vec(&request).unwrap().len() as u64
            );
            assert!(size.instruction_bytes > 0);
            assert!(size.conversation_bytes > 0);
            assert!(size.tool_catalog_bytes > 0);
            assert_eq!(request, original);
            let serialized = serde_json::to_string(&size).unwrap();
            assert!(!serialized.contains("synthetic"));
            assert!(!serialized.contains("fixture"));
            assert!(!serialized.contains("hello"));
        }
    }

    #[test]
    fn additional_tools_are_not_counted_again_as_conversation() {
        let request = json!({"input":[{"type":"additional_tools","tools":[{"type":"function","name":"fixture","parameters":{"type":"object"}}]}]});
        let size = RequestContextSize::capture(&request);
        assert!(size.tool_catalog_bytes > 0);
        assert_eq!(size.conversation_bytes, 0);
    }

    #[test]
    fn ordinary_text_has_no_invented_instructions_or_tools() {
        let size = RequestContextSize::capture(&json!({"input":"hello"}));
        assert_eq!(size.instruction_bytes, 0);
        assert_eq!(size.tool_catalog_bytes, 0);
        assert_eq!(size.conversation_bytes, 7);
    }
}
