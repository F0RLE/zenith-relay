use super::*;

#[test]
fn responses_done_reasoning_remains_authoritative_in_all_foreign_streams() {
    let done = json!({"type":"reasoning","id":"rs_synthetic","summary":[],
        "status":"completed","encrypted_content":"synthetic-done-cipher"});
    for client in [WireApi::Messages, WireApi::ChatCompletions, WireApi::Gemini] {
        for replacement in [
            None,
            Some(Value::Null),
            Some(json!("synthetic-terminal-cipher")),
        ] {
            let mut terminal = output(WireApi::Responses);
            terminal["output"]
                .as_array_mut()
                .unwrap()
                .insert(0, done.clone());
            terminal["output"][0]
                .as_object_mut()
                .unwrap()
                .remove("encrypted_content");
            if let Some(replacement) = replacement {
                terminal["output"][0]["encrypted_content"] = replacement;
            }
            let wire = [
                json!({"type":"response.output_item.added","output_index":0,"item":{
                    "type":"reasoning","id":"rs_synthetic","summary":[],"encrypted_content":"synthetic-incomplete-cipher"}}),
                json!({"type":"response.output_item.done","output_index":0,"item":done}),
                json!({"type":"response.output_item.added","output_index":1,"item":terminal["output"][1]}),
                json!({"type":"response.output_item.done","output_index":1,"item":terminal["output"][1]}),
                json!({"type":"response.completed","response":terminal}),
            ].iter().map(|event| format!("data: {event}\n\n")).collect::<String>();
            for chunk_size in [1, 7, wire.len()] {
                let mut bridge = prepare(client, WireApi::Responses, &input(client), true)
                    .into_stream_bridge()
                    .unwrap();
                for chunk in wire.as_bytes().chunks(chunk_size) {
                    bridge.push(chunk);
                }
                bridge.finish();
                let completed = bridge
                    .completed()
                    .expect("done reasoning must survive the terminal snapshot");
                assert_eq!(completed.continuation.native_turns[&1][0], done);
                let frames = String::from_utf8(
                    std::iter::from_fn(|| bridge.pop_output())
                        .flatten()
                        .collect(),
                )
                .unwrap();
                assert!(!frames.contains("synthetic-done-cipher"));
                assert!(!frames.contains("synthetic-terminal-cipher"));
                assert!(!frames.contains("event: error"));
            }
        }
    }
}

#[test]
fn responses_without_reasoning_done_keep_the_terminal_ciphertext() {
    for client in [WireApi::Messages, WireApi::ChatCompletions, WireApi::Gemini] {
        let reasoning = json!({"type":"reasoning","id":"rs_synthetic","summary":[],
            "encrypted_content":"synthetic-terminal-cipher"});
        let mut terminal = output(WireApi::Responses);
        terminal["output"]
            .as_array_mut()
            .unwrap()
            .insert(0, reasoning.clone());
        let mut bridge = prepare(client, WireApi::Responses, &input(client), true)
            .into_stream_bridge()
            .unwrap();
        for event in [
            json!({"type":"response.output_item.added","output_index":0,"item":{
                "type":"reasoning","id":"rs_synthetic","summary":[],"encrypted_content":"synthetic-incomplete-cipher"}}),
            json!({"type":"response.output_item.added","output_index":1,"item":terminal["output"][1]}),
            json!({"type":"response.completed","response":terminal}),
        ] {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        bridge.finish();
        let completed = bridge
            .completed()
            .expect("a missing done event must not promote incomplete state");
        assert_eq!(completed.continuation.native_turns[&1][0], reasoning);
    }
}

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
