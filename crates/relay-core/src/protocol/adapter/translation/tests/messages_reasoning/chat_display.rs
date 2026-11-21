use super::*;

#[test]
fn omitted_chat_thinking_stays_in_scoped_history_after_json_and_fragmented_sse() {
    let thought = "Retain this synthetic private plan.";
    let tool = json!({"id":"call_edit","type":"function","function":{"name":"edit_file","arguments":"{\"path\":\"synthetic.txt\"}"}});
    let usage = json!({"prompt_tokens":10,"completion_tokens":8,"total_tokens":18});
    for field in ["reasoning_content", "reasoning"] {
        let mut native = json!({"id":"chat_synthetic","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[tool]},"finish_reason":"tool_calls"}],"usage":usage});
        native["choices"][0]["message"][field] = json!(thought);
        let mut initial = json!({"role":"assistant"});
        initial[field] = json!(thought);
        let frames = [
            json!({"id":"chat_synthetic","choices":[{"index":0,"delta":initial,"finish_reason":null}]}),
            json!({"id":"chat_synthetic","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_edit","type":"function","function":{"name":"edit_file","arguments":"{\"path\":"}}]},"finish_reason":null}]}),
            json!({"id":"chat_synthetic","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"synthetic.txt\"}"}}]},"finish_reason":null}]}),
            json!({"id":"chat_synthetic","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":usage}),
        ];
        let mut wire = frames
            .iter()
            .map(|event| format!("data: {event}\n\n"))
            .collect::<String>();
        wire.push_str("data: [DONE]\n\n");
        for chunks in [None, Some(1), Some(7), Some(wire.len())] {
            let mut request = request();
            request["thinking"]["display"] = json!("omitted");
            let prepared = prepare(
                WireApi::Messages,
                WireApi::ChatCompletions,
                &request,
                chunks.is_some(),
            );
            let completed = if let Some(chunks) = chunks {
                let mut stream = prepared.into_stream_bridge().unwrap();
                for chunk in wire.as_bytes().chunks(chunks) {
                    stream.push(chunk);
                }
                stream.finish();
                let visible = String::from_utf8(
                    std::iter::from_fn(|| stream.pop_output())
                        .flatten()
                        .collect(),
                )
                .unwrap();
                assert!(!visible.contains(thought));
                stream.completed().unwrap().clone()
            } else {
                prepared
                    .translate_response_bytes(&serde_json::to_vec(&native).unwrap())
                    .unwrap()
                    .unwrap()
                    .translated_response()
                    .clone()
            };
            assert_eq!(
                completed.response_body["content"],
                json!([{"type":"tool_use","id":"call_edit","name":"edit_file","input":{"path":"synthetic.txt"}}])
            );
            assert_eq!(completed.response_body["usage"]["input_tokens"], 10);
            assert_eq!(completed.response_body["usage"]["output_tokens"], 8);
            let retained = completed.continuation.portable_history.as_ref().unwrap();
            assert!(
                retained
                    .last()
                    .unwrap()
                    .blocks
                    .iter()
                    .any(|block| matches!(block, Block::Reasoning(text) if text == thought))
            );
            request["messages"].as_array_mut().unwrap().extend([
                json!({"role":"assistant","content":completed.response_body["content"]}),
                json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call_edit","content":"updated"}]}),
            ]);
            assert_eq!(
                continuation::request_key(WireApi::Messages, &request, "test").unwrap(),
                completed.continuation.history_key
            );
            let next = SourceAdapter::MessagesToChatCompletions
                .prepare_request(AdapterRequestContext {
                    client_wire_api: WireApi::Messages,
                    request: &request,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: Some(completed.continuation),
                    response_scope: "source-test",
                    response_id_seed: "next",
                })
                .unwrap();
            let history = next.upstream_body()["messages"].as_array().unwrap();
            assert_eq!(history[1]["reasoning_content"], thought);
            assert_eq!(history[1]["tool_calls"], json!([tool]));
            assert_eq!(history[2]["role"], "tool");
            assert_eq!(history[2]["tool_call_id"], "call_edit");
            assert_eq!(history[2]["content"], "updated");
        }
    }
}
