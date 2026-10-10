use super::*;
use serde_json::json;

#[test]
fn incomplete_stream_never_produces_a_successful_completion() {
    for client in WireApi::ALL {
        for upstream in WireApi::ALL {
            if client == upstream {
                continue;
            }
            let mut bridge = prepare(client, upstream, &input(client), true)
                .into_stream_bridge()
                .unwrap();
            bridge.push(b"data: {");
            bridge.finish();
            assert!(bridge.is_terminal());
            assert!(bridge.completed().is_none());
        }
    }
}
#[test]
fn messages_refusal_keeps_its_terminal_reason_across_json_translation() {
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let prepared = prepare(client, WireApi::Messages, &input(client), false);
        let completed = prepared
            .translate_response_bytes(
                &serde_json::to_vec(&json!({
                    "id":"msg_refusal", "stop_reason":"refusal",
                    "content":[{"type":"text","text":"I cannot help with that."}],
                    "usage":{"input_tokens":10,"output_tokens":7}
                }))
                .unwrap(),
            )
            .unwrap()
            .unwrap();
        let decoded = response::decode(client, completed.response_body(), "seed").unwrap();
        assert_eq!(decoded.finish, Finish::Filter, "{client:?}");
    }
}
#[test]
fn messages_refusal_stream_completes_with_client_refusal_reason() {
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let mut bridge = prepare(client, WireApi::Messages, &input(client), true)
            .into_stream_bridge()
            .unwrap();
        for event in [
            json!({"type":"message_start","message":{"id":"msg_refusal","usage":{"input_tokens":10}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"I cannot help."}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"refusal"},"usage":{"output_tokens":7}}),
            json!({"type":"message_stop"}),
        ] {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        bridge.finish();
        let completed = bridge.completed().expect("refusal is a terminal response");
        if client == WireApi::Responses {
            assert_eq!(completed.response_body["status"], "incomplete");
            assert_eq!(
                completed.response_body["incomplete_details"]["reason"],
                "content_filter"
            );
        }
        let decoded = response::decode(client, &completed.response_body, "seed").unwrap();
        assert_eq!(
            decoded.finish,
            Finish::Filter,
            "{client:?}: {}",
            completed.response_body
        );
        assert_eq!(decoded.usage.input, Some(10), "{client:?}");
        assert_eq!(decoded.usage.output, Some(7), "{client:?}");
        let mut output = Vec::new();
        while let Some(chunk) = bridge.pop_output() {
            output.extend_from_slice(&chunk);
        }
        assert!(!String::from_utf8(output)
            .unwrap()
            .contains("adapter_upstream_stream_invalid"));
    }
}
#[test]
fn chat_refusal_field_and_content_part_convert_without_a_transport_error() {
    for upstream in [
        json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":null,"refusal":"Cannot help."}}]}),
        json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":[{"type":"refusal","refusal":"Cannot help."}]}}]}),
    ] {
        for client in [WireApi::Responses, WireApi::Messages, WireApi::Gemini] {
            let completed = prepare(client, WireApi::ChatCompletions, &input(client), false)
                .translate_response_bytes(&serde_json::to_vec(&upstream).unwrap())
                .unwrap()
                .unwrap();
            let decoded = response::decode(client, completed.response_body(), "seed").unwrap();
            assert_eq!(decoded.finish, Finish::Filter, "{client:?}");
            assert!(matches!(&decoded.blocks[0], Block::Text(text) if text == "Cannot help."));
        }
    }
}
#[test]
fn chat_refusal_stream_preserves_text_and_terminal_reason() {
    for client in [WireApi::Responses, WireApi::Messages, WireApi::Gemini] {
        let mut bridge = prepare(client, WireApi::ChatCompletions, &input(client), true)
            .into_stream_bridge()
            .unwrap();
        for event in [
            json!({"choices":[{"index":0,"delta":{"role":"assistant","refusal":"Cannot "},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{"refusal":"help."},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
        ] {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        bridge.push(b"data: [DONE]\n\n");
        let completed = bridge.completed().expect("refusal stream should complete");
        let decoded = response::decode(client, &completed.response_body, "seed").unwrap();
        assert_eq!(decoded.finish, Finish::Filter, "{client:?}");
        assert!(matches!(&decoded.blocks[0], Block::Text(text) if text == "Cannot help."));
    }
}
#[test]
fn gemini_prompt_block_without_candidates_keeps_filtered_terminal_in_json_and_stream() {
    for reason in [
        "SAFETY",
        "OTHER",
        "BLOCKLIST",
        "PROHIBITED_CONTENT",
        "IMAGE_SAFETY",
    ] {
        let blocked = json!({"promptFeedback":{"blockReason":reason},"usageMetadata":{"promptTokenCount":3},"candidates":[]});
        for client in [
            WireApi::Responses,
            WireApi::ChatCompletions,
            WireApi::Messages,
        ] {
            let request = input(client);
            let expected_input = (client != WireApi::Responses).then_some(3);
            let completed = prepare(client, WireApi::Gemini, &request, false)
                .translate_response_bytes(&serde_json::to_vec(&blocked).unwrap())
                .unwrap()
                .expect("converted Gemini response");
            let decoded = response::decode(client, completed.response_body(), "seed").unwrap();
            assert_eq!(decoded.finish, Finish::Filter, "{client:?}: {reason}");
            assert!(decoded
                .blocks
                .iter()
                .all(|block| matches!(block, Block::Text(text) if text.is_empty())));
            // A Responses client gets complete usage or null; a prompt-only
            // count is incomplete, so it stays unknown rather than invented.
            assert_eq!(decoded.usage.input, expected_input, "{client:?}");

            let mut bridge = prepare(client, WireApi::Gemini, &request, true)
                .into_stream_bridge()
                .unwrap();
            bridge.push(format!("data: {blocked}\n\n").as_bytes());
            let streamed = bridge
                .completed()
                .expect("blocked prompt terminates stream");
            let decoded = response::decode(client, &streamed.response_body, "seed").unwrap();
            assert_eq!(decoded.finish, Finish::Filter, "{client:?}: {reason}");
            assert!(decoded
                .blocks
                .iter()
                .all(|block| matches!(block, Block::Text(text) if text.is_empty())));
            assert_eq!(decoded.usage.input, expected_input, "{client:?}");
            if client == WireApi::Responses {
                let frames = std::iter::from_fn(|| bridge.pop_output())
                    .map(|frame| String::from_utf8(frame).unwrap())
                    .collect::<String>();
                assert!(frames.contains("event: response.incomplete"));
                assert!(!frames.contains("event: response.completed"));
            }
        }
    }
    let without_candidates = json!({"promptFeedback":{"blockReason":"SAFETY"}});
    assert_eq!(
        response::decode(WireApi::Gemini, &without_candidates, "seed")
            .unwrap()
            .finish,
        Finish::Filter
    );

    let mut bridge = prepare(
        WireApi::Responses,
        WireApi::Gemini,
        &input(WireApi::Responses),
        true,
    )
    .into_stream_bridge()
    .unwrap();
    bridge.push(b"data: {\"usageMetadata\":{\"promptTokenCount\":7}}\n\n");
    bridge.push(format!("data: {without_candidates}\n\n").as_bytes());
    let completed = bridge.completed().expect("prompt block after usage");
    // Prompt count alone is not complete Responses usage, so it is null.
    assert!(completed.response_body["usage"].is_null());
}
#[test]
fn gemini_missing_candidates_without_known_prompt_block_never_succeeds() {
    for value in [
        json!({"candidates":[],"usageMetadata":{"promptTokenCount":3}}),
        json!({"promptFeedback":{"blockReason":"BLOCK_REASON_UNSPECIFIED"},"candidates":[]}),
        json!({"promptFeedback":{"blockReason":"SAFETY"},"candidates":[{"finishReason":"STOP"}]}),
        json!({"candidates":[{"finishReason":"FINISH_REASON_UNSPECIFIED","content":{"parts":[{"text":"Hi"}]}}]}),
    ] {
        assert!(response::decode(WireApi::Gemini, &value, "seed").is_err());
        let mut bridge = prepare(
            WireApi::Responses,
            WireApi::Gemini,
            &input(WireApi::Responses),
            true,
        )
        .into_stream_bridge()
        .unwrap();
        bridge.push(format!("data: {value}\n\n").as_bytes());
        bridge.finish();
        assert!(bridge.completed().is_none());
    }
}
#[test]
fn gemini_empty_filtered_or_limited_candidate_is_incomplete_in_json_and_stream() {
    for (reason, finish) in [("SAFETY", Finish::Filter), ("MAX_TOKENS", Finish::Length)] {
        let upstream = json!({"candidates":[{"index":0,"finishReason":reason}],"usageMetadata":{"promptTokenCount":3}});
        for client in [
            WireApi::Responses,
            WireApi::ChatCompletions,
            WireApi::Messages,
        ] {
            let request = input(client);
            let result = prepare(client, WireApi::Gemini, &request, false)
                .translate_response_bytes(&serde_json::to_vec(&upstream).unwrap())
                .unwrap()
                .unwrap();
            let decoded = response::decode(client, result.response_body(), "seed").unwrap();
            assert_eq!(decoded.finish, finish, "{client:?}: {reason}");
            assert!(decoded
                .blocks
                .iter()
                .all(|block| matches!(block, Block::Text(text) if text.is_empty())));
            if client == WireApi::Responses {
                assert_eq!(
                    result.response_body()["incomplete_details"]["reason"],
                    if finish == Finish::Length {
                        "max_output_tokens"
                    } else {
                        "content_filter"
                    }
                );
            }

            let mut bridge = prepare(client, WireApi::Gemini, &request, true)
                .into_stream_bridge()
                .unwrap();
            bridge.push(format!("data: {upstream}\n\n").as_bytes());
            let result = bridge
                .completed()
                .expect("incomplete candidate terminates stream");
            let decoded = response::decode(client, &result.response_body, "seed").unwrap();
            assert_eq!(decoded.finish, finish, "{client:?}: {reason}");
            if client == WireApi::Responses {
                let frames = std::iter::from_fn(|| bridge.pop_output())
                    .map(|frame| String::from_utf8(frame).unwrap())
                    .collect::<String>();
                assert!(frames.contains("event: response.incomplete"));
                assert!(!frames.contains("event: response.completed"));
            }
        }
    }
}
