use super::*;
use crate::{AdapterRequestContext, CacheWriteTtl, MessagesReasoningMode, SourceAdapter, WireApi};

#[test]
fn messages_write_svg_and_empty_signature_tool_result_preserve_the_conversation() {
    let request = json!({"max_tokens":4096,"messages":[{"role":"user","content":"Create a synthetic SVG."}],
        "thinking":{"type":"adaptive"},"output_config":{"effort":"max"},
        "tools":[{"name":"Write","input_schema":{"type":"object","properties":{"file_path":{"type":"string"},"content":{"type":"string"}},"required":["file_path","content"],"additionalProperties":false}}]});
    let arguments = json!({"file_path":"synthetic.svg","content":"<svg xmlns=\"http://www.w3.org/2000/svg\">\n<text>test \\ &amp; value</text>\n</svg>"});
    for incremental in [false, true] {
        let prepared = SourceAdapter::MessagesToResponses
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request: &request,
                model: "test",
                stream: true,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "synthetic-bps",
                response_id_seed: "synthetic-write",
            })
            .unwrap();
        let mut normalizer = Translator::new(prepared.upstream_body().clone());
        let mut bridge = prepared.into_stream_bridge().unwrap();
        let mut write = call("Write");
        write["arguments"] = json!({"references":["Write"],"code":arguments.to_string()})
            .to_string()
            .into();
        let native = completed(json!([
            {"type":"reasoning","id":"rs_write","summary":[{"type":"summary_text","text":"Plan the file."},{"type":"summary_text","text":"Preserve SVG escaping."}],"encrypted_content":"synthetic-native-state"},
            {"type":"message","id":"msg_write","role":"assistant","status":"completed","content":[{"type":"output_text","text":"Creating the SVG.","annotations":[]}]},
            write
        ]));
        if incremental {
            emit_unseen_output(&native["response"], &HashSet::new(), |event| {
                normalizer.push(&frame(event));
            });
        }
        normalizer.push(&frame(native.clone()));
        while let Some(bytes) = normalizer.output.pop_front() {
            for chunk in bytes.chunks(7) {
                bridge.push(chunk);
            }
        }
        bridge.finish();
        let wire = String::from_utf8(
            std::iter::from_fn(|| bridge.pop_output())
                .flatten()
                .collect(),
        )
        .unwrap();
        assert!(!wire.contains("event: error"), "{wire}");
        assert_eq!(wire.matches("event: message_stop").count(), 1);
        assert_eq!(wire.matches("\"type\":\"tool_use\"").count(), 1);
        let completed = bridge
            .completed()
            .expect("Write must complete the first turn");
        assert_eq!(completed.response_body["content"][3]["input"], arguments);
        let mut content = completed.response_body["content"].clone();
        for part in content.as_array_mut().unwrap() {
            if part["type"] == "thinking" {
                part["signature"] = json!("");
            }
        }
        let mut next = request.clone();
        next["messages"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":content}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"File created successfully."}]}),
        ]);
        let continued = SourceAdapter::MessagesToResponses
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request: &next,
                model: "test",
                stream: false,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: Some(completed.continuation.clone()),
                response_scope: "synthetic-bps",
                response_id_seed: "synthetic-write-next",
            })
            .unwrap();
        assert_eq!(
            continued.upstream_body()["input"][1],
            native["response"]["output"][0]
        );
        assert_eq!(continued.upstream_body()["input"][4]["call_id"], "call_1");
        assert!(!continued.upstream_body().to_string().contains("signature"));
        let answer = continued
            .translate_response_bytes(
                &serde_json::to_vec(&json!({
                    "id":"resp_write_final","status":"completed","output":[
                        {"type":"message","role":"assistant","content":[
                            {"type":"output_text","text":"The SVG is ready."}
                        ]}
                    ]
                }))
                .unwrap(),
            )
            .unwrap()
            .unwrap();
        let answer = &answer.translated_response().response_body;
        assert_eq!(answer["stop_reason"], "end_turn");
        assert_eq!(answer["content"][0]["text"], "The SVG is ready.");
    }
}

#[test]
fn reasoning_text_and_atomic_tool_complete_through_messages_bridge() {
    let request = json!({"max_tokens":4096,"messages":[{"role":"user","content":"Inspect a synthetic file."}],
        "thinking":{"type":"adaptive"},"output_config":{"effort":"high"},
        "tools":[{"name":"inspect","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}]});
    for incremental in [false, true] {
        let prepared = SourceAdapter::MessagesToResponses
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request: &request,
                model: "test",
                stream: true,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "synthetic-bps",
                response_id_seed: "synthetic-request",
            })
            .unwrap();
        let mut normalizer = Translator::new(prepared.upstream_body().clone());
        let mut bridge = prepared.into_stream_bridge().unwrap();
        let native = completed(json!([
            {"type":"reasoning","id":"rs_synthetic","summary":[{"type":"summary_text","text":"Plan the inspection."}],"encrypted_content":"synthetic-opaque-state"},
            {"type":"message","id":"msg_synthetic","role":"assistant","status":"completed","content":[{"type":"output_text","text":"Inspecting.","annotations":[]}]},
            call("inspect")
        ]));
        if incremental {
            emit_unseen_output(&native["response"], &HashSet::new(), |event| {
                normalizer.push(&frame(event));
            });
        }
        normalizer.push(&frame(native.clone()));
        while let Some(bytes) = normalizer.output.pop_front() {
            for chunk in bytes.chunks(13) {
                bridge.push(chunk);
            }
        }
        bridge.finish();
        let wire = String::from_utf8(
            std::iter::from_fn(|| bridge.pop_output())
                .flatten()
                .collect(),
        )
        .unwrap();
        assert!(!wire.contains("event: error"), "{wire}");
        assert_eq!(wire.matches("event: message_stop").count(), 1);
        let completed = bridge
            .completed()
            .expect("native terminal must complete the Messages turn");
        assert_eq!(completed.response_body["stop_reason"], "tool_use");
        assert_eq!(completed.response_body["content"][2]["id"], "call_1");
        let mut next = request.clone();
        next["messages"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":completed.response_body["content"]}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"inspected"}]}),
        ]);
        next["messages"][1]["content"][0]["signature"] = json!("");
        let continued = SourceAdapter::MessagesToResponses
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request: &next,
                model: "test",
                stream: false,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: Some(completed.continuation.clone()),
                response_scope: "synthetic-bps",
                response_id_seed: "synthetic-next",
            })
            .unwrap();
        assert_eq!(
            continued.upstream_body()["input"][1],
            native["response"]["output"][0]
        );
        assert_eq!(continued.upstream_body()["input"][4]["call_id"], "call_1");
        let answer = continued.translate_response_bytes(&serde_json::to_vec(&json!({
            "id":"resp_final","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done."}]}]
        })).unwrap()).unwrap().unwrap();
        assert_eq!(
            answer.translated_response().response_body["stop_reason"],
            "end_turn"
        );
    }
}
