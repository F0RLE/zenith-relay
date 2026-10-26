use super::{Block, WireApi, stream};
use serde_json::{Value, json};
use std::{hint::black_box, time::Instant};

fn text_buffer(block: &mut Block) -> &mut String {
    match block {
        Block::Text(text) | Block::Reasoning(text) | Block::Refusal(text) => text,
        _ => panic!("synthetic text block expected"),
    }
}

#[test]
fn growing_text_snapshots_reuse_reserved_buffers() {
    let constructors: [fn(String) -> Block; 3] = [Block::Text, Block::Reasoning, Block::Refusal];
    for client in [
        WireApi::Responses,
        WireApi::Messages,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let upstream = if client == WireApi::ChatCompletions {
            WireApi::Messages
        } else {
            WireApi::ChatCompletions
        };
        for block in constructors {
            let mut stream = stream(client, upstream);
            stream.decoded_response.blocks.push(block("prefix".into()));
            stream.emit_changes(false).unwrap();
            while stream.pop_output().is_some() {}
            let snapshot = text_buffer(&mut stream.emitted[0]);
            snapshot.reserve(64 * 1024);
            let pointer = snapshot.as_ptr();
            let capacity = snapshot.capacity();
            for _ in 0..32 {
                text_buffer(&mut stream.decoded_response.blocks[0]).push_str(" дельта 🦀");
                stream.emit_changes(false).unwrap();
                let snapshot = text_buffer(&mut stream.emitted[0]);
                assert_eq!(snapshot.as_ptr(), pointer, "{client:?}");
                assert_eq!(snapshot.capacity(), capacity, "{client:?}");
                assert_eq!(stream.emitted, stream.decoded_response.blocks);
                while stream.pop_output().is_some() {}
            }
        }
    }
}

#[test]
fn text_snapshots_reject_shrinking_and_block_kind_changes() {
    let constructors: [fn(String) -> Block; 3] = [Block::Text, Block::Reasoning, Block::Refusal];
    for client in [
        WireApi::Responses,
        WireApi::Messages,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let upstream = if client == WireApi::ChatCompletions {
            WireApi::Messages
        } else {
            WireApi::ChatCompletions
        };
        for block in constructors {
            for invalid in [
                block("Прив".into()),
                block(String::new()),
                block("ПриXет 🦀".into()),
                Block::Text("Привет 🦀".into()),
                Block::Reasoning("Привет 🦀".into()),
                Block::Refusal("Привет 🦀".into()),
            ] {
                let original = block("Привет 🦀".into());
                if invalid == original {
                    continue;
                }
                let mut stream = stream(client, upstream);
                stream.decoded_response.blocks.push(original.clone());
                stream.emit_changes(false).unwrap();
                while stream.pop_output().is_some() {}
                let pointer = text_buffer(&mut stream.emitted[0]).as_ptr();
                stream.decoded_response.blocks[0] = invalid;
                assert!(stream.emit_changes(false).is_err(), "{client:?}");
                assert_eq!(stream.emitted, vec![original]);
                assert_eq!(text_buffer(&mut stream.emitted[0]).as_ptr(), pointer);
                assert!(stream.pop_output().is_none());
            }
        }
    }
}

#[test]
fn messages_appended_text_keeps_block_order_and_single_close_events() {
    fn events(stream: &mut super::TranslationStream) -> Vec<Value> {
        std::iter::from_fn(|| stream.pop_output())
            .map(|frame| {
                let frame = String::from_utf8(frame).unwrap();
                let data = frame
                    .lines()
                    .find_map(|line| line.strip_prefix("data: "))
                    .unwrap();
                serde_json::from_str(data).unwrap()
            })
            .collect()
    }

    let mut stream = stream(WireApi::Messages, WireApi::ChatCompletions);
    stream.decoded_response.blocks = vec![
        Block::Reasoning("thought".into()),
        Block::Text("answer".into()),
    ];
    stream.emit_changes(false).unwrap();
    events(&mut stream);
    assert_eq!(stream.emitted.len(), 1);
    text_buffer(&mut stream.decoded_response.blocks[0]).push_str(" more");
    text_buffer(&mut stream.decoded_response.blocks[1]).push_str(" later");
    stream.emit_changes(false).unwrap();
    assert_eq!(stream.emitted.len(), 1);
    assert_eq!(
        events(&mut stream),
        vec![
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":" more"}})
        ]
    );

    stream.closed.insert(0);
    stream.emit_changes(false).unwrap();
    assert_eq!(stream.finished_message_blocks, 1);
    assert_eq!(stream.emitted, stream.decoded_response.blocks);
    assert_eq!(
        events(&mut stream),
        vec![
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer later"}}),
        ]
    );

    text_buffer(&mut stream.decoded_response.blocks[1]).push_str(" done");
    stream.emit_changes(true).unwrap();
    assert_eq!(stream.finished_message_blocks, 2);
    assert_eq!(
        events(&mut stream),
        vec![
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":" done"}}),
            json!({"type":"content_block_stop","index":1}),
        ]
    );
    stream.emit_changes(true).unwrap();
    assert!(events(&mut stream).is_empty());
    for index in 0..2 {
        stream.decoded_response.blocks = stream.emitted.clone();
        text_buffer(&mut stream.decoded_response.blocks[index]).push_str(" invalid");
        assert!(stream.emit_changes(false).is_err());
        assert_eq!(stream.finished_message_blocks, 2);
        assert!(events(&mut stream).is_empty());
    }
}

#[test]
fn growing_blocks_emit_only_appended_bytes_and_reject_prefix_mutation() {
    for client in [
        WireApi::Responses,
        WireApi::Messages,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let upstream = if client == WireApi::ChatCompletions {
            WireApi::Messages
        } else {
            WireApi::ChatCompletions
        };
        for original in [
            Block::Text("prefix".into()),
            Block::Reasoning("prefix".into()),
            Block::Refusal("prefix".into()),
            Block::ToolCall {
                id: "call_synthetic".into(),
                name: "lookup".into(),
                arguments: "{\"query\":\"prefix".into(),
            },
        ] {
            let mut stream = stream(client, upstream);
            stream
                .translation_request
                .allowed_tool_names
                .insert("lookup".into());
            let buffered_call =
                client == WireApi::Gemini && matches!(original, Block::ToolCall { .. });
            stream.decoded_response.blocks.push(original);
            stream.emit_changes(false).unwrap();
            while stream.pop_output().is_some() {}
            for suffix in [" first", " второй", " 🦀"] {
                match &mut stream.decoded_response.blocks[0] {
                    Block::Text(text)
                    | Block::Reasoning(text)
                    | Block::Refusal(text)
                    | Block::ToolCall {
                        arguments: text, ..
                    } => text.push_str(suffix),
                    _ => unreachable!(),
                }
                stream.emit_changes(false).unwrap();
                assert_eq!(stream.emitted, stream.decoded_response.blocks);
                let frames: Vec<_> = std::iter::from_fn(|| stream.pop_output()).collect();
                if buffered_call {
                    assert!(frames.is_empty());
                    continue;
                }
                assert_eq!(frames.len(), 1, "{client:?}");
                let output = String::from_utf8(frames.into_iter().flatten().collect()).unwrap();
                assert!(output.contains(suffix), "{client:?}");
                assert!(!output.contains("prefix"), "{client:?}");
            }
            let accepted = stream.emitted.clone();
            match &mut stream.decoded_response.blocks[0] {
                Block::Text(text)
                | Block::Reasoning(text)
                | Block::Refusal(text)
                | Block::ToolCall {
                    arguments: text, ..
                } => text.replace_range(..1, "X"),
                _ => unreachable!(),
            }
            assert!(stream.emit_changes(false).is_err(), "{client:?}");
            assert_eq!(stream.emitted, accepted);
            assert!(stream.pop_output().is_none());
        }
    }
}

#[test]
fn changed_tool_identity_is_rejected_without_changing_the_emitted_snapshot() {
    for client in [
        WireApi::Responses,
        WireApi::Messages,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let upstream = if client == WireApi::ChatCompletions {
            WireApi::Messages
        } else {
            WireApi::ChatCompletions
        };
        for change_name in [false, true] {
            let mut stream = stream(client, upstream);
            stream
                .translation_request
                .allowed_tool_names
                .insert("lookup".into());
            stream
                .translation_request
                .allowed_tool_names
                .insert("lookup_other".into());
            stream.decoded_response.blocks.push(Block::ToolCall {
                id: "call_synthetic".into(),
                name: "lookup".into(),
                arguments: "{\"query\":\"".into(),
            });
            stream.emit_changes(false).unwrap();
            while stream.pop_output().is_some() {}
            let accepted = stream.emitted.clone();
            let Block::ToolCall {
                id,
                name,
                arguments,
            } = &mut stream.decoded_response.blocks[0]
            else {
                unreachable!()
            };
            if change_name {
                *name = "lookup_other".into();
            } else {
                *id = "call_other".into();
            }
            arguments.push_str("changed");
            assert!(stream.emit_changes(false).is_err(), "{client:?}");
            assert_eq!(stream.emitted, accepted);
            assert!(stream.pop_output().is_none());
        }
    }
}

#[test]
#[ignore = "manual synthetic growing stream snapshot measurement"]
fn measure_growing_block_emission() {
    for (fragments, fragment_bytes) in [(128, 256), (1024, 1024)] {
        let fragment = "x".repeat(fragment_bytes);
        for client in [
            WireApi::Responses,
            WireApi::Messages,
            WireApi::ChatCompletions,
            WireApi::Gemini,
        ] {
            let upstream = if client == WireApi::ChatCompletions {
                WireApi::Messages
            } else {
                WireApi::ChatCompletions
            };
            let mut samples = Vec::new();
            for _ in 0..7 {
                let mut stream = stream(client, upstream);
                stream
                    .decoded_response
                    .blocks
                    .push(Block::Text(String::with_capacity(
                        fragments * fragment_bytes,
                    )));
                stream.emit_changes(false).unwrap();
                while stream.pop_output().is_some() {}
                let mut frames = 0;
                let started = Instant::now();
                for _ in 0..fragments {
                    let Block::Text(text) = &mut stream.decoded_response.blocks[0] else {
                        unreachable!()
                    };
                    text.push_str(black_box(&fragment));
                    stream.emit_changes(false).unwrap();
                    while let Some(frame) = stream.pop_output() {
                        black_box(frame);
                        frames += 1;
                    }
                }
                samples.push(started.elapsed().as_micros());
                assert_eq!(frames, fragments);
                assert_eq!(
                    stream.emitted,
                    vec![Block::Text(fragment.repeat(fragments))]
                );
            }
            samples.sort_unstable();
            eprintln!(
                "growing snapshot client={client:?} fragments={fragments} fragment_bytes={fragment_bytes} trials=7 median_us={} min_us={} max_us={}",
                samples[3], samples[0], samples[6]
            );
        }
    }
}
