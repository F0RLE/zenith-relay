use super::*;

#[test]
fn responses_done_and_terminal_allow_absent_or_null_optional_phase() {
    for client in [WireApi::Messages, WireApi::ChatCompletions, WireApi::Gemini] {
        for null_in_done in [false, true] {
            let mut terminal = output(WireApi::Responses);
            terminal["output"][0]
                .as_object_mut()
                .unwrap()
                .remove("phase");
            let mut done_item = terminal["output"][0].clone();
            if null_in_done {
                done_item["phase"] = Value::Null;
            } else {
                terminal["output"][0]["phase"] = Value::Null;
            }
            let mut bridge = prepare(client, WireApi::Responses, &input(client), true)
                .into_stream_bridge()
                .unwrap();
            for kind in ["response.output_item.added", "response.output_item.done"] {
                let event = json!({"type":kind,"output_index":0,"item":done_item});
                bridge.push(format!("data: {event}\n\n").as_bytes());
            }
            let event = json!({"type":"response.completed","response":terminal});
            bridge.push(format!("data: {event}\n\n").as_bytes());
            bridge.finish();
            assert!(
                bridge.completed().is_some(),
                "{client:?}, null in done: {null_in_done}"
            );
        }
    }
}
