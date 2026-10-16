use super::*;

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
