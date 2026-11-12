use super::*;

fn done(index: u64, bytes: usize) -> Value {
    json!({"type":"response.output_item.done","output_index":index,"item":{
        "type":"message","id":format!("msg_synthetic_{index}"),"role":"assistant",
        "content":[{"type":"output_text","text":"x".repeat(bytes)}]
    }})
}

#[test]
fn repeated_indexed_snapshot_does_not_exhaust_retained_bytes() {
    let event = done(0, MAX_CAPTURE_BYTES / 2 + 128);
    let mut capture = NativeReplayCapture::default();
    for _ in 0..3 {
        capture.observe(&event);
    }
    assert!(!capture.disabled);
    assert_eq!(capture.captured_items.len(), 1);
    assert_eq!(
        capture.retained_bytes,
        serde_json::to_vec(&event["item"]).unwrap().len()
    );
    let response = capture
        .finish(Some(json!({"id":"resp_synthetic","output":[]})), None)
        .unwrap();
    assert_eq!(response["output"], json!([event["item"]]));
}

#[test]
fn equivalent_indexed_replacements_charge_only_the_current_snapshot() {
    let mut capture = NativeReplayCapture::default();
    let unchanged = done(1, 128);
    capture.observe(&unchanged);
    let unchanged_bytes = serde_json::to_vec(&unchanged["item"]).unwrap().len();
    for phase in [None, Some(Value::Null), Some(Value::Null), None] {
        let mut event = done(0, MAX_CAPTURE_BYTES / 2);
        if let Some(phase) = phase {
            event["item"]["phase"] = phase;
        }
        capture.observe(&event);
        assert!(!capture.disabled);
        assert_eq!(capture.captured_items.len(), 2);
        assert_eq!(
            capture.retained_bytes,
            unchanged_bytes + serde_json::to_vec(&event["item"]).unwrap().len()
        );
    }
    capture.observe(&done(2, MAX_CAPTURE_BYTES / 2));
    assert!(
        capture.disabled,
        "distinct snapshots still obey the total retained-byte bound"
    );
    assert!(capture.captured_items.is_empty());
}

#[test]
fn replacing_at_the_item_bound_preserves_distinct_item_limits() {
    let mut capture = NativeReplayCapture::default();
    for index in 0..MAX_CAPTURE_ITEMS {
        capture.observe(&done(index as u64, 1));
    }
    capture.observe(&done(0, 1));
    assert!(!capture.disabled);
    assert_eq!(capture.captured_items.len(), MAX_CAPTURE_ITEMS);
    capture.observe(&done(MAX_CAPTURE_ITEMS as u64, 1));
    assert!(capture.disabled, "a new index still exceeds the item bound");
    assert!(capture.captured_items.is_empty());
}
