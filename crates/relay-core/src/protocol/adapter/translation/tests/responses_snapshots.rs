use super::*;

#[test]
fn optional_completed_reasoning_status_preserves_foreign_streams_and_native_state() {
    for client in [WireApi::Messages, WireApi::ChatCompletions, WireApi::Gemini] {
        for omitted_in_done in [false, true] {
            let mut done = json!({"type":"reasoning","id":"rs_synthetic","summary":[],
                "encrypted_content":"synthetic-done-cipher"});
            let mut terminal_reasoning = done.clone();
            if omitted_in_done {
                terminal_reasoning["status"] = json!("completed");
            } else {
                done["status"] = json!("completed");
            }
            terminal_reasoning["encrypted_content"] = json!("synthetic-terminal-cipher");
            let mut terminal = super::architecture::tool_reply(WireApi::Responses);
            terminal["output"]
                .as_array_mut()
                .unwrap()
                .insert(0, terminal_reasoning.clone());
            let mut initial = done.clone();
            initial["status"] = json!("in_progress");
            initial["encrypted_content"] = json!("synthetic-incomplete-cipher");
            let mut events = vec![
                json!({"type":"response.created","response":{"id":terminal["id"],"output":[]}}),
                json!({"type":"response.output_item.added","output_index":0,"item":initial}),
                json!({"type":"response.output_item.done","output_index":0,"item":done}),
            ];
            for (index, item) in terminal["output"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .skip(1)
            {
                events.push(
                    json!({"type":"response.output_item.added","output_index":index,"item":item}),
                );
                events.push(
                    json!({"type":"response.output_item.done","output_index":index,"item":item}),
                );
            }
            events.push(json!({"type":"response.completed","response":terminal}));
            let wire = events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect::<String>();
            for chunk_size in [1, 7, wire.len()] {
                let mut request = input(client);
                request["tools"] = tool_history(client)["tools"].clone();
                let mut bridge = prepare(client, WireApi::Responses, &request, true)
                    .into_stream_bridge()
                    .unwrap();
                for chunk in wire.as_bytes().chunks(chunk_size) {
                    bridge.push(chunk);
                }
                bridge.finish();
                let frames = String::from_utf8(
                    std::iter::from_fn(|| bridge.pop_output())
                        .flatten()
                        .collect(),
                )
                .unwrap();
                let completed = bridge.completed().unwrap_or_else(|| panic!("{client:?}, omitted in done={omitted_in_done}, chunk={chunk_size}: {frames}")).clone();
                let mut expected = terminal_reasoning.clone();
                expected["encrypted_content"] = json!("synthetic-done-cipher");
                assert_eq!(completed.continuation.native_turns[&1][0], expected);
                assert!(!frames.contains("event: error"), "{frames}");
                assert!(!frames.contains("synthetic-done-cipher"));
                assert!(!frames.contains("synthetic-terminal-cipher"));
                let next =
                    super::architecture::append_tool_results(client, request, &completed, true);
                let followup = SourceAdapter::between(client, WireApi::Responses)
                    .unwrap()
                    .prepare_request(AdapterRequestContext {
                        client_wire_api: client,
                        request: &next,
                        model: "test",
                        stream: false,
                        reasoning_mode: MessagesReasoningMode::Adaptive,
                        cache_write_ttl: CacheWriteTtl::Provider,
                        previous: Some(completed.continuation),
                        response_scope: "source-test",
                        response_id_seed: "next-tool-turn",
                    })
                    .unwrap();
                let restored = followup.upstream_body()["input"].as_array().unwrap();
                assert!(restored.contains(&expected));
                assert_eq!(
                    restored
                        .iter()
                        .filter(|item| item["type"] == "function_call_output")
                        .count(),
                    2
                );
                let answer = followup
                    .translate_response_bytes(
                        &serde_json::to_vec(&output(WireApi::Responses)).unwrap(),
                    )
                    .unwrap()
                    .unwrap();
                assert!(answer.response_body().to_string().contains("Hi"));
            }
        }
    }
}

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
