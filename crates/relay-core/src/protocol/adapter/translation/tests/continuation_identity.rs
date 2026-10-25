use super::*;
use crate::AdapterContinuationStore;

#[test]
fn messages_file_tool_chain_restores_state_after_client_reserialization() {
    for streaming in [false, true] {
        let adapter = SourceAdapter::MessagesToResponses;
        let mut store = AdapterContinuationStore::new(8, 60_000);
        let mut request = json!({
            "system":"Synthetic policy.","max_tokens":4096,
            "messages":[{"role":"user","content":"Create and edit a synthetic SVG."}],
            "thinking":{"type":"adaptive"},"output_config":{"effort":"max"},
            "tools":[{"name":"write_file","input_schema":{"type":"object",
                "properties":{"path":{"type":"string"},"count":{"type":"number"}},
                "required":["path","count"]}}]
        });
        for round in 0..=3 {
            let previous = adapter
                .continuation_key(WireApi::Messages, &request, "test")
                .unwrap()
                .map(|key| {
                    assert!(store.get("other-key", &key, "route", round).is_err());
                    assert!(store.get("key", &key, "other-route", round).is_err());
                    store.get("key", &key, "route", round).unwrap()
                });
            assert_eq!(previous.is_some(), round > 0);
            let prepared = adapter
                .prepare_request(AdapterRequestContext {
                    client_wire_api: WireApi::Messages,
                    request: &request,
                    model: "test",
                    stream: streaming,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous,
                    response_scope: "synthetic-files",
                    response_id_seed: &format!("round-{round}"),
                })
                .unwrap();
            assert_eq!(prepared.upstream_body()["reasoning"]["effort"], "max");
            if round > 0 {
                let history = prepared.upstream_body()["input"].as_array().unwrap();
                assert!(history
                    .iter()
                    .any(|item| item["encrypted_content"] == "synthetic-state"));
                assert!(history
                    .iter()
                    .any(|item| item["arguments"] == r#"{"path":"synthetic.svg","count":1.30e2}"#));
            }
            let call_id = format!("call_file_{round}");
            let native = if round == 3 {
                json!({"id":"resp_file_final","status":"completed","output":[
                    {"type":"message","role":"assistant","content":[{"type":"output_text","text":"The SVG is ready."}]}
                ]})
            } else {
                json!({"id":format!("resp_file_{round}"),"status":"completed","output":[
                {"type":"reasoning","id":format!("rs_file_{round}"),"summary":[],"encrypted_content":"synthetic-state"},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"Creating the SVG."}]},
                {"type":"function_call","call_id":call_id,"name":"write_file","arguments":r#"{"path":"synthetic.svg","count":1.30e2}"#}
                ]})
            };
            let completed = if streaming {
                let mut bridge = prepared.into_stream_bridge().unwrap();
                let wire = format!(
                    "data: {}\n\n",
                    json!({"type":"response.completed","response":native})
                );
                for chunk in wire.as_bytes().chunks(7) {
                    bridge.push(chunk);
                }
                bridge.finish();
                bridge.completed().expect("tool turn must complete").clone()
            } else {
                prepared
                    .translate_response_bytes(&serde_json::to_vec(&native).unwrap())
                    .unwrap()
                    .unwrap()
                    .translated_response()
                    .clone()
            };
            if round == 3 {
                assert_eq!(completed.response_body["stop_reason"], "end_turn");
                assert_eq!(
                    completed.response_body["content"],
                    json!([
                        {"type":"text","text":"The SVG is ready."}
                    ])
                );
                break;
            }
            let storage_key = completed
                .continuation
                .storage_key(&completed.response_id)
                .to_owned();
            store.insert(
                "key",
                &storage_key,
                "route",
                completed.continuation.clone(),
                round,
            );
            let mut call = completed.response_body["content"]
                .as_array()
                .unwrap()
                .iter()
                .find(|block| block["type"] == "tool_use")
                .unwrap()
                .clone();
            // JSON.parse/stringify in a client changes numeric spelling and
            // object order; its assembler may split adjacent assistant text.
            call["input"] = json!({"count":130,"path":"synthetic.svg"});
            request["messages"].as_array_mut().unwrap().extend([
                json!({"role":"assistant","content":[{"type":"text","text":"Creating "}]}),
                json!({"role":"assistant","content":[{"type":"text","text":"the SVG."},call]}),
                json!({"role":"user","content":[{"type":"tool_result","tool_use_id":call_id,"content":"Saved."}]}),
            ]);
            let key = adapter
                .continuation_key(WireApi::Messages, &request, "test")
                .unwrap()
                .unwrap();
            assert_eq!(key, storage_key);
            let mut changed = request.clone();
            let messages = changed["messages"].as_array_mut().unwrap();
            let assistant = messages.len() - 2;
            messages[assistant]["content"][1]["input"]["count"] = json!(131);
            let changed_key = adapter
                .continuation_key(WireApi::Messages, &changed, "test")
                .unwrap()
                .unwrap();
            assert_ne!(changed_key, key);
            assert!(store.get("key", &changed_key, "route", round).is_err());
        }
    }
}
