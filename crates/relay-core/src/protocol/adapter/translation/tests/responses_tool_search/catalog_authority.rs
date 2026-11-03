use super::*;

fn namespace(name: &str, tool: &str) -> Value {
    json!({"type":"namespace","name":name,"tools":[{
        "type":"function","name":tool,"parameters":{"type":"object"}
    }]})
}

fn first_call(upstream: WireApi) -> (Value, String, crate::TranslatedResponse) {
    let original = json!({"input":"Synthetic lookup","tools":[namespace("records", "lookup")]});
    let prepared = prepare_search(upstream, &original, false, None, "first");
    let alias = callable(prepared.upstream_body(), upstream);
    let response = complete_call(
        prepared,
        upstream,
        false,
        "call_original",
        &alias,
        &json!({}),
        false,
    );
    (original, alias, response)
}

#[test]
fn current_catalog_revokes_historical_tools_across_protocols_and_continuations() {
    for upstream in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        let (original, alias, first) = first_call(upstream);
        for declaration in ["additional_tools", "tool_search_output"] {
            for roots in [json!([]), json!([namespace("records", "retained")])] {
                for streaming in [false, true] {
                    let mut request = json!({"tools": roots, "input":[
                        {"type":declaration,"tools":original["tools"]},
                        {"role":"user","content":"Synthetic lookup"},
                        first.response_body["output"][0],
                        {"type":"function_call_output","call_id":"call_original","output":"Synthetic result"},
                        {"role":"user","content":"Continue with the current catalog"}
                    ]});
                    if declaration == "tool_search_output" {
                        request["input"][0]["execution"] = json!("client");
                        request["input"][0]["call_id"] = json!("call_discovery");
                        request["input"].as_array_mut().unwrap().insert(
                            0,
                            json!({
                                "type":"tool_search_call","execution":"client",
                                "call_id":"call_discovery","arguments":{"goal":"lookup"}
                            }),
                        );
                    }
                    let prepared = prepare_search(upstream, &request, streaming, None, "current");
                    let decoded = decode::request(upstream, prepared.upstream_body()).unwrap();
                    assert_eq!(decoded.tools.len(), roots.as_array().unwrap().len());
                    assert!(!decoded.tools.iter().any(|tool| tool.name == alias));
                    assert!(decoded.messages.iter().flat_map(|message| &message.blocks).any(|block| {
                        matches!(block, Block::ToolCall { id, name, .. } if id == "call_original" && name == &alias)
                    }));
                    assert!(decoded.messages.iter().flat_map(|message| &message.blocks).any(|block| {
                        matches!(block, Block::ToolResult { id, .. } if id == "call_original")
                    }));
                    if streaming {
                        let mut bridge = prepared.into_stream_bridge().unwrap();
                        for event in call_events(upstream, "call_revoked", &alias, &json!({})) {
                            bridge.push(format!("data: {event}\n\n").as_bytes());
                        }
                        if upstream == WireApi::ChatCompletions {
                            bridge.push(b"data: [DONE]\n\n");
                        }
                        bridge.finish();
                        assert!(bridge.completed().is_none());
                    } else {
                        let reply = call_reply(upstream, "call_revoked", &alias, &json!({}));
                        assert!(
                            prepared
                                .translate_response_bytes(&serde_json::to_vec(&reply).unwrap())
                                .is_err()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn omitted_catalog_preserves_incremental_declarations_but_current_schemas_win() {
    for upstream in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        let historical = namespace("records", "lookup");
        let mut current = historical.clone();
        current["tools"][0]["parameters"]["description"] = json!("synthetic-current-contract");
        let mut request = json!({"input":[
            {"type":"additional_tools","tools":[historical]},
            {"role":"user","content":"Synthetic continuation"}
        ]});
        let incremental = prepare_search(upstream, &request, false, None, "incremental");
        let alias = callable(incremental.upstream_body(), upstream);
        request["tools"] = json!([current]);
        let prepared = prepare_search(upstream, &request, false, None, "updated");
        assert!(
            prepared
                .upstream_body()
                .to_string()
                .contains("synthetic-current-contract")
        );
        let reply = complete_call(
            prepared,
            upstream,
            false,
            "call_current",
            &alias,
            &json!({}),
            false,
        );
        assert_eq!(reply.response_body["output"][0]["name"], "lookup");
        assert_eq!(reply.response_body["output"][0]["namespace"], "records");
    }
}

#[test]
fn explicit_tool_choice_cannot_restore_an_omitted_namespace_child() {
    for root in [json!([]), json!([namespace("records", "retained")])] {
        let request = json!({"tools":root,"input":[
            {"type":"additional_tools","tools":[namespace("records", "lookup")]},
            {"role":"user","content":"Synthetic continuation"}
        ],"tool_choice":{"type":"function","namespace":"records","name":"lookup"}});
        assert!(decode::request(WireApi::Responses, &request).is_err());
    }
}

#[test]
fn empty_catalog_survives_response_id_continuations() {
    for upstream in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        let (_, _, first) = first_call(upstream);
        let request = json!({"previous_response_id":first.response_id,"tools":[],"input":[
            {"type":"function_call_output","call_id":"call_original","output":"Synthetic result"}
        ]});
        let prepared = prepare_search(upstream, &request, false, Some(first.continuation), "empty");
        assert!(
            decode::request(upstream, prepared.upstream_body())
                .unwrap()
                .tools
                .is_empty()
        );
        let answer = prepared
            .translate_response_bytes(&serde_json::to_vec(&output(upstream)).unwrap())
            .unwrap()
            .unwrap()
            .translated_response()
            .clone();
        let request = json!({"previous_response_id":answer.response_id,"input":"Continue"});
        let prepared = prepare_search(upstream, &request, false, Some(answer.continuation), "next");
        assert!(
            decode::request(upstream, prepared.upstream_body())
                .unwrap()
                .tools
                .is_empty()
        );
    }
}
