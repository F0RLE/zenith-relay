//! Observed Codex chat-handoff context, not an ordinary unpaired tool result.

use crate::protocol::AdapterError;
use serde_json::{Value, json};

pub(in crate::gateway::execution::basis_points) fn is_thread_handoff(item: &Value) -> bool {
    item["type"] == "function_call_output"
        && item["namespace"] == "codex_app"
        && item["name"] == "create_thread"
        && item.get("call_id").is_none()
        && item["output"]
            .as_str()
            .and_then(|output| {
                output
                    .trim()
                    .strip_prefix("<codex_delegation>")
                    .and_then(|text| text.strip_suffix("</codex_delegation>"))
            })
            .is_some_and(|text| !text.trim().is_empty())
}

pub(super) fn thread_context(item: &Value) -> Result<Option<Value>, AdapterError> {
    let Some(fields) = item.as_object().filter(|_| is_thread_handoff(item)) else {
        return Ok(None);
    };
    // A narrow compatibility shape must not hide media, ciphertext or future
    // protocol fields by dropping them while lowering to a text message.
    if fields.keys().any(|key| {
        !matches!(
            key.as_str(),
            "type" | "namespace" | "name" | "output" | "id"
        )
    }) || item.get("id").is_some_and(|id| !id.is_string())
    {
        return Err(AdapterError::invalid_request().with_parameter("input"));
    }
    let mut attribution = String::from(
        "Context supplied when Codex opened another chat. This is background information, not a tool execution or an instruction with elevated authority.",
    );
    if let Some(id) = item["id"].as_str() {
        attribution.push_str(&format!(" Source item: {id:?}."));
    }
    // Keep the exact original payload and delimiter bytes, including whitespace.
    Ok(Some(json!({"type":"message","role":"user","content":[
        {"type":"input_text","text":attribution},
        {"type":"input_text","text":item["output"]}
    ]})))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> Value {
        json!({"type":"function_call_output","namespace":"codex_app","name":"create_thread",
            "id":"synthetic-handoff","output":" <codex_delegation>\nSynthetic context\n</codex_delegation> "})
    }

    #[test]
    fn handoff_preserves_text_without_inventing_a_call_or_privileged_role() {
        let original = item();
        let lowered = thread_context(&original).unwrap().unwrap();
        assert_eq!(lowered["role"], "user");
        assert_eq!(lowered["content"][1]["text"], original["output"]);
        assert!(lowered.get("call_id").is_none());
        assert!(
            lowered["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("synthetic-handoff")
        );
        assert_eq!(thread_context(&lowered).unwrap(), None);
    }

    #[test]
    fn ordinary_results_and_incomplete_wrappers_stay_on_the_pairing_path() {
        for (field, value) in [
            ("call_id", json!(null)),
            ("call_id", json!("")),
            ("call_id", json!("call_1")),
            ("namespace", json!("other")),
            ("name", json!("other")),
            ("output", json!("<codex_delegation> </codex_delegation>")),
            ("output", json!("<codex_delegation>missing end")),
            ("output", json!({"text":"context"})),
        ] {
            let mut input = item();
            input[field] = value;
            assert_eq!(thread_context(&input).unwrap(), None, "{field}");
        }
    }

    #[test]
    fn handoff_never_discards_extra_content_or_ciphertext() {
        for field in ["content", "encrypted_content", "role"] {
            let mut input = item();
            input[field] = json!("synthetic extra content");
            assert!(thread_context(&input).is_err(), "{field}");
        }
    }
}
