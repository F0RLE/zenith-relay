use super::*;

fn keep_all() -> Value {
    json!({"edits":[{"type":"clear_thinking_20251015","keep":"all"}]})
}

#[test]
fn messages_keep_all_preserves_tool_history_and_existing_continuation_guards() {
    let mut original = input(WireApi::Messages);
    original["tools"] = json!([{"name":"lookup","input_schema":{"type":"object","properties":{"value":{"type":"string"}}}}]);
    original["system"] = json!("Synthetic system instruction.");
    original["messages"] = json!([
        {"role":"user","content":"Original synthetic question."},
        {"role":"assistant","content":[{"type":"text","text":"Original synthetic answer."},{"type":"tool_use","id":"toolu_keep_all","name":"lookup","input":{"value":"synthetic"}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_keep_all","content":"Original synthetic result."}]},
        {"role":"assistant","content":"Retained synthetic conclusion."},
        {"role":"user","content":"Next synthetic question."}
    ]);
    let mut request = original.clone();
    request["context_management"] = keep_all();
    let unchanged = request.clone();
    let baseline = decode::request(WireApi::Messages, &original).unwrap();
    let retained = decode::request(WireApi::Messages, &request).unwrap();
    assert_eq!(retained.messages, baseline.messages);
    assert_eq!(retained.metadata, baseline.metadata);
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        for stream in [false, true] {
            let expected = try_convert(WireApi::Messages, upstream, &original, stream);
            let preserved = try_convert(WireApi::Messages, upstream, &request, stream);
            match (expected, preserved) {
                (Ok(expected), Ok(preserved)) => {
                    assert_eq!(preserved.upstream_body(), expected.upstream_body());
                }
                (Err(expected), Err(preserved)) => {
                    assert_eq!(preserved.code(), expected.code());
                    assert_eq!(preserved.parameter(), expected.parameter());
                }
                _ => panic!("keep-all changed continuation admission: {upstream:?}/{stream}"),
            }
            assert_eq!(request, unchanged);
        }
    }
    let native = prepare(WireApi::Messages, WireApi::Messages, &request, false);
    assert_eq!(native.upstream_body(), &request);
}

#[test]
fn messages_keep_all_leaves_foreign_text_requests_unchanged() {
    let mut baseline = input(WireApi::Messages);
    baseline["messages"] = json!([
        {"role":"user","content":"Original synthetic question."},
        {"role":"assistant","content":"Retained synthetic answer."},
        {"role":"user","content":"Next synthetic question."}
    ]);
    let mut request = baseline.clone();
    request["context_management"] = keep_all();
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        for stream in [false, true] {
            let expected = prepare(WireApi::Messages, upstream, &baseline, stream);
            let retained = prepare(WireApi::Messages, upstream, &request, stream);
            assert_eq!(retained.upstream_body(), expected.upstream_body());
        }
    }
}

#[test]
fn messages_context_clearing_and_unknown_options_are_not_treated_as_keep_all() {
    for control in [
        json!({"edits":[]}),
        json!({"edits":[{"type":"clear_thinking_20251015"}]}),
        json!({"edits":[{"type":"clear_thinking_20251015","keep":{"type":"thinking_turns","value":1}}]}),
        json!({"edits":[{"type":"clear_tool_uses_20250919","keep":"all"}]}),
        json!({"edits":[{"type":"clear_thinking_20251015","keep":"all","synthetic_unknown_option":true}]}),
        json!({"edits":[{"type":"clear_thinking_20251015","keep":"all"}],"synthetic_unknown_option":true}),
        json!({"edits":[{"type":"clear_thinking_20251015","keep":"all"},{"type":"clear_tool_uses_20250919"}]}),
        json!({"edits":[{"type":"compact_20260112"}]}),
        json!([]),
        json!(false),
        json!("all"),
    ] {
        let mut request = input(WireApi::Messages);
        request["context_management"] = control;
        let error = decode::request(WireApi::Messages, &request).unwrap_err();
        assert_eq!(
            error.code(),
            crate::error_codes::ADAPTER_PARAMETER_UNSUPPORTED
        );
        assert_eq!(error.parameter(), Some("context_management"));
        let native = prepare(WireApi::Messages, WireApi::Messages, &request, false);
        assert_eq!(native.upstream_body(), &request);
    }
}

#[test]
fn keep_all_does_not_grant_provenance_to_foreign_thinking_signatures() {
    let mut request = input(WireApi::Messages);
    request["messages"] = json!([
        {"role":"user","content":"Synthetic question."},
        {"role":"assistant","content":[{"type":"thinking","thinking":"Synthetic reasoning.","signature":"synthetic-foreign-signature"}]},
        {"role":"user","content":"Next question."}
    ]);
    let baseline = decode::request(WireApi::Messages, &request).unwrap_err();
    request["context_management"] = keep_all();
    let retained = decode::request(WireApi::Messages, &request).unwrap_err();
    assert_eq!(retained.code(), baseline.code());
    assert_eq!(retained.parameter(), baseline.parameter());
}
