use super::{has_semantic_output, is_compaction_payload};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const MAX_CAPTURE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CAPTURE_ITEMS: usize = 1_024;

/// Volatile replay input, never diagnostics. Deltas alone cannot reconstruct
/// phase, tool state, or reasoning; retain the upstream's completed items.
#[derive(Default)]
pub(in crate::gateway) struct NativeReplayCapture {
    items: Vec<(Option<u64>, Value)>,
    pending: BTreeSet<u64>,
    retained_bytes: usize,
    unindexed_output: bool,
    disabled: bool,
}

impl NativeReplayCapture {
    pub(in crate::gateway) fn observe(&mut self, payload: &Value) {
        if self.disabled {
            return;
        }
        let kind = payload.get("type").and_then(Value::as_str);
        let index = payload.get("output_index").and_then(Value::as_u64);
        if kind == Some("response.output_item.done") {
            let Some(item) = payload.get("item").filter(|item| item.is_object()) else {
                self.unindexed_output = true;
                return;
            };
            let size = serde_json::to_vec(item).map_or(MAX_CAPTURE_BYTES + 1, |bytes| bytes.len());
            self.retained_bytes = self.retained_bytes.saturating_add(size);
            if self.retained_bytes > MAX_CAPTURE_BYTES || self.items.len() >= MAX_CAPTURE_ITEMS {
                self.disable();
                return;
            }
            if let Some(index) = index {
                self.pending.remove(&index);
                if let Some(existing) = self.items.iter_mut().find(|entry| entry.0 == Some(index)) {
                    existing.1 = item.clone();
                    return;
                }
            }
            self.items.push((index, item.clone()));
        } else if has_semantic_output(payload, kind)
            || is_compaction_payload(payload, kind)
            || kind == Some("response.output_item.added")
            || kind.is_some_and(|kind| kind.starts_with("response.") && kind.ends_with(".delta"))
        {
            if let Some(index) = index {
                self.pending.insert(index);
                if self.pending.len() > MAX_CAPTURE_ITEMS {
                    self.disable();
                }
            } else {
                self.unindexed_output = true;
            }
        }
    }

    pub(in crate::gateway) fn mark_unmaterialized(&mut self) {
        self.unindexed_output = true;
    }

    fn disable(&mut self) {
        self.disabled = true;
        self.items.clear();
        self.pending.clear();
    }

    pub(in crate::gateway) fn finish(
        mut self,
        response: Option<Value>,
        response_id: Option<&str>,
    ) -> Option<Value> {
        if self.disabled {
            return None;
        }
        let mut response = response.unwrap_or_else(|| json!({}));
        let object = response.as_object_mut()?;
        if !object
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.trim().is_empty())
        {
            let id = response_id.filter(|id| !id.trim().is_empty())?;
            object.insert("id".to_string(), Value::String(id.to_string()));
        }
        let output = object.get("output").and_then(Value::as_array);
        if output.is_none_or(Vec::is_empty) {
            if self.unindexed_output || !self.pending.is_empty() {
                return None;
            }
            if self.items.is_empty() && output.is_none() {
                return None;
            }
            if self.items.iter().all(|entry| entry.0.is_some()) {
                self.items.sort_by_key(|entry| entry.0);
            }
            object.insert(
                "output".to_string(),
                Value::Array(self.items.into_iter().map(|entry| entry.1).collect()),
            );
        }
        let size = serde_json::to_vec(&response).ok()?.len();
        (size <= MAX_CAPTURE_BYTES).then_some(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_output_preserves_phase_and_encrypted_reasoning() {
        let mut capture = NativeReplayCapture::default();
        capture.observe(&json!({"type":"response.output_text.delta","delta":"hello"}));
        let response = json!({"id":"resp_complete","output":[
            {"type":"reasoning","encrypted_content":"synthetic","summary":[]},
            {"type":"message","role":"assistant","phase":"final_answer",
             "content":[{"type":"output_text","text":"hello"}]}
        ]});
        assert_eq!(capture.finish(Some(response.clone()), None), Some(response));
    }

    #[test]
    fn sparse_terminal_requires_completed_items_for_every_observed_index() {
        let mut capture = NativeReplayCapture::default();
        capture
            .observe(&json!({"type":"response.output_text.delta","output_index":0,"delta":"text"}));
        capture.observe(&json!({"type":"response.output_item.done","output_index":1,
            "item":{"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{}"}}));
        assert!(capture
            .finish(Some(json!({"id":"resp_partial"})), None)
            .is_none());
    }

    #[test]
    fn completed_items_replace_deltas_without_losing_order_or_phase() {
        let mut capture = NativeReplayCapture::default();
        capture
            .observe(&json!({"type":"response.output_text.delta","output_index":0,"delta":"text"}));
        let message = json!({"type":"message","role":"assistant","phase":"commentary",
            "content":[{"type":"output_text","text":"text"}]});
        let call =
            json!({"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{}"});
        capture.observe(&json!({"type":"response.output_item.done","output_index":1,"item":call}));
        capture
            .observe(&json!({"type":"response.output_item.done","output_index":0,"item":message}));
        let response = capture
            .finish(Some(json!({"id":"resp_complete","output":[]})), None)
            .unwrap();
        assert_eq!(response["output"], json!([message, call]));
    }

    #[test]
    fn unindexed_text_is_never_synthesized_into_an_assistant_message() {
        let mut capture = NativeReplayCapture::default();
        capture.observe(&json!({"type":"response.output_text.delta","delta":"hello"}));
        assert!(capture
            .finish(Some(json!({"id":"resp_sparse","output":[]})), None)
            .is_none());
    }

    #[test]
    fn capture_stops_retaining_oversized_output_without_truncating_history() {
        let mut capture = NativeReplayCapture::default();
        capture.observe(&json!({"type":"response.output_item.done","item":{
            "type":"message","content":"x".repeat(MAX_CAPTURE_BYTES)}}));
        assert!(capture.items.is_empty());
        assert!(capture
            .finish(Some(json!({"id":"resp_large","output":[]})), None)
            .is_none());
    }
}
