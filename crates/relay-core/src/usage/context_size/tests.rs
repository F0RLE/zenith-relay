use super::*;
use serde_json::json;

#[test]
fn all_protocols_separate_instructions_history_and_tools_without_mutation() {
    let requests = [
        json!({"instructions":"synthetic policy","input":"hello","tools":[{"type":"function","name":"fixture","parameters":{"type":"object"}}]}),
        json!({"messages":[{"role":"system","content":"synthetic policy"},{"role":"user","content":"hello"}],"tools":[{"type":"function","function":{"name":"fixture","parameters":{"type":"object"}}}]}),
        json!({"system":[{"type":"text","text":"synthetic policy"}],"messages":[{"role":"user","content":"hello"}],"tools":[{"name":"fixture","input_schema":{"type":"object"}}]}),
        json!({"systemInstruction":{"parts":[{"text":"synthetic policy"}]},"contents":[{"role":"user","parts":[{"text":"hello"}]}],"tools":[{"functionDeclarations":[{"name":"fixture","parameters":{"type":"OBJECT"}}]}]}),
    ];
    for request in requests {
        let original = request.clone();
        let size = RequestContextSize::capture(&request);
        assert_eq!(
            size,
            RequestContextSize::capture_with_catalog_bytes(
                &request,
                crate::tool_policy::catalog_stats(&request).bytes,
            )
        );
        assert_eq!(
            size.body_bytes,
            serde_json::to_vec(&request).unwrap().len() as u64
        );
        assert!(size.instruction_bytes > 0);
        assert!(size.conversation_bytes > 0);
        assert!(size.tool_catalog_bytes > 0);
        assert_eq!(request, original);
        let serialized = serde_json::to_string(&size).unwrap();
        assert!(!serialized.contains("synthetic"));
        assert!(!serialized.contains("fixture"));
        assert!(!serialized.contains("hello"));
    }
}

#[test]
fn additional_tools_are_not_counted_again_as_conversation() {
    let request = json!({"input":[{"type":"additional_tools","tools":[{"type":"function","name":"fixture","parameters":{"type":"object"}}]}]});
    let size = RequestContextSize::capture(&request);
    assert!(size.tool_catalog_bytes > 0);
    assert_eq!(size.conversation_bytes, 0);
}

#[test]
fn ordinary_text_has_no_invented_instructions_or_tools() {
    let size = RequestContextSize::capture(&json!({"input":"hello"}));
    assert_eq!(size.instruction_bytes, 0);
    assert_eq!(size.tool_catalog_bytes, 0);
    assert_eq!(size.conversation_bytes, 7);
}

#[test]
fn size_matches_json_for_empty_null_escaped_and_large_inputs() {
    let requests = [
        json!(null),
        json!(["not an object", 123]),
        json!({}),
        json!({"input":[],"messages":null,"instructions":null,"tools":[]}),
        json!({"input":"Привет\n\"🦜\"","odd\"key\\":true,"model":"test"}),
        json!({"input":[{"role":"developer","content":"rules"},{"role":"user","content":"hi"},null],"system":"system","max_output_tokens":1024}),
        json!({"system_instruction":{"parts":[{"text":"rules"}]},"contents":[{"role":"model","parts":[{"text":"answer"}]}]}),
        json!({"messages":[{"role":"user","content":[{"type":"image","source":{"data":"a".repeat(1024 * 1024)}}]}]}),
    ];
    for request in requests {
        assert_eq!(
            RequestContextSize::capture(&request).body_bytes,
            serde_json::to_vec(&request).unwrap().len() as u64
        );
    }
}

#[test]
fn category_sizes_preserve_null_items_and_omit_array_delimiters() {
    let instruction = json!({"role":"developer","content":"rules"});
    let message = json!({"role":"user","content":"hi"});
    let tools = json!([{"type":"function","name":"fixture"}]);
    let request = json!({
        "instructions":null,
        "system":"system",
        "input":[instruction,message,null,{"type":"additional_tools","tools":tools}],
        "contents":[],
    });
    let bytes = crate::tool_policy::serialized_bytes;
    let size = RequestContextSize::capture(&request);
    assert_eq!(
        size.instruction_bytes,
        bytes(&instruction) + bytes(&json!("system"))
    );
    assert_eq!(
        size.conversation_bytes,
        bytes(&message) + bytes(&Value::Null)
    );
    assert_eq!(size.tool_catalog_bytes, bytes(&tools));
}

#[test]
#[ignore = "local context-size benchmark; no provider requests"]
fn large_context_capture_latency() {
    let message = json!({"role":"user","content":"synthetic text\n".repeat(8192)});
    let request = json!({"model":"fixture","input":vec![message; 10],"tools":[]});
    let expected_bytes = serde_json::to_vec(&request).unwrap().len() as u64;
    let started = std::time::Instant::now();
    for _ in 0..100 {
        let sizes = std::hint::black_box(RequestContextSize::capture(&request));
        assert_eq!(sizes.body_bytes, expected_bytes);
    }
    eprintln!(
        "capture 100 x {expected_bytes} bytes: {:?}",
        started.elapsed()
    );
}
