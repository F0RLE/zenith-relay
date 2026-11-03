use super::*;

fn request() -> Value {
    json!({"input":"Find the synthetic record","tools":[{"type":"web_search"},
        {"type":"function","name":"lookup","parameters":{"type":"object"}}],
        "include":["web_search_call.action.sources"]})
}

fn prepared(
    body: &Value,
    stream: bool,
    previous: Option<crate::AdapterContinuationState>,
) -> AdapterResult<crate::PreparedAdapterRequest> {
    SourceAdapter::between(WireApi::Responses, WireApi::Messages)
        .unwrap()
        .prepare_request_with_capabilities(
            AdapterRequestContext {
                client_wire_api: WireApi::Responses,
                request: body,
                model: "test",
                stream,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous,
                response_scope: "hosted-search",
                response_id_seed: "synthetic",
            },
            &crate::model_metadata::ModelCapabilities {
                reasoning: Some(false),
                ..Default::default()
            },
        )
}

fn search_call(id: &str) -> Value {
    json!({"type":"server_tool_use","id":id,"name":"web_search","input":{"query":"synthetic query"},"caller":{"type":"direct"}})
}

fn search_result(id: &str) -> Value {
    json!({"type":"web_search_tool_result","tool_use_id":id,"content":[{
        "type":"web_search_result","url":"https://example.invalid/source","title":"Synthetic source",
        "encrypted_content":"synthetic-opaque-result","page_age":null}]})
}

fn citation() -> Value {
    json!({"type":"web_search_result_location","url":"https://example.invalid/source","title":"Synthetic source",
        "encrypted_index":"synthetic-opaque-index","cited_text":"A source quote, not assistant text"})
}

fn reply(with_client_tool: bool) -> Value {
    let mut blocks = vec![
        search_call("srvtoolu_synthetic"),
        search_result("srvtoolu_synthetic"),
        json!({"type":"text","text":"Найдено 🦀","citations":[citation()]}),
    ];
    if with_client_tool {
        blocks.push(json!({"type":"tool_use","id":"call_lookup","name":"lookup","input":{"id":7}}));
    }
    json!({"id":"msg_synthetic","type":"message","role":"assistant","content":blocks,
        "stop_reason":if with_client_tool {"tool_use"} else {"end_turn"},"usage":{"input_tokens":9,"output_tokens":6}})
}

fn stream_events(body: &Value) -> Vec<Value> {
    let mut events = vec![
        json!({"type":"message_start","message":{"id":"msg_synthetic","content":[],"usage":{"input_tokens":9}}}),
    ];
    for (index, native) in body["content"].as_array().unwrap().iter().enumerate() {
        let mut start = native.clone();
        let mut deltas = Vec::new();
        match native["type"].as_str().unwrap() {
            "server_tool_use" | "tool_use" => {
                start["input"] = json!({});
                let input = native["input"].to_string();
                for fragment in [input[..3].to_owned(), input[3..].to_owned()] {
                    deltas.push(json!({"type":"input_json_delta","partial_json":fragment}));
                }
            }
            "text" => {
                start["text"] = "".into();
                start["citations"] = json!([]);
                deltas.push(json!({"type":"text_delta","text":native["text"]}));
                for citation in native["citations"].as_array().unwrap() {
                    deltas.push(json!({"type":"citations_delta","citation":citation}));
                }
            }
            _ => {}
        }
        events.push(json!({"type":"content_block_start","index":index,"content_block":start}));
        for delta in deltas {
            events.push(json!({"type":"content_block_delta","index":index,"delta":delta}));
        }
        events.push(json!({"type":"content_block_stop","index":index}));
    }
    events.push(json!({"type":"message_delta","delta":{"stop_reason":body["stop_reason"]},"usage":{"output_tokens":6}}));
    events.push(json!({"type":"message_stop"}));
    events
}

fn translate(body: &Value, streaming: bool) -> crate::TranslatedResponse {
    let prepared = prepared(&request(), streaming, None).unwrap();
    if !streaming {
        return prepared
            .translate_response_bytes(&serde_json::to_vec(body).unwrap())
            .unwrap()
            .unwrap()
            .translated_response()
            .clone();
    }
    let mut bridge = prepared.into_stream_bridge().unwrap();
    for event in stream_events(body) {
        for chunk in format!("data: {event}\n\n").as_bytes().chunks(3) {
            bridge.push(chunk);
        }
    }
    bridge.finish();
    let mut wire = String::new();
    while let Some(frame) = bridge.pop_output() {
        wire.push_str(std::str::from_utf8(&frame).unwrap());
    }
    let response = bridge
        .completed()
        .unwrap_or_else(|| panic!("{wire}"))
        .clone();
    let events: Vec<Value> = wire
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    let added: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "response.output_item.added")
        .collect();
    let done: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "response.output_item.done")
        .collect();
    assert_eq!(
        added.len(),
        response.response_body["output"].as_array().unwrap().len()
    );
    assert_eq!(done.len(), added.len());
    for (i, (start, end)) in added.iter().zip(done.iter()).enumerate() {
        assert_eq!(start["output_index"], i);
        assert_eq!(start["item"]["id"], end["item"]["id"]);
        assert_eq!(end["item"], response.response_body["output"][i]);
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "response.web_search_call.completed")
            .count(),
        response.response_body["output"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "web_search_call")
            .count()
    );
    assert!(!wire.contains("synthetic-opaque"));
    response
}

#[test]
fn hosted_web_search_json_and_fragmented_sse_preserve_sources_citations_and_native_replay() {
    for streaming in [false, true] {
        for by_id in [false, true] {
            let native = reply(true);
            let response = translate(&native, streaming);
            let output = response.response_body["output"].as_array().unwrap();
            assert_eq!(output[0]["type"], "web_search_call");
            assert_eq!(output[0]["status"], "completed");
            assert_eq!(output[0]["action"]["query"], "synthetic query");
            assert_eq!(
                output[0]["action"]["sources"][0]["url"],
                "https://example.invalid/source"
            );
            let annotation = &output[1]["content"][0]["annotations"][0];
            assert_eq!(annotation["start_index"], 0);
            assert_eq!(annotation["end_index"], "Найдено 🦀".chars().count());
            assert_eq!(annotation["url"], "https://example.invalid/source");
            let result = json!({"type":"function_call_output","call_id":"call_lookup","output":"synthetic result"});
            let next = if by_id {
                json!({"previous_response_id":response.response_id,"input":[result],"include":["web_search_call.action.sources"]})
            } else {
                let mut history =
                    vec![json!({"role":"user","content":"Find the synthetic record"})];
                history.extend(output.iter().cloned());
                history.push(result);
                json!({"tools":request()["tools"],"input":history})
            };
            let follow = prepared(&next, streaming, Some(response.continuation)).unwrap();
            assert_eq!(
                follow.upstream_body()["messages"][1]["content"],
                native["content"]
            );
            assert!(
                follow.upstream_body()["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["name"] == "web_search")
            );
            let answer = follow
                .translate_response_bytes(&serde_json::to_vec(&output_message()).unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(
                answer.response_body()["output"][0]["content"][0]["text"],
                "Done"
            );
        }
    }
}

fn output_message() -> Value {
    json!({"id":"msg_final","content":[{"type":"text","text":"Done"}],"stop_reason":"end_turn"})
}

#[test]
fn hosted_web_search_maps_supported_controls_without_silently_dropping_others() {
    let mut body = request();
    body["tools"] = json!([{"type":"web_search_2025_08_26","filters":{"allowed_domains":["example.invalid"]},
        "user_location":{"type":"approximate","city":"Boston","country":"US","region":"MA","timezone":"America/New_York"},"external_web_access":true}]);
    body["tool_choice"] = "required".into();
    let mapped = prepared(&body, false, None).unwrap();
    assert_eq!(
        mapped.upstream_body()["tools"][0]["type"],
        "web_search_20250305"
    );
    assert_eq!(
        mapped.upstream_body()["tools"][0]["allowed_domains"],
        json!(["example.invalid"])
    );
    assert_eq!(
        mapped.upstream_body()["tools"][0]["user_location"],
        body["tools"][0]["user_location"]
    );
    assert_eq!(mapped.upstream_body()["tool_choice"]["type"], "any");
    assert!(
        mapped
            .translate_response_bytes(&serde_json::to_vec(&reply(false)).unwrap())
            .is_ok()
    );
    for extra in [
        json!({"external_web_access":false}),
        json!({"search_context_size":"high"}),
        json!({"filters":{"blocked_domains":["example.invalid"]}}),
    ] {
        let mut invalid = request();
        invalid["tools"][0]
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(prepared(&invalid, false, None).is_err(), "{invalid}");
    }
    for tools in [
        json!([{"type":"web_search"},{"type":"web_search"}]),
        json!([{"type":"web_search"},{"type":"function","name":"web_search","parameters":{"type":"object"}}]),
        json!([{"type":"namespace","name":"nested","tools":[{"type":"web_search"}]}]),
    ] {
        body["tools"] = tools;
        assert!(prepared(&body, false, None).is_err());
    }
}

#[test]
fn hosted_web_search_rejects_unpaired_failed_foreign_and_unfinished_results() {
    let valid = reply(false);
    let mut cases = Vec::new();
    let mut bad = valid.clone();
    bad["content"].as_array_mut().unwrap().remove(1);
    cases.push(bad);
    let mut bad = valid.clone();
    bad["content"].as_array_mut().unwrap().remove(0);
    cases.push(bad);
    let mut bad = valid.clone();
    bad["content"]
        .as_array_mut()
        .unwrap()
        .insert(1, search_call("srvtoolu_synthetic"));
    cases.push(bad);
    let mut bad = valid.clone();
    bad["content"]
        .as_array_mut()
        .unwrap()
        .insert(2, search_result("srvtoolu_synthetic"));
    cases.push(bad);
    let mut bad = valid.clone();
    bad["content"][1]["content"] =
        json!({"type":"web_search_tool_result_error","error_code":"unavailable"});
    cases.push(bad);
    let mut bad = valid.clone();
    bad["content"][0]["caller"] =
        json!({"type":"code_execution_20250825","tool_id":"srvtoolu_foreign"});
    cases.push(bad);
    let mut bad = valid.clone();
    bad["content"][0]["name"] = "web_fetch".into();
    cases.push(bad);
    let mut bad = valid.clone();
    bad["stop_reason"] = "pause_turn".into();
    cases.push(bad);
    for bad in cases {
        assert!(
            prepared(&request(), false, None)
                .unwrap()
                .translate_response_bytes(&serde_json::to_vec(&bad).unwrap())
                .is_err(),
            "{bad}"
        );
        let mut stream = prepared(&request(), true, None)
            .unwrap()
            .into_stream_bridge()
            .unwrap();
        for event in stream_events(&bad) {
            stream.push(format!("data: {event}\n\n").as_bytes());
        }
        stream.finish();
        assert!(stream.completed().is_none(), "{bad}");
    }
    let mut disabled = request();
    disabled["tool_choice"] = "none".into();
    assert!(
        prepared(&disabled, false, None)
            .unwrap()
            .translate_response_bytes(&serde_json::to_vec(&valid).unwrap())
            .is_err()
    );
    let response = translate(&valid, false);
    let full = json!({"tools":request()["tools"],"input":[{"role":"user","content":"Find the synthetic record"},response.response_body["output"][0],response.response_body["output"][1],{"role":"user","content":"Continue"}]});
    assert_eq!(
        prepared(&full, false, None).unwrap_err().code(),
        "adapter_continuation_missing"
    );
    let mut forged = full.clone();
    forged["input"][1]["action"]["query"] = "forged query".into();
    assert!(prepared(&forged, false, Some(response.continuation.clone())).is_err());
    let mut forged = full;
    forged["input"][2]["content"][0]["annotations"][0]["url"] =
        "https://example.invalid/forged".into();
    assert!(prepared(&forged, false, Some(response.continuation)).is_err());
}

#[test]
fn hosted_web_search_multiple_calls_retain_thinking_and_empty_results() {
    let mut native = reply(false);
    let blocks = native["content"].as_array_mut().unwrap();
    blocks.insert(0, json!({"type":"thinking","thinking":"Synthetic plan","signature":"synthetic-opaque-signature"}));
    blocks.insert(3, search_call("srvtoolu_second"));
    blocks.insert(
        4,
        json!({"type":"web_search_tool_result","tool_use_id":"srvtoolu_second","content":[]}),
    );
    for streaming in [false, true] {
        let response = translate(&native, streaming);
        assert_eq!(
            response.response_body["output"][2]["action"]["sources"],
            json!([])
        );
        let next = json!({"previous_response_id":response.response_id,"input":"Continue"});
        let follow = prepared(&next, false, Some(response.continuation)).unwrap();
        assert_eq!(
            follow.upstream_body()["messages"][1]["content"],
            native["content"]
        );
    }
}
