use super::*;

#[test]
fn codex_handoff_is_context_in_json_and_streaming_without_changing_turn_metadata() {
    for stream in [false, true] {
        let mut request = request_with_tool();
        request["stream"] = json!(stream);
        request["input"] = json!([
            {"role":"user","content":"Synthetic task"},
            {"type":"function_call_output","namespace":"codex_app","name":"create_thread",
             "id":"synthetic-handoff","output":"<codex_delegation>\nSynthetic context\n</codex_delegation>"},
            {"role":"developer","content":"Keep the original instructions"},
            {"type":"function_call","name":"exec_command","call_id":"call_synthetic","arguments":"{\"cmd\":\"echo synthetic\"}"},
            {"type":"function_call_output","call_id":"call_synthetic","output":"Synthetic tool result"}
        ]);
        let before = request.clone();
        let prepared = prepare_request(&request).unwrap();
        let items = prepared["input"].as_array().unwrap();
        let handoff = items
            .iter()
            .find(|item| item["content"][1]["text"] == request["input"][1]["output"])
            .unwrap();
        assert_eq!(handoff["type"], "message");
        assert_eq!(handoff["role"], "user");
        assert_eq!(
            items
                .iter()
                .filter(|item| item["type"] == "function_call")
                .count(),
            1
        );
        let result = items
            .iter()
            .find(|item| item["type"] == "function_call_output")
            .unwrap();
        assert_eq!(result["call_id"], "call_synthetic");
        assert_eq!(result["output"], "Synthetic tool result");
        assert_eq!(prepared["stream"], stream);
        assert_eq!(request, before);
        request["input"].as_array_mut().unwrap().remove(1);
        assert_eq!(
            prepared["metadata"],
            prepare_request(&request).unwrap()["metadata"]
        );
    }
}

#[test]
fn translating_large_tool_results_reuses_the_owned_output_buffer() {
    use super::super::history::translate_input_items;

    let output = json!({
        "type":"custom_tool_call_output", "call_id":"call_buffer_reuse",
        "name":"old_name", "namespace":"old_namespace",
        "output":"synthetic output\n".repeat(64 * 1024)
    });
    let original = output["output"].as_str().unwrap();
    let pointer = original.as_ptr();
    let length = original.len();
    let translated = translate_input_items(
        vec![
            json!({"type":"function_call","name":"run_officejs", "call_id":"call_buffer_reuse","arguments":"{}"}),
            output,
        ],
        &[],
    )
    .unwrap();
    let result = &translated[1];
    assert_eq!(result["type"], "function_call_output");
    assert_eq!(result["call_id"], "call_buffer_reuse");
    assert!(result.get("name").is_none());
    assert!(result.get("namespace").is_none());
    let retained = result["output"].as_str().unwrap();
    assert_eq!(retained.as_ptr(), pointer);
    assert_eq!(retained.len(), length);
}

#[test]
fn standalone_named_function_outputs_keep_their_native_identity() {
    for call_id in [None, Some(Value::Null)] {
        for stream in [false, true] {
            let mut output = json!({
                "type":"function_call_output", "id":"fc_synthetic_context",
                "name":"context_lookup", "namespace":"synthetic",
                "caller":{"type":"direct"}, "status":"completed",
                "output":"Synthetic external context"
            });
            if let Some(call_id) = &call_id {
                output["call_id"] = call_id.clone();
            }
            let mut request = request_with_tool();
            request["stream"] = json!(stream);
            request["input"] = json!([
                {"role":"user","content":"Synthetic task"},
                output,
                {"role":"developer","content":"Keep the original context"}
            ]);
            let before = request.clone();
            let prepared = prepare_request(&request).unwrap();
            let items = prepared["input"].as_array().unwrap();
            let preserved = items
                .iter()
                .find(|item| item["type"] == "function_call_output")
                .unwrap();
            assert_eq!(preserved, &request["input"][1]);
            assert!(!items.iter().any(|item| item["type"] == "function_call"));
            assert_eq!(prepared["stream"], stream);
            assert_eq!(request, before);
        }
    }
}

#[test]
fn standalone_outputs_do_not_make_invalid_call_ids_or_custom_results_valid() {
    for output in [
        json!({"type":"function_call_output","name":"lookup","call_id":"","output":"Synthetic"}),
        json!({"type":"function_call_output","name":"lookup","call_id":"  ","output":"Synthetic"}),
        json!({"type":"function_call_output","name":"lookup","call_id":42,"output":"Synthetic"}),
        json!({"type":"function_call_output","name":"","output":"Synthetic"}),
        json!({"type":"function_call_output","name":42,"output":"Synthetic"}),
        json!({"type":"function_call_output","output":"Synthetic"}),
        json!({"type":"custom_tool_call_output","name":"lookup","output":"Synthetic"}),
    ] {
        let mut request = request_with_tool();
        request["input"] = json!([output]);
        assert_eq!(
            prepare_request(&request).unwrap_err().parameter(),
            Some("input.call_id")
        );
    }
}

#[test]
fn complex_function_history_uses_the_same_frame_as_new_calls() {
    let script = "Write-Output \"quoted\"\r\n$path = 'C:\\synthetic\\dir'\n\t😀\n".repeat(8);
    let arguments = json!({"code":script,"path":"C:\\synthetic\\file","limit":123});
    let request = json!({
        "model":"gpt-test",
        "tools":[{"type":"function","name":"inspect_script","parameters":{
            "type":"object","properties":{"code":{"type":"string"},"path":{"type":"string"},"limit":{"type":"integer"}},
            "required":["code","path","limit"],"additionalProperties":false
        }}],
        "input":[
            {"type":"function_call","name":"inspect_script","call_id":"call_previous","arguments":arguments.to_string()},
            {"type":"function_call_output","call_id":"call_previous","output":"synthetic result"},
            {"role":"user","content":"Continue"}
        ]
    });
    let before = request.clone();
    let prepared = prepare_request(&request).unwrap();
    let mut call = prepared["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call")
        .unwrap()
        .clone();
    let envelope: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
    let code = envelope["code"].as_str().unwrap();
    assert!(code.starts_with(super::super::FUNCTION_STRING_PREFIX));
    assert!(code.ends_with(&script));
    assert_eq!(
        super::super::codec::function_arguments(&envelope["code"]).unwrap(),
        arguments
    );
    assert_eq!(request, before);
    call["call_id"] = json!("call_next");
    let response = json!({"id":"resp_synthetic","status":"completed","output":[call]});
    let translated = translate_response(&serde_json::to_vec(&response).unwrap(), &request).unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["name"], "inspect_script");
    assert_eq!(translated["output"][0]["call_id"], "call_next");
    assert_eq!(
        serde_json::from_str::<Value>(translated["output"][0]["arguments"].as_str().unwrap())
            .unwrap(),
        arguments
    );
}

#[test]
fn attribution_is_preserved_as_context_without_changing_authority_or_arguments() {
    let mut request = request_with_tool();
    request["input"] = json!([
        {"role":"assistant","author":"worker","recipient":"root","phase":"commentary","content":"Ready"},
        {"type":"agent_message","role":"developer","author":"worker","content":"Report"},
        {"type":"function_call","name":"exec_command","call_id":"call_1","arguments":"{\"cmd\":\"author recipient\"}"},
        {"type":"function_call_output","call_id":"call_1","output":"ok"}
    ]);
    let prepared = prepare_request(&request).unwrap();
    let items = prepared["input"].as_array().unwrap();
    let assistant = &items[1];
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["phase"], "commentary");
    assert!(assistant.get("author").is_none());
    assert!(assistant.get("recipient").is_none());
    assert!(assistant["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("worker"));
    assert_eq!(items[2]["role"], "user");
    assert_eq!(items[2]["content"][1]["text"], "Report");
    let arguments: Value = serde_json::from_str(items[3]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments["code"], "{\"cmd\":\"author recipient\"}");
}

#[test]
fn tool_images_keep_result_order_call_association_and_logical_turn() {
    for kind in ["function", "custom_tool"] {
        let mut request = request_with_tool();
        request["input"] = json!([
            {"role":"user","content":"Inspect"},
            {"type":"function_call","name":"exec_command","call_id":"call_1","arguments":"{\"cmd\":\"inspect\"}"},
            {"type":format!("{kind}_call_output"),"call_id":"call_1","output":[
                {"type":"input_text","text":"Before"},
                {"type":"input_image","file_id":"file-synthetic-a"},
                {"type":"input_text","text":"Between"},
                {"type":"input_image","image_url":"data:image/png;base64,synthetic"}
            ]}
        ]);
        let prepared = prepare_request(&request).unwrap();
        let items = prepared["input"].as_array().unwrap();
        let result = &items[3];
        assert_eq!(result["call_id"], "call_1");
        assert_eq!(result["output"][0]["text"], "Before");
        assert_eq!(result["output"][2]["text"], "Between");
        assert_eq!(items[4]["content"][2]["file_id"], "file-synthetic-a");
        assert_eq!(
            items[4]["content"][4]["image_url"],
            "data:image/png;base64,synthetic"
        );
        assert!(items[4]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not a new user instruction"));
        assert_eq!(prepared["metadata"]["agent_iteration"], "2");
        let mut text_only = request.clone();
        text_only["input"][2]["output"] = json!("ok");
        assert_eq!(
            prepare_request(&text_only).unwrap()["metadata"],
            prepared["metadata"]
        );
    }
}

#[test]
fn invalid_agent_content_is_not_silently_discarded() {
    assert!(prepare_request(
        &json!({"model":"test","input":[{"type":"agent_message","content":42}]})
    )
    .is_err());
}
