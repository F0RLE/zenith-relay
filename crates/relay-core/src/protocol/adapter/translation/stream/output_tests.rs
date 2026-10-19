use super::{Block, TranslationStream, WireApi};
use serde_json::json;

fn stream(client: WireApi, upstream: WireApi) -> Box<TranslationStream> {
    let request = match client {
        WireApi::Messages => {
            json!({"model":"test","max_tokens":64,"messages":[{"role":"user","content":"Hello"}]})
        }
        WireApi::ChatCompletions => {
            json!({"model":"test","messages":[{"role":"user","content":"Hello"}]})
        }
        WireApi::Gemini => {
            json!({"contents":[{"role":"user","parts":[{"text":"Hello"}]}]})
        }
        WireApi::Responses => json!({"model":"test","input":"Hello"}),
    };
    let prepared = crate::SourceAdapter::between(client, upstream)
        .unwrap()
        .prepare_request(crate::AdapterRequestContext {
            client_wire_api: client,
            request: &request,
            model: "test",
            stream: true,
            reasoning_mode: crate::MessagesReasoningMode::Disabled,
            cache_write_ttl: crate::CacheWriteTtl::Provider,
            previous: None,
            response_scope: "synthetic",
            response_id_seed: "synthetic",
        })
        .unwrap();
    let crate::AdapterStreamBridge::Translated(stream) = prepared.into_stream_bridge().unwrap();
    stream
}

#[test]
fn completed_streams_move_existing_history_into_their_continuations() {
    for client in WireApi::ALL {
        for upstream in WireApi::ALL
            .into_iter()
            .filter(|upstream| *upstream != client)
        {
            let mut stream = stream(client, upstream);
            let Block::Text(text) = &mut stream.translation_request.history[0].blocks[0] else {
                unreachable!()
            };
            text.reserve(64 * 1024);
            let pointer = text.as_ptr();
            let events = match upstream {
                WireApi::Responses => vec![
                    json!({"type":"response.completed","response":{"id":"resp_test","status":"completed","output":[]}}),
                ],
                WireApi::ChatCompletions => vec![
                    json!({"choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":"stop"}]}),
                ],
                WireApi::Messages => vec![
                    json!({"type":"message_start","message":{"id":"msg_test","content":[],"usage":{"input_tokens":3}}}),
                    json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":0}}),
                    json!({"type":"message_stop"}),
                ],
                WireApi::Gemini => vec![
                    json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"Hi"}]},"finishReason":"STOP"}]}),
                ],
            };
            for event in events {
                stream.push(format!("data: {event}\n\n").as_bytes());
            }
            if upstream == WireApi::ChatCompletions {
                stream.push(b"data: [DONE]\n\n");
            }
            stream.finish();
            let completed = stream
                .completed()
                .unwrap_or_else(|| panic!("{client:?} -> {upstream:?}"));
            let Block::Text(text) =
                &completed.continuation.portable_history.as_ref().unwrap()[0].blocks[0]
            else {
                unreachable!()
            };
            assert_eq!(text, "Hello");
            assert_eq!(text.as_ptr(), pointer, "{client:?} -> {upstream:?}");
            assert!(stream.translation_request.history.is_empty());
        }
    }
}

#[test]
fn gemini_native_history_keeps_exact_signature_and_thought_boundaries() {
    let parts = [
        json!({"text":"prefix"}),
        json!({"text":" fragment"}),
        json!({"text":"signed","thoughtSignature":"synthetic-signature-1"}),
        json!({"text":"after"}),
        json!({"text":" signature"}),
        json!({"thoughtSignature":"synthetic-signature-2"}),
        json!({"text":"tail"}),
        json!({"text":" fragment"}),
        json!({"text":"thought","thought":true}),
        json!({"text":" fragment","thought":true}),
        json!({"text":"signed thought","thought":true,"thoughtSignature":"synthetic-signature-3"}),
        json!({"text":"final thought","thought":true}),
        json!({"text":"answer"}),
    ];
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
    ] {
        let mut stream = stream(client, WireApi::Gemini);
        for part in &parts {
            let event =
                json!({"candidates":[{"index":0,"content":{"role":"model","parts":[part]}}]});
            stream.push(format!("data: {event}\n\n").as_bytes());
            assert!(!stream.is_terminal());
            while stream.pop_output().is_some() {}
        }
        assert_eq!(stream.decoded_response.native_output, parts);
        stream.push(b"data: {\"candidates\":[{\"index\":0,\"finishReason\":\"STOP\"}]}\n\n");
        stream.finish();
        assert_eq!(
            stream.completed().unwrap().continuation.native_turns[&1],
            parts
        );
    }
}

#[test]
fn unchanged_text_snapshots_are_not_reallocated_on_later_deltas() {
    let mut stream = stream(WireApi::Responses, WireApi::ChatCompletions);
    stream.decoded_response.blocks = vec![
        Block::Text("x".repeat(64 * 1024)),
        Block::Text("hello".into()),
    ];
    stream.emit_changes(false).unwrap();
    while stream.pop_output().is_some() {}
    let Block::Text(first) = &stream.emitted[0] else {
        unreachable!()
    };
    let first_pointer = first.as_ptr();
    stream.decoded_response.blocks[1] = Block::Text("hello world".into());
    stream.emit_changes(false).unwrap();
    let Block::Text(first) = &stream.emitted[0] else {
        unreachable!()
    };
    assert_eq!(first_pointer, first.as_ptr());
    let frames = std::iter::from_fn(|| stream.pop_output())
        .flatten()
        .collect::<Vec<_>>();
    let output = String::from_utf8(frames).unwrap();
    assert!(output.contains(" world"));
    assert!(!output.contains("xxxx"));
}

#[test]
fn unchanged_messages_blocks_still_close_and_closed_mutations_fail() {
    let mut stream = stream(WireApi::Messages, WireApi::ChatCompletions);
    stream.decoded_response.blocks = vec![Block::Text("hello".into())];
    stream.emit_changes(false).unwrap();
    stream.closed.insert(0);
    stream.emit_changes(false).unwrap();
    assert_eq!(stream.finished_message_blocks, 1);
    stream.emit_changes(false).unwrap();
    stream.decoded_response.blocks[0] = Block::Text("changed".into());
    assert!(stream.emit_changes(false).is_err());
}

#[test]
fn responses_done_snapshots_append_only_missing_text_and_reuse_its_buffer() {
    for client in [WireApi::Messages, WireApi::ChatCompletions, WireApi::Gemini] {
        let mut stream = stream(client, WireApi::Responses);
        let prefix = "synthetic\n\u{1f600}\0";
        let final_text = format!("{prefix} suffix");
        for event in [
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_synthetic","role":"assistant","status":"in_progress","content":[]}}),
            json!({"type":"response.content_part.added","output_index":0,"item_id":"msg_synthetic","content_index":0,"part":{"type":"output_text","text":prefix,"annotations":[]}}),
        ] {
            stream.push(format!("data: {event}\n\n").as_bytes());
        }
        let Block::Text(text) = &mut stream.decoded_response.blocks[0] else {
            unreachable!()
        };
        text.reserve(64);
        let pointer = text.as_ptr();
        let part = json!({"type":"output_text","text":final_text,"annotations":[]});
        for event in [
            json!({"type":"response.output_text.done","output_index":0,"item_id":"msg_synthetic","content_index":0,"text":final_text}),
            json!({"type":"response.content_part.done","output_index":0,"item_id":"msg_synthetic","content_index":0,"part":part}),
        ] {
            for fragment in format!("data: {event}\n\n").as_bytes().chunks(3) {
                stream.push(fragment);
            }
            let Block::Text(text) = &stream.decoded_response.blocks[0] else {
                unreachable!()
            };
            assert_eq!(text, &final_text);
            assert_eq!(text.as_ptr(), pointer);
        }
        let terminal = json!({"type":"response.completed","response":{"id":"resp_synthetic","status":"completed","output":[{"type":"message","id":"msg_synthetic","role":"assistant","status":"completed","content":[part]}]}});
        stream.push(format!("data: {terminal}\n\n").as_bytes());
        assert!(stream.completed().is_some());
    }
}

#[test]
fn messages_native_snapshots_finalize_once_and_keep_fragmented_signatures() {
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let mut stream = stream(client, WireApi::Messages);
        stream.push(b"data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_synthetic\",\"usage\":{\"input_tokens\":7}}}\n\n");
        for (index, kind, field, delta_kind) in [
            (0, "thinking", "thinking", "thinking_delta"),
            (1, "text", "text", "text_delta"),
        ] {
            let start = json!({"type":"content_block_start","index":index,"content_block":{"type":kind,field:"prefix"}});
            stream.push(format!("data: {start}\n\n").as_bytes());
            let delta = json!({"type":"content_block_delta","index":index,"delta":{"type":delta_kind,field:" fragment"}});
            for _ in 0..64 {
                for fragment in format!("data: {delta}\n\n").as_bytes().chunks(3) {
                    stream.push(fragment);
                }
                while stream.pop_output().is_some() {}
                assert_eq!(
                    stream.decoded_response.native_output[index][field],
                    "prefix"
                );
            }
            if kind == "thinking" {
                for signature in ["synthetic-", "opaque-", "signature"] {
                    let delta = json!({"type":"content_block_delta","index":index,"delta":{"type":"signature_delta","signature":signature}});
                    stream.push(format!("data: {delta}\n\n").as_bytes());
                }
            }
            let stop = json!({"type":"content_block_stop","index":index});
            stream.push(format!("data: {stop}\n\n").as_bytes());
            assert_eq!(
                stream.decoded_response.native_output[index][field],
                format!("prefix{}", " fragment".repeat(64))
            );
        }
        stream.push(b"data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":9}}\n\ndata: {\"type\":\"message_stop\"}\n\n");
        let completed = stream
            .completed()
            .expect("signed Messages stream completes");
        let native = completed.continuation.native_turns.values().last().unwrap();
        assert_eq!(native[0]["signature"], "synthetic-opaque-signature");
        assert_eq!(
            native[0]["thinking"],
            format!("prefix{}", " fragment".repeat(64))
        );
        assert_eq!(
            native[1]["text"],
            format!("prefix{}", " fragment".repeat(64))
        );
    }
}
