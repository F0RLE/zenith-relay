use super::{stream, Block, WireApi};
use std::{hint::black_box, time::Instant};

#[test]
fn growing_tool_snapshots_reuse_buffers_and_preserve_numeric_normalization() {
    for client in [
        WireApi::Responses,
        WireApi::Messages,
        WireApi::ChatCompletions,
    ] {
        for decimal in [false, true] {
            let upstream = if client == WireApi::ChatCompletions {
                WireApi::Messages
            } else {
                WireApi::ChatCompletions
            };
            let mut stream = stream(client, upstream);
            stream
                .translation_request
                .allowed_tool_names
                .insert("lookup".into());
            let prefix = if decimal {
                r#"{"count":1000.0,"payload":""#
            } else {
                r#"{"payload":""#
            };
            stream.decoded_response.blocks.push(Block::ToolCall {
                id: "call_synthetic".into(),
                name: "lookup".into(),
                arguments: prefix.into(),
            });
            stream.emit_changes(false).unwrap();
            while stream.pop_output().is_some() {}
            let Block::ToolCall {
                id,
                name,
                arguments,
            } = &mut stream.emitted[0]
            else {
                unreachable!()
            };
            arguments.reserve(64 * 1024);
            let pointers = (id.as_ptr(), name.as_ptr(), arguments.as_ptr());
            for _ in 0..32 {
                let Block::ToolCall { arguments, .. } = &mut stream.decoded_response.blocks[0]
                else {
                    unreachable!()
                };
                arguments.push_str(" дельта 🦀");
                stream.emit_changes(false).unwrap();
                let Block::ToolCall {
                    id,
                    name,
                    arguments,
                } = &stream.emitted[0]
                else {
                    unreachable!()
                };
                assert_eq!(
                    (id.as_ptr(), name.as_ptr(), arguments.as_ptr()),
                    pointers,
                    "{client:?}"
                );
                while stream.pop_output().is_some() {}
            }
            let Block::ToolCall { arguments, .. } = &mut stream.decoded_response.blocks[0] else {
                unreachable!()
            };
            arguments.push_str("\"}");
            let mut expected = arguments.clone();
            if client == WireApi::Responses && decimal {
                expected = expected.replacen("1000.0", "1000", 1);
            }
            stream.emit_changes(true).unwrap();
            let Block::ToolCall {
                id,
                name,
                arguments,
            } = &stream.emitted[0]
            else {
                unreachable!()
            };
            assert_eq!(arguments, &expected);
            assert_eq!(
                (id.as_ptr(), name.as_ptr(), arguments.as_ptr()),
                pointers,
                "{client:?}"
            );
        }
    }
}

#[test]
#[ignore = "manual synthetic fragmented tool argument emission measurement"]
fn measure_growing_tool_argument_emission() {
    for (fragments, fragment_bytes) in [(128, 256), (256, 1024)] {
        let fragment = "x".repeat(fragment_bytes);
        for decimal in [false, true] {
            let prefix = if decimal {
                "{\"count\":1000.0,\"payload\":\""
            } else {
                "{\"payload\":\""
            };
            let normalized_prefix = if decimal {
                "{\"count\":1000,\"payload\":\""
            } else {
                prefix
            };
            let expected = format!("{}{}\"}}", normalized_prefix, fragment.repeat(fragments));
            let mut samples = Vec::new();
            for _ in 0..5 {
                let mut stream = stream(WireApi::Responses, WireApi::ChatCompletions);
                stream
                    .translation_request
                    .allowed_tool_names
                    .insert("lookup".into());
                let mut arguments =
                    String::with_capacity(prefix.len() + fragments * fragment_bytes + 2);
                arguments.push_str(prefix);
                stream.decoded_response.blocks.push(Block::ToolCall {
                    id: "call_synthetic".into(),
                    name: "lookup".into(),
                    arguments,
                });
                stream.emit_changes(false).unwrap();
                while stream.pop_output().is_some() {}
                let mut frames = 0;
                let started = Instant::now();
                for _ in 0..fragments {
                    let Block::ToolCall { arguments, .. } = &mut stream.decoded_response.blocks[0]
                    else {
                        unreachable!()
                    };
                    arguments.push_str(black_box(&fragment));
                    stream.emit_changes(false).unwrap();
                    while let Some(frame) = stream.pop_output() {
                        black_box(frame);
                        frames += 1;
                    }
                }
                let Block::ToolCall { arguments, .. } = &mut stream.decoded_response.blocks[0]
                else {
                    unreachable!()
                };
                arguments.push_str("\"}");
                stream.emit_changes(true).unwrap();
                while let Some(frame) = stream.pop_output() {
                    black_box(frame);
                    frames += 1;
                }
                samples.push(started.elapsed().as_micros());
                assert_eq!(frames, fragments + 1);
                let Block::ToolCall { arguments, .. } = &stream.emitted[0] else {
                    unreachable!()
                };
                assert_eq!(arguments, &expected);
            }
            samples.sort_unstable();
            eprintln!(
                "tool arguments fragments={fragments} fragment_bytes={fragment_bytes} decimal={decimal} trials=5 median_us={} min_us={} max_us={}",
                samples[2], samples[0], samples[4]
            );
        }
    }
}
