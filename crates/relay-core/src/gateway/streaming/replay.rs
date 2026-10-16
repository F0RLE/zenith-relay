use super::{has_semantic_output, is_compaction_payload};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const MAX_CAPTURE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CAPTURE_ITEMS: usize = 1_024;

/// Volatile replay input, never diagnostics. Deltas alone cannot reconstruct
/// phase, tool state, or reasoning; retain the upstream's completed items.
#[derive(Default)]
pub(in crate::gateway) struct NativeReplayCapture {
    captured_items: Vec<(Option<u64>, Value)>,
    pending: BTreeSet<u64>,
    retained_bytes: usize,
    unindexed_output: bool,
    disabled: bool,
}

impl NativeReplayCapture {
    pub(in crate::gateway) fn observe(&mut self, event_payload: &Value) {
        if self.disabled {
            return;
        }
        let event_type = event_payload.get("type").and_then(Value::as_str);
        let output_index = event_payload.get("output_index").and_then(Value::as_u64);
        if event_type == Some("response.output_item.done") {
            let Some(output_item) = event_payload
                .get("item")
                .filter(|output_item| output_item.is_object())
            else {
                self.unindexed_output = true;
                return;
            };
            let item_size =
                serde_json::to_vec(output_item).map_or(MAX_CAPTURE_BYTES + 1, |bytes| bytes.len());
            self.retained_bytes = self.retained_bytes.saturating_add(item_size);
            if self.retained_bytes > MAX_CAPTURE_BYTES
                || self.captured_items.len() >= MAX_CAPTURE_ITEMS
            {
                self.disable();
                return;
            }
            if let Some(output_index) = output_index {
                self.pending.remove(&output_index);
                if let Some(existing) = self
                    .captured_items
                    .iter_mut()
                    .find(|captured_item| captured_item.0 == Some(output_index))
                {
                    existing.1 = output_item.clone();
                    return;
                }
            }
            self.captured_items
                .push((output_index, output_item.clone()));
        } else if has_semantic_output(event_payload, event_type)
            || is_compaction_payload(event_payload, event_type)
            || event_type == Some("response.output_item.added")
            || event_type.is_some_and(|event_type| {
                event_type.starts_with("response.") && event_type.ends_with(".delta")
            })
        {
            self.observe_response_delta(output_index);
        }
    }

    pub(in crate::gateway) fn observe_response_delta(&mut self, output_index: Option<u64>) {
        if self.disabled {
            return;
        }
        if let Some(index) = output_index {
            self.pending.insert(index);
            if self.pending.len() > MAX_CAPTURE_ITEMS {
                self.disable();
            }
        } else {
            self.unindexed_output = true;
        }
    }

    pub(in crate::gateway) fn mark_unmaterialized(&mut self) {
        self.unindexed_output = true;
    }

    fn disable(&mut self) {
        self.disabled = true;
        self.captured_items.clear();
        self.pending.clear();
    }

    pub(in crate::gateway) fn finish(
        mut self,
        completed_response: Option<Value>,
        response_id: Option<&str>,
    ) -> Option<Value> {
        if self.disabled {
            return None;
        }
        let mut replay_response = completed_response.unwrap_or_else(|| json!({}));
        let response_object = replay_response.as_object_mut()?;
        if !response_object
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|response_id_value| !response_id_value.trim().is_empty())
        {
            let response_id_value =
                response_id.filter(|response_id_value| !response_id_value.trim().is_empty())?;
            response_object.insert(
                "id".to_string(),
                Value::String(response_id_value.to_string()),
            );
        }
        let output_items = response_object.get("output").and_then(Value::as_array);
        if output_items.is_none_or(Vec::is_empty) {
            if self.unindexed_output || !self.pending.is_empty() {
                return None;
            }
            if self.captured_items.is_empty() && output_items.is_none() {
                return None;
            }
            if self
                .captured_items
                .iter()
                .all(|captured_item| captured_item.0.is_some())
            {
                self.captured_items
                    .sort_by_key(|captured_item| captured_item.0);
            }
            response_object.insert(
                "output".to_string(),
                Value::Array(
                    self.captured_items
                        .into_iter()
                        .map(|captured_item| captured_item.1)
                        .collect(),
                ),
            );
        }
        let response_size = serde_json::to_vec(&replay_response).ok()?.len();
        (response_size <= MAX_CAPTURE_BYTES).then_some(replay_response)
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
        assert!(capture.captured_items.is_empty());
        assert!(capture
            .finish(Some(json!({"id":"resp_large","output":[]})), None)
            .is_none());
    }

    #[test]
    fn response_delta_index_tracking_does_not_need_the_payload_tree() {
        let mut indexed_capture = NativeReplayCapture::default();
        let mut from_index = NativeReplayCapture::default();
        indexed_capture
            .observe(&json!({"type":"response.output_text.delta","output_index":2,"delta":"x"}));
        from_index.observe_response_delta(Some(2));
        assert_eq!(indexed_capture.pending, from_index.pending);
        assert_eq!(
            indexed_capture.unindexed_output,
            from_index.unindexed_output
        );

        let mut unindexed_capture = NativeReplayCapture::default();
        let mut unindexed_fast = NativeReplayCapture::default();
        unindexed_capture
            .observe(&json!({"type":"response.function_call_arguments.delta","delta":"{"}));
        unindexed_fast.observe_response_delta(None);
        assert!(unindexed_capture.unindexed_output);
        assert!(unindexed_fast.unindexed_output);
        assert!(unindexed_capture.pending.is_empty());
        assert!(unindexed_fast.pending.is_empty());
    }
}
