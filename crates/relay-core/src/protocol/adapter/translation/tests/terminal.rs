use super::*;
use serde_json::json;

#[test]
fn responses_stream_failures_name_the_stage_without_echoing_provider_content() {
    for (event, parameter) in [
        (
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"synthetic-private-content"}),
            "response.text.delta",
        ),
        (
            json!({"type":"synthetic-private-event","content":"synthetic-private-content"}),
            "stream.responses.event",
        ),
    ] {
        let mut bridge = prepare(
            WireApi::Messages,
            WireApi::Responses,
            &input(WireApi::Messages),
            true,
        )
        .into_stream_bridge()
        .unwrap();
        bridge.push(format!("data: {event}\n\n").as_bytes());
        bridge.finish();
        assert!(bridge.completed().is_none());
        let wire = String::from_utf8(
            std::iter::from_fn(|| bridge.pop_output())
                .flatten()
                .collect(),
        )
        .unwrap();
        let error: Value = serde_json::from_str(
            wire.lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(error["error"]["param"], parameter);
        assert!(error["error"]["message"]
            .as_str()
            .unwrap()
            .contains(parameter));
        assert!(!wire.contains("synthetic-private"));
        assert!(!wire.contains("event: message_stop"));
    }
}

fn refusal_finish(protocol: WireApi) -> Finish {
    if protocol == WireApi::Gemini {
        Finish::Filter
    } else {
        Finish::Refusal
    }
}

fn filter_finish(protocol: WireApi) -> Finish {
    if protocol == WireApi::Messages {
        Finish::Refusal
    } else {
        Finish::Filter
    }
}

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
        assert_eq!(decoded.finish, refusal_finish(client), "{client:?}");
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
            assert_eq!(completed.response_body["status"], "completed");
            assert_eq!(
                completed.response_body["output"][1]["content"][0]["type"],
                "refusal"
            );
            assert!(completed.response_body["incomplete_details"].is_null());
        }
        let decoded = response::decode(client, &completed.response_body, "seed").unwrap();
        assert_eq!(
            decoded.finish,
            refusal_finish(client),
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
            assert_eq!(decoded.finish, refusal_finish(client), "{client:?}");
            assert!(
                matches!(&decoded.blocks[0], Block::Text(text) | Block::Refusal(text) if text == "Cannot help.")
            );
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
        assert_eq!(decoded.finish, refusal_finish(client), "{client:?}");
        assert!(
            matches!(&decoded.blocks[0], Block::Text(text) | Block::Refusal(text) if text == "Cannot help.")
        );
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
            assert_eq!(
                decoded.finish,
                filter_finish(client),
                "{client:?}: {reason}"
            );
            assert!(decoded.blocks.iter().all(
                |block| matches!(block, Block::Text(text) | Block::Refusal(text) if text.is_empty())
            ));
            // A Responses client gets complete usage or null; a prompt-only
            // count is incomplete, so it stays unknown rather than invented.
            assert_eq!(decoded.usage.input, expected_input, "{client:?}");

            let mut bridge = prepare(client, WireApi::Gemini, &request, true)
                .into_stream_bridge()
                .unwrap();
            bridge.push(format!("data: {blocked}\n\n").as_bytes());
            assert!(
                bridge.completed().is_none(),
                "Gemini usage can arrive after finish"
            );
            bridge.finish();
            let streamed = bridge
                .completed()
                .expect("blocked prompt terminates stream");
            let decoded = response::decode(client, &streamed.response_body, "seed").unwrap();
            assert_eq!(
                decoded.finish,
                filter_finish(client),
                "{client:?}: {reason}"
            );
            assert!(decoded.blocks.iter().all(
                |block| matches!(block, Block::Text(text) | Block::Refusal(text) if text.is_empty())
            ));
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
    bridge.finish();
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
            let expected = if finish == Finish::Filter {
                filter_finish(client)
            } else {
                finish
            };
            assert_eq!(decoded.finish, expected, "{client:?}: {reason}");
            assert!(decoded.blocks.iter().all(
                |block| matches!(block, Block::Text(text) | Block::Refusal(text) if text.is_empty())
            ));
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
            bridge.finish();
            let result = bridge
                .completed()
                .expect("incomplete candidate terminates stream");
            let decoded = response::decode(client, &result.response_body, "seed").unwrap();
            assert_eq!(decoded.finish, expected, "{client:?}: {reason}");
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

#[test]
fn chat_stream_requires_finish_before_done_and_retains_trailing_usage() {
    let start = json!({"choices":[{"index":0,"delta":{"content":"answer"},"finish_reason":null}]});
    let finish = json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
    let usage = json!({"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":2}});
    for events in [
        vec![start.clone(), finish.clone(), usage.clone()],
        vec![start.clone(), usage, finish.clone()],
        vec![start.clone()],
        vec![start.clone(), finish.clone(), start],
        vec![finish.clone(), finish],
    ] {
        let valid = events.len() == 3
            && events[1]["choices"][0]["finish_reason"] == "stop"
            && events[2]["choices"].as_array().unwrap().is_empty();
        let mut bridge = prepare(
            WireApi::Responses,
            WireApi::ChatCompletions,
            &input(WireApi::Responses),
            true,
        )
        .into_stream_bridge()
        .unwrap();
        for event in events {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        bridge.push(b"data: [DONE]\n\n");
        bridge.finish();
        if valid {
            let response = &bridge.completed().unwrap().response_body;
            assert_eq!(response["usage"]["input_tokens"], 5);
            assert_eq!(response["usage"]["total_tokens"], 7);
        } else {
            assert!(bridge.completed().is_none());
        }
    }
}

#[test]
fn gemini_stream_keeps_late_usage_but_transport_failure_and_reopening_are_invalid() {
    for client in [
        WireApi::Responses,
        WireApi::Messages,
        WireApi::ChatCompletions,
    ] {
        for invalid in [None, Some("late_candidate"), Some("transport")] {
            let mut bridge = prepare(client, WireApi::Gemini, &input(client), true)
                .into_stream_bridge()
                .unwrap();
            bridge.push(format!("data: {}\n\n", output(WireApi::Gemini)).as_bytes());
            assert!(bridge.completed().is_none());
            bridge.push(b"data: {\"usageMetadata\":{\"promptTokenCount\":12,\"candidatesTokenCount\":8}}\n\n");
            match invalid {
                Some("late_candidate") => {
                    bridge.push(b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"late\"}]}}]}\n\n");
                    bridge.finish();
                }
                Some("transport") => bridge.abort(),
                None => bridge.finish(),
                _ => unreachable!(),
            }
            if invalid.is_some() {
                assert!(bridge.completed().is_none(), "{client:?}: {invalid:?}");
            } else {
                let decoded =
                    response::decode(client, &bridge.completed().unwrap().response_body, "seed")
                        .unwrap();
                assert_eq!(decoded.usage.input, Some(12));
                assert_eq!(decoded.usage.output, Some(8));
            }
        }
    }
}

#[test]
fn responses_done_and_terminal_cannot_replace_streamed_content_or_reopen_items() {
    let part = json!({"type":"output_text","text":"Hi","annotations":[]});
    let item = output(WireApi::Responses)["output"][0].clone();
    let events = vec![
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_test","role":"assistant","content":[]}}),
        json!({"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"Hi"}),
        json!({"type":"response.output_text.done","output_index":0,"content_index":0,"text":"Hi"}),
        json!({"type":"response.content_part.done","output_index":0,"content_index":0,"part":part}),
        json!({"type":"response.output_item.done","output_index":0,"item":item}),
        json!({"type":"response.completed","response":output(WireApi::Responses)}),
    ];
    for mutation in 0..6 {
        let mut events = events.clone();
        match mutation {
            0 => {}
            1 => events[6]["response"]["output"][0]["content"][0]["text"] = "different".into(),
            2 => events[3]["text"] = "different".into(),
            3 => events.insert(4, events[2].clone()),
            4 => events.insert(6, events[5].clone()),
            5 => events[5]["item"]["id"] = "different".into(),
            _ => unreachable!(),
        }
        let mut bridge = prepare(
            WireApi::ChatCompletions,
            WireApi::Responses,
            &input(WireApi::ChatCompletions),
            true,
        )
        .into_stream_bridge()
        .unwrap();
        for event in events {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        bridge.finish();
        assert_eq!(
            bridge.completed().is_some(),
            mutation == 0,
            "mutation {mutation}"
        );
    }
}

#[test]
fn responses_failed_envelope_survives_for_gateway_error_classification() {
    let failed = json!({"type":"response.failed","response":{"id":"resp_synthetic","status":"failed","output":[],
        "error":{"code":"synthetic_limit","message":"Synthetic limit reached"}}});
    let mut bridge = prepare(
        WireApi::ChatCompletions,
        WireApi::Responses,
        &input(WireApi::ChatCompletions),
        true,
    )
    .into_stream_bridge()
    .unwrap();
    bridge.push(format!("data: {failed}\n\n").as_bytes());
    assert!(bridge.is_terminal());
    assert!(bridge.completed().is_none());
    assert_eq!(bridge.take_upstream_error(), Some(failed));
}

#[test]
fn chat_refusal_deduplicates_consistent_forms_and_preserves_incomplete_reason() {
    let mut body = json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant",
        "refusal":"Cannot help.","content":[{"type":"refusal","refusal":"Cannot help."}]}}]});
    let decoded = response::decode(WireApi::ChatCompletions, &body, "seed").unwrap();
    assert_eq!(decoded.finish, Finish::Refusal);
    assert_eq!(decoded.blocks, vec![Block::Refusal("Cannot help.".into())]);
    body["choices"][0]["finish_reason"] = "length".into();
    assert_eq!(
        response::decode(WireApi::ChatCompletions, &body, "seed")
            .unwrap()
            .finish,
        Finish::Length
    );
    body["choices"][0]["message"]["content"][0]["refusal"] = "different".into();
    assert!(response::decode(WireApi::ChatCompletions, &body, "seed").is_err());
}
