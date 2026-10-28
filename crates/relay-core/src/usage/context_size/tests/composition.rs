use super::*;

fn partition(request: &Value) -> ConversationSizeBreakdown {
    let before = request.clone();
    let measured = RequestContextSize::capture(request);
    assert_eq!(
        measured.body_bytes,
        serde_json::to_vec(request).unwrap().len() as u64
    );
    assert_eq!(request, &before);
    let parts = measured.conversation.unwrap();
    assert_eq!(
        measured.conversation_bytes,
        parts.message_bytes
            + parts.tool_call_bytes
            + parts.tool_result_bytes
            + parts.reasoning_bytes
            + parts.attachment_bytes
            + parts.other_bytes,
    );
    parts
}

#[test]
fn response_history_categories_are_disjoint_and_private() {
    let call = json!({"type":"function_call","name":"private_name","arguments":"{\"content\":\"private argument\"}"});
    let result = json!({"type":"function_call_output","output":"private result"});
    let reasoning = json!({"type":"reasoning","encrypted_content":"opaque private ciphertext"});
    let image = json!({"type":"input_image","image_url":"data:image/png;base64,synthetic"});
    let request = json!({"instructions":"private policy","input":[
        {"role":"developer","content":"private policy"},
        {"role":"user","content":[{"type":"input_text","text":"Привет\n\"🦜\""},image]},
        call,result,reasoning,null,{"type":"future_unknown","value":"private unknown"},
        {"type":"additional_tools","tools":[{"type":"function","name":"private_tool"}]}
    ]});
    let parts = partition(&request);
    let bytes = crate::tool_policy::serialized_bytes;
    assert_eq!(parts.tool_call_bytes, bytes(&call));
    assert_eq!(parts.tool_result_bytes, bytes(&result));
    assert_eq!(parts.reasoning_bytes, bytes(&reasoning));
    assert_eq!(parts.attachment_bytes, bytes(&image));
    assert_eq!(parts.attachment_count, 1);
    assert_eq!(parts.item_count, 6);
    let encoded = serde_json::to_string(&parts).unwrap();
    for private in ["private", "synthetic", "opaque", "Привет", "data:"] {
        assert!(!encoded.contains(private));
    }
}

#[test]
fn mixed_provider_blocks_keep_text_in_its_parent_category() {
    let attachment = json!({"type":"image","source":{"data":"synthetic".repeat(4096)}});
    let result = json!({"type":"tool_result","content":["result text",{"type":"text","text":"more"},attachment]});
    let parts = partition(&json!({"messages":[{"role":"user","content":[result]}]}));
    let bytes = crate::tool_policy::serialized_bytes;
    assert_eq!(parts.tool_result_bytes, bytes(&result) - bytes(&attachment));
    assert_eq!(parts.attachment_bytes, bytes(&attachment));
    assert!(parts.message_bytes > 0);
    assert_eq!(parts.attachment_count, 1);
    for request in [
        json!({"messages":[{"role":"assistant","tool_calls":[{"type":"function","function":{"arguments":"{}"}}]},{"role":"tool","content":"answer"}]}),
        json!({"messages":[{"role":"assistant","function_call":{"arguments":"{}"}},{"role":"function","content":"answer"}]}),
        json!({"messages":[{"role":"assistant","content":[{"type":"tool_use","input":{"type":"image","data":"not an attachment"}}]},{"role":"user","content":[{"type":"tool_result","content":"answer"}]}]}),
        json!({"contents":[{"role":"model","parts":[{"functionCall":{"args":{}}},{"functionResponse":{"response":{}}}]}]}),
    ] {
        let parts = partition(&request);
        assert!(parts.tool_call_bytes > 0);
        assert!(parts.tool_result_bytes > 0);
        assert_eq!(parts.attachment_count, 0);
    }
}

#[test]
fn typed_media_and_reasoning_parts_do_not_require_payload_inspection() {
    let request = json!({"contents":[{"role":"model","parts":[
        {"inlineData":{"data":"opaque media"}}, {"fileData":{"fileUri":"private reference"}},
        {"text":"private thought","thought":true}, {"text":"ordinary"}
    ]}]});
    let parts = partition(&request);
    assert_eq!(parts.attachment_count, 2);
    assert!(parts.attachment_bytes > 0 && parts.reasoning_bytes > 0 && parts.message_bytes > 0);
}

#[test]
fn legacy_counters_do_not_invent_a_breakdown() {
    let legacy =
        json!({"bodyBytes":100,"instructionBytes":10,"conversationBytes":70,"toolCatalogBytes":5});
    let restored: RequestContextSize = serde_json::from_value(legacy.clone()).unwrap();
    assert!(restored.conversation.is_none());
    assert_eq!(serde_json::to_value(restored).unwrap(), legacy);
    for request in [
        json!({}),
        json!({"input":null}),
        json!({"input":[]}),
        json!({"input":""}),
    ] {
        partition(&request);
    }
}

#[test]
fn nested_content_is_bounded_and_preserves_exact_sizes() {
    let mut item = json!({"type":"input_file","file_id":"private reference"});
    for _ in 0..40 {
        item = json!({"type":"tool_result","content":[item]});
    }
    let parts = partition(&json!({"input":[item]}));
    assert!(parts.tool_result_bytes > 0);
    assert_eq!(parts.attachment_count, 0); // opaque beyond the classification bound
}
