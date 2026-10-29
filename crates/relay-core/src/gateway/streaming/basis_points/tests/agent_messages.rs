use super::*;
use crate::protocol::agent_messages::AgentMessageContext;

#[test]
fn encrypted_agent_messages_are_sealed_once_and_never_leak_through_stream_events() {
    let context = AgentMessageContext::new(Default::default(), "synthetic-key");
    let request = json!({"model":"test","tools":[{"type":"function","name":"delegate","parameters":{
        "type":"object","properties":{"message":{"type":"string","encrypted":true}},
        "required":["message"]
    }}]});
    let request = PreparedClientRequest::from(request).with_agent_messages(context.clone());
    let mut translator = Translator::new(request);
    let text = "synthetic-private-task";
    let mut native = call("delegate");
    native["arguments"] =
        json!({"references":["delegate"],"code":json!({"message":text}).to_string()})
            .to_string()
            .into();
    translator.push(&frame(
        json!({"type":"response.created","response":{"status":"in_progress","output":[native]}}),
    ));
    translator.push(&frame(
        json!({"type":"response.output_item.added","output_index":0,"item":native}),
    ));
    translator.push(&frame(json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":native["id"],"delta":native["arguments"]})));
    translator.push(&frame(
        json!({"type":"response.output_item.done","output_index":0,"item":native}),
    ));
    let early = output(&mut translator);
    assert!(!early.contains("event: error"), "{early}");
    assert!(!early.contains(text));
    assert!(!early.contains("function_call_arguments"));
    let terminal = completed(json!([native]));
    translator.push(&frame(terminal.clone()));
    let body = output(&mut translator);
    assert!(!body.contains(text));
    assert!(!body.contains("event: error"));
    let mut pending = body.into_bytes();
    let mut arguments = Vec::new();
    while let Some(frame) = take_sse_event(&mut pending) {
        let event: Value = serde_json::from_slice(&sse_event_data(&frame)).unwrap();
        match event["type"].as_str().unwrap() {
            "response.function_call_arguments.delta" => arguments.push(event["delta"].clone()),
            "response.function_call_arguments.done" => arguments.push(event["arguments"].clone()),
            "response.output_item.done" => arguments.push(event["item"]["arguments"].clone()),
            "response.completed" => {
                arguments.push(event["response"]["output"][0]["arguments"].clone())
            }
            _ => {}
        }
    }
    assert_eq!(arguments.len(), 4);
    assert!(arguments.windows(2).all(|pair| pair[0] == pair[1]));
    let args: Value = serde_json::from_str(arguments[0].as_str().unwrap()).unwrap();
    let mut child = json!({"input":{"type":"agent_message","encrypted_content":args["message"]}});
    context.restore_request(&mut child).unwrap();
    assert_eq!(child["input"]["content"][1]["text"], text);
    translator.push(&frame(terminal));
    assert!(output(&mut translator).is_empty());
}
