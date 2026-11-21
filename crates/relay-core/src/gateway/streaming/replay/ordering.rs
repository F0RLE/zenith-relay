use super::*;

fn item(index: usize) -> Value {
    json!({"type":"message","id":format!("msg_synthetic_{index}"),"role":"assistant",
        "content":[{"type":"output_text","text":format!("synthetic {index}")}]})
}

fn capture(indices: &[Option<Value>]) -> NativeReplayCapture {
    let mut capture = NativeReplayCapture::default();
    for (position, index) in indices.iter().enumerate() {
        let mut event = json!({"type":"response.output_item.done","item":item(position)});
        if let Some(index) = index {
            event["output_index"] = index.clone();
        }
        capture.observe(&event);
    }
    capture
}

#[test]
fn sparse_terminal_cannot_hide_gapped_mixed_or_malformed_indices() {
    for indices in [
        vec![Some(json!(1))],
        vec![Some(json!(0)), Some(json!(2))],
        vec![Some(json!(u64::MAX))],
        vec![None, Some(json!(0))],
        vec![Some(json!(0)), None],
        vec![Some(Value::Null)],
        vec![Some(json!("0"))],
        vec![Some(json!(-1))],
    ] {
        for terminal in [
            json!({"id":"resp_synthetic"}),
            json!({"id":"resp_synthetic","output":[]}),
        ] {
            assert!(
                capture(&indices).finish(Some(terminal), None).is_none(),
                "unproven sparse ordering: {indices:?}"
            );
        }
    }
}

#[test]
fn out_of_order_complete_indices_and_unindexed_items_keep_existing_ordering() {
    for (indices, expected) in [
        (
            vec![Some(json!(1)), Some(json!(0))],
            json!([item(1), item(0)]),
        ),
        (vec![None, None], json!([item(0), item(1)])),
    ] {
        let response = capture(&indices)
            .finish(Some(json!({"id":"resp_synthetic","output":[]})), None)
            .unwrap();
        assert_eq!(response["output"], expected);
    }
}

#[test]
fn complete_terminal_can_supply_items_missing_from_sparse_capture() {
    let terminal = json!({"id":"resp_synthetic","output":[item(0),item(1)]});
    let mut capture = NativeReplayCapture::default();
    capture.observe(&json!({"type":"response.output_item.done","output_index":1,"item":item(1)}));
    assert_eq!(capture.finish(Some(terminal.clone()), None), Some(terminal));
}
