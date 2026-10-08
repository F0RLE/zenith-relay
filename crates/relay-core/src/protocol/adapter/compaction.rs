use super::contracts::{AdapterError, AdapterResult};
use base64::Engine;
use serde_json::{json, Value};

pub(crate) const PREFIX: &str = "zenith-relay-compact-v1:";
const SUMMARY_INSTRUCTION: &str = "Summarize this conversation so a later turn can continue it. Preserve the user's goal, decisions, constraints, file paths, tool results, and unfinished work. Reply with only the summary.";

/// A bridged Responses request after Relay-owned compaction items are made
/// portable. The caller's value is never changed.
#[derive(Debug)]
pub(crate) enum BridgedCompaction {
    Unchanged,
    Rewritten {
        rewritten_request: Value,
        summarize: bool,
    },
}

impl BridgedCompaction {
    pub(crate) fn request<'a>(&'a self, original: &'a Value) -> &'a Value {
        match self {
            Self::Unchanged => original,
            Self::Rewritten {
                rewritten_request, ..
            } => rewritten_request,
        }
    }

    pub(crate) fn summarize(&self) -> bool {
        matches!(
            self,
            Self::Rewritten {
                summarize: true,
                ..
            }
        )
    }
}

/// Adds Codex compaction trigger once. An existing trigger keeps its position.
pub(crate) fn ensure_compaction_trigger(input_items: &mut Vec<Value>) {
    if !input_items.iter().any(|input_item| {
        input_item.get("type").and_then(Value::as_str) == Some("compaction_trigger")
    }) {
        input_items.push(json!({"type": "compaction_trigger"}));
    }
}

/// Turns Codex auto-compact into an ordinary text request for every non-native
/// route. A Relay checkpoint becomes normal text. Another provider's encrypted
/// checkpoint stays an explicit incompatibility.
pub(crate) fn prepare_bridged_compaction(request_body: &Value) -> AdapterResult<BridgedCompaction> {
    let Some(input_items) = compaction_items(request_body.get("input")) else {
        return Ok(BridgedCompaction::Unchanged);
    };
    if !input_items.iter().any(is_compaction_item) {
        return Ok(BridgedCompaction::Unchanged);
    }
    let mut summarize = false;
    let mut rewritten_input = Vec::with_capacity(input_items.len());
    for input_item in input_items {
        match input_item.get("type").and_then(Value::as_str) {
            Some("compaction_trigger") => summarize = true,
            Some("compaction" | "compaction_summary") => {
                let encrypted = input_item
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::compaction_unsupported)?;
                match decode_summary(encrypted)? {
                    Some(summary) => rewritten_input.push(summary_message(&summary)),
                    None => return Err(AdapterError::compaction_unsupported()),
                }
            }
            _ => rewritten_input.push(input_item.clone()),
        }
    }
    if summarize {
        rewritten_input.push(json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": SUMMARY_INSTRUCTION}]
        }));
    }
    let mut rewritten_request = request_body.clone();
    let request_object = rewritten_request
        .as_object_mut()
        .ok_or_else(AdapterError::invalid_request)?;
    request_object.insert("input".into(), Value::Array(rewritten_input));
    // Server-side context management has no equivalent on a bridged route.
    // Removing it does not invent a provider checkpoint.
    request_object.remove("context_management");
    if summarize {
        request_object.remove("tools");
        request_object.remove("tool_choice");
        request_object.remove("parallel_tool_calls");
    }
    Ok(BridgedCompaction::Rewritten {
        rewritten_request,
        summarize,
    })
}

pub(crate) fn wrap_compaction_response_bytes(bytes: &mut Vec<u8>) -> AdapterResult<()> {
    let mut upstream_response: Value =
        serde_json::from_slice(bytes).map_err(|_| AdapterError::upstream_response_invalid())?;
    wrap_compaction_response(&mut upstream_response)?;
    *bytes = serde_json::to_vec(&upstream_response)
        .map_err(|_| AdapterError::upstream_response_invalid())?;
    Ok(())
}

pub(crate) fn wrap_compaction_response(upstream_response: &mut Value) -> AdapterResult<()> {
    let response_id = upstream_response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("resp_compact")
        .to_string();
    let response_items = upstream_response
        .get_mut("output")
        .and_then(Value::as_array_mut)
        .ok_or_else(AdapterError::upstream_response_invalid)?;
    let mut summary_text = String::new();
    for output_item in response_items.iter() {
        if output_item.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(content_blocks) = output_item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for content_part in content_blocks {
            if content_part.get("type").and_then(Value::as_str) != Some("output_text") {
                continue;
            }
            if let Some(text) = content_part.get("text").and_then(Value::as_str) {
                summary_text.push_str(text);
            }
        }
    }
    let summary_text = summary_text.trim();
    if summary_text.is_empty() {
        return Err(AdapterError::upstream_response_invalid());
    }
    *response_items = vec![json!({
        "id": format!("cmp_{response_id}"),
        "type": "compaction",
        "encrypted_content": encode_summary(summary_text),
    })];
    Ok(())
}

fn compaction_items(input_value: Option<&Value>) -> Option<Vec<Value>> {
    match input_value? {
        Value::Array(items) => Some(items.clone()),
        Value::Object(_) => Some(vec![input_value?.clone()]),
        _ => None,
    }
}

/// Checkpoint items are provider state. A trigger only asks for a new summary.
pub(crate) fn is_compaction_checkpoint_type(item_type: &str) -> bool {
    matches!(item_type, "compaction" | "compaction_summary")
}

pub(super) fn is_compaction_item(input_item: &Value) -> bool {
    match input_item.get("type").and_then(Value::as_str) {
        Some("compaction_trigger") => true,
        Some(item_type) => is_compaction_checkpoint_type(item_type),
        None => false,
    }
}

fn summary_message(summary: &str) -> Value {
    json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": format!("Conversation summary:\n{summary}")}]
    })
}

fn encode_summary(summary: &str) -> String {
    let summary_payload = serde_json::to_vec(&json!({"summary": summary})).unwrap_or_default();
    format!(
        "{PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(summary_payload)
    )
}

fn decode_summary(encoded_summary: &str) -> AdapterResult<Option<String>> {
    let Some(encoded) = encoded_summary.strip_prefix(PREFIX) else {
        return Ok(None);
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| AdapterError::invalid_request().with_parameter("input"))?;
    let parsed: Value =
        serde_json::from_slice(&bytes).map_err(|_| AdapterError::invalid_request())?;
    let summary = parsed
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .ok_or_else(AdapterError::invalid_request)?;
    Ok(Some(summary.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error_codes;
    use serde_json::json;

    #[test]
    fn trigger_becomes_a_tool_free_summary_and_round_trips() {
        let request = json!({
            "model": "vendor/model",
            "tools": [{"type": "custom", "name": "apply_patch"}],
            "tool_choice": "auto",
            "context_management": [{"type": "compaction", "compact_threshold": 1000}],
            "input": [
                {"type": "message", "role": "user", "content": "Keep src/main.rs"},
                {"type": "compaction_trigger"}
            ]
        });
        let original = request.clone();
        let prepared = prepare_bridged_compaction(&request).unwrap();
        assert_eq!(request, original);
        assert!(matches!(prepared, BridgedCompaction::Rewritten { .. }));
        let BridgedCompaction::Rewritten {
            rewritten_request: rewritten,
            summarize,
        } = prepared
        else {
            return;
        };
        assert!(summarize);
        assert!(rewritten.get("tools").is_none());
        assert!(rewritten.get("tool_choice").is_none());
        assert!(rewritten.get("context_management").is_none());
        let input = rewritten["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert!(input.iter().all(|input_item| {
            input_item.get("type").and_then(Value::as_str) != Some("compaction_trigger")
        }));
        assert!(rewritten.to_string().contains("Keep src/main.rs"));
        assert!(rewritten
            .to_string()
            .contains("Reply with only the summary"));

        let mut response = json!({
            "id": "resp_test",
            "status": "completed",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Goal: keep src/main.rs"}]
            }]
        });
        wrap_compaction_response(&mut response).unwrap();
        assert_eq!(response["output"][0]["type"], "compaction");
        let encrypted = response["output"][0]["encrypted_content"].as_str().unwrap();
        assert!(encrypted.starts_with(PREFIX));

        let continued = json!({
            "model": "vendor/model",
            "input": [response["output"][0].clone(), {"type": "message", "role": "user", "content": "continue"}]
        });
        let prepared_checkpoint = prepare_bridged_compaction(&continued).unwrap();
        assert!(matches!(
            prepared_checkpoint,
            BridgedCompaction::Rewritten { .. }
        ));
        let BridgedCompaction::Rewritten {
            rewritten_request: continued,
            summarize,
        } = prepared_checkpoint
        else {
            return;
        };
        assert!(!summarize);
        assert!(continued["tools"].is_null() || continued.get("tools").is_none());
        assert!(continued.to_string().contains("Goal: keep src/main.rs"));
        assert!(continued["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|input_item| {
                input_item.get("type").and_then(Value::as_str) != Some("compaction")
            }));
    }

    #[test]
    fn foreign_encrypted_compaction_stays_explicit() {
        let request = json!({
            "input": [{"type": "compaction", "encrypted_content": "opaque-fixture"}]
        });
        let error = prepare_bridged_compaction(&request).unwrap_err();
        assert_eq!(error.code(), error_codes::ADAPTER_COMPACTION_UNSUPPORTED);
        assert!(error.is_route_incompatible());
    }

    #[test]
    fn ordinary_requests_are_not_copied() {
        let request = json!({"input": "hello", "tools": []});
        assert!(matches!(
            prepare_bridged_compaction(&request).unwrap(),
            BridgedCompaction::Unchanged
        ));
    }
}
