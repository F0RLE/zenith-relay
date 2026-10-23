use super::*;

fn batch() -> Value {
    let mut item = call("inspect");
    item["arguments"] = json!({"references":["inspect","inspect"],"code":"relay-tool-batch-v1\n[{\"path\":\"a\"},{\"path\":\"b\"}]"}).to_string().into();
    item
}

fn events(translator: &mut Translator) -> Vec<Value> {
    let mut bytes = output(translator).into_bytes();
    let mut events = Vec::new();
    while let Some(frame) = take_sse_event(&mut bytes) {
        events.push(serde_json::from_slice(&sse_event_data(&frame)).unwrap());
    }
    events
}

#[test]
fn batch_buffers_tail_and_emits_final_indexes_once_with_opaque_reasoning() {
    for arguments_first in [false, true] {
        let mut translator = Translator::new(request());
        let leading = json!({"id":"msg_leading","type":"message","role":"assistant","content":[{"type":"output_text","text":"before"}]});
        let tail = json!({"id":"msg_tail","type":"message","role":"assistant","content":[{"type":"output_text","text":"after"}]});
        let reasoning = json!({"id":"rs_tail","type":"reasoning","summary":[{"type":"summary_text","text":"summary"}],"encrypted_content":"synthetic-opaque"});
        translator.push(&frame(
            json!({"type":"response.output_text.delta","output_index":0,"delta":"before"}),
        ));
        assert_eq!(events(&mut translator).len(), 1);
        if arguments_first {
            translator.push(&frame(json!({"type":"response.function_call_arguments.delta","output_index":1,"item_id":"fc_native","delta":"partial"})));
        }
        translator.push(&frame(
            json!({"type":"response.output_item.done","output_index":1,"item":batch()}),
        ));
        translator.push(&frame(
            json!({"type":"response.output_text.delta","output_index":2,"delta":"after"}),
        ));
        translator.push(&frame(
            json!({"type":"response.output_item.done","output_index":3,"item":reasoning}),
        ));
        assert!(events(&mut translator).is_empty());
        let terminal = completed(json!([leading, batch(), tail, reasoning]));
        translator.push(&frame(terminal.clone()));
        translator.push(&frame(terminal));
        let events = events(&mut translator);
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "response.completed")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "response.function_call_arguments.delta")
                .count(),
            2
        );
        let deltas = events
            .iter()
            .filter(|e| e["type"] == "response.output_text.delta")
            .collect::<Vec<_>>();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["output_index"], 3);
        assert_eq!(deltas[0]["delta"], "after");
        assert_eq!(events.last().unwrap()["response"]["output"][4], reasoning);
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event["sequence_number"], index + 1);
        }
    }
}

#[test]
fn invalid_or_incomplete_batches_never_emit_any_executable_prefix() {
    let mut malformed = batch();
    malformed["arguments"] = json!({"references":["inspect","unknown"],"code":"relay-tool-batch-v1\n[{\"path\":\"a\"},{}]"}).to_string().into();
    for terminal in [
        completed(json!([malformed])),
        json!({"type":"response.incomplete","response":{"status":"incomplete","output":[batch()]}}),
        json!({"type":"response.failed","response":{"status":"failed","output":[batch()]}}),
    ] {
        let mut translator = Translator::new(request());
        translator.push(&frame(
            json!({"type":"response.output_item.done","output_index":0,"item":batch()}),
        ));
        translator.push(&frame(terminal));
        let wire = output(&mut translator);
        assert!(!wire.contains("function_call_arguments"));
        assert!(!wire.contains("\"type\":\"function_call\""));
        assert!(translator.terminal);
    }
}

#[test]
fn terminal_only_batch_is_valid_but_cannot_renumber_already_streamed_tail() {
    let mut translator = Translator::new(request());
    translator.push(&frame(completed(json!([batch()]))));
    assert_eq!(
        events(&mut translator)
            .iter()
            .filter(|e| e["type"] == "response.function_call_arguments.delta")
            .count(),
        2
    );
    let mut translator = Translator::new(request());
    translator.push(&frame(
        json!({"type":"response.output_text.delta","output_index":1,"delta":"too early"}),
    ));
    output(&mut translator);
    translator.push(&frame(completed(json!([batch(),{"id":"msg_tail","type":"message","content":[{"type":"output_text","text":"too early"}]}]))));
    let wire = output(&mut translator);
    assert!(wire.contains("stream.output_index"));
    assert!(!wire.contains("function_call_arguments"));
}
