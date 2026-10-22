use super::*;
use serde_json::json;

#[test]
fn messages_tool_block_stays_open_until_the_responses_terminal_validates() {
    for valid_terminal in [false, true] {
        let mut bridge =
            matrix::prepare_portable_tool_history(WireApi::Messages, WireApi::Responses, true)
                .into_stream_bridge()
                .unwrap();
        let call = json!({"type":"function_call","id":"fc_pending","call_id":"call_pending","name":"lookup","arguments":"{\"value\":3}","status":"completed"});
        let mut added = call.clone();
        added["arguments"] = json!("");
        added["status"] = json!("in_progress");
        for event in [
            json!({"type":"response.output_item.added","output_index":0,"item":added}),
            json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"value\":3}"}),
            json!({"type":"response.output_item.done","output_index":0,"item":call}),
        ] {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        let prefix = String::from_utf8(
            std::iter::from_fn(|| bridge.pop_output())
                .flatten()
                .collect(),
        )
        .unwrap();
        assert!(prefix.contains("input_json_delta"));
        assert!(!prefix.contains("event: content_block_stop"));
        assert!(bridge.completed().is_none());
        let mut terminal_call = call;
        if !valid_terminal {
            terminal_call["arguments"] = json!("{\"value\":99}");
        }
        let terminal = json!({"type":"response.completed","response":{"id":"resp_pending","status":"completed","output":[terminal_call]}});
        bridge.push(format!("data: {terminal}\n\n").as_bytes());
        bridge.finish();
        let suffix = String::from_utf8(
            std::iter::from_fn(|| bridge.pop_output())
                .flatten()
                .collect(),
        )
        .unwrap();
        if valid_terminal {
            assert_eq!(suffix.matches("event: content_block_stop").count(), 1);
            assert_eq!(suffix.matches("event: message_stop").count(), 1);
            assert!(bridge.completed().unwrap().continuation.is_reusable());
        } else {
            assert!(suffix.contains("event: error"));
            assert!(!suffix.contains("event: content_block_stop"));
            assert!(!suffix.contains("event: message_stop"));
            assert!(bridge.completed().is_none());
        }
    }
}

#[test]
fn oversized_sse_events_terminate_each_upstream_protocol_without_false_completion() {
    let oversized = vec![b'x'; crate::protocol::sse::MAX_EVENT_BYTES + 1];
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
        WireApi::Gemini,
    ] {
        let client = if upstream == WireApi::Responses {
            WireApi::Messages
        } else {
            WireApi::Responses
        };
        let mut bridge = prepare(client, upstream, &input(client), true)
            .into_stream_bridge()
            .unwrap();
        bridge.push(&oversized);
        assert!(bridge.is_terminal());
        assert!(bridge.completed().is_none());
        let wire = String::from_utf8(
            std::iter::from_fn(|| bridge.pop_output())
                .flatten()
                .collect(),
        )
        .unwrap();
        assert!(wire.contains("adapter_upstream_stream_invalid"));
        assert!(wire.contains("stream.event_size"));
        bridge.push(b"data: [DONE]\n\n");
        bridge.finish();
        assert!(bridge.pop_output().is_none());
        assert!(bridge.completed().is_none());
    }
}

#[test]
fn chat_conversion_diagnostics_preserve_safe_shapes_in_json_and_streams() {
    for (key, rejected_field) in [
        ("reasoning_details", "reasoning_details"),
        ("synthetic-private-key", "unrecognized"),
    ] {
        let extension = json!({"private":"synthetic-private-content"});
        let mut payload = output(WireApi::ChatCompletions);
        payload["choices"][0]["message"][key] = extension.clone();
        let failure = prepare(
            WireApi::Messages,
            WireApi::ChatCompletions,
            &input(WireApi::Messages),
            false,
        )
        .translate_response_bytes(&serde_json::to_vec(&payload).unwrap())
        .unwrap_err();
        assert_eq!(failure.parameter(), Some("choices[].message"));
        let diagnostic = failure.to_string();
        assert!(diagnostic.contains(&format!("rejected_field={rejected_field}")));
        assert!(diagnostic.contains("rejected_shape=object"));
        assert!(!diagnostic.contains("synthetic-private"));

        let mut bridge = prepare(
            WireApi::Messages,
            WireApi::ChatCompletions,
            &input(WireApi::Messages),
            true,
        )
        .into_stream_bridge()
        .unwrap();
        let mut event = json!({"choices":[{"index":0,"delta":{"content":"synthetic-private-content"},"finish_reason":null}]});
        event["choices"][0]["delta"][key] = extension;
        bridge.push(format!("data: {event}\n\n").as_bytes());
        bridge.finish();
        assert!(bridge.completed().is_none());
        let wire = String::from_utf8(
            std::iter::from_fn(|| bridge.pop_output())
                .flatten()
                .collect(),
        )
        .unwrap();
        assert!(wire.contains("choices[].delta"));
        assert!(wire.contains(&format!("rejected_field={rejected_field}")));
        assert!(wire.contains("rejected_shape=object"));
        assert!(!wire.contains("synthetic-private"));
    }
}

#[test]
fn chat_length_limits_preserve_messages_terminal_and_usage_in_json_and_streams() {
    let mut payload = output(WireApi::ChatCompletions);
    payload["choices"][0]["finish_reason"] = "length".into();
    payload["usage"] = json!({"prompt_tokens":23,"completion_tokens":7,"total_tokens":30});
    let result = prepare(
        WireApi::Messages,
        WireApi::ChatCompletions,
        &input(WireApi::Messages),
        false,
    )
    .translate_response_bytes(&serde_json::to_vec(&payload).unwrap())
    .unwrap()
    .expect("converted response must be available");
    assert_eq!(result.response_body()["stop_reason"], "max_tokens");
    assert_eq!(result.response_body()["usage"]["input_tokens"], 23);
    assert_eq!(result.response_body()["usage"]["output_tokens"], 7);

    let mut bridge = prepare(
        WireApi::Messages,
        WireApi::ChatCompletions,
        &input(WireApi::Messages),
        true,
    )
    .into_stream_bridge()
    .unwrap();
    for event in [
        json!({"choices":[{"index":0,"delta":{"content":"Limited output"},"finish_reason":null}]}),
        json!({"choices":[{"index":0,"delta":{},"finish_reason":"length"}],"usage":{"prompt_tokens":23,"completion_tokens":7,"total_tokens":30}}),
    ] {
        bridge.push(format!("data: {event}\n\n").as_bytes());
    }
    bridge.push(b"data: [DONE]\n\n");
    bridge.finish();
    let result = bridge
        .completed()
        .expect("a length limit is a valid terminal");
    assert_eq!(result.response_body["stop_reason"], "max_tokens");
    assert_eq!(result.response_body["usage"]["output_tokens"], 7);
    let wire = String::from_utf8(
        std::iter::from_fn(|| bridge.pop_output())
            .flatten()
            .collect(),
    )
    .unwrap();
    assert!(wire.contains("max_tokens"));
    assert!(!wire.contains("adapter_upstream_stream_invalid"));
}

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
fn messages_server_tool_pause_is_not_reported_as_token_exhaustion() {
    let paused = json!({"id":"msg_pause","stop_reason":"pause_turn",
        "content":[{"type":"text","text":"Server work is paused."}],
        "usage":{"input_tokens":10,"output_tokens":7}});
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let error = prepare(client, WireApi::Messages, &input(client), false)
            .translate_response_bytes(&serde_json::to_vec(&paused).unwrap())
            .unwrap_err();
        assert_eq!(error.parameter(), Some("stop_reason"));

        let mut bridge = prepare(client, WireApi::Messages, &input(client), true)
            .into_stream_bridge()
            .unwrap();
        for event in [
            json!({"type":"message_start","message":{"id":"msg_pause","usage":{"input_tokens":10}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Server work is paused."}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"pause_turn"},"usage":{"output_tokens":7}}),
            json!({"type":"message_stop"}),
        ] {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        bridge.finish();
        assert!(bridge.completed().is_none());
        let wire = String::from_utf8(
            std::iter::from_fn(|| bridge.pop_output())
                .flatten()
                .collect(),
        )
        .unwrap();
        assert!(
            wire.contains("\"param\":\"stop_reason\""),
            "{client:?}: {wire}"
        );
        assert!(!wire.contains("max_output_tokens"));
        assert!(!wire.contains("MAX_TOKENS"));
    }
    assert!(prepare(
        WireApi::Messages,
        WireApi::Messages,
        &input(WireApi::Messages),
        false
    )
    .is_passthrough());
}

#[test]
fn converted_json_responses_cannot_disguise_foreign_roles_or_choices() {
    for (upstream, variants, parameter) in [
        (
            WireApi::Responses,
            vec![json!("user"), json!("system"), json!(false), Value::Null],
            "output[].role",
        ),
        (
            WireApi::ChatCompletions,
            vec![json!(1), json!(-1), json!("0"), Value::Null],
            "choices[].index",
        ),
    ] {
        for variant in variants {
            let mut payload = output(upstream);
            if upstream == WireApi::Responses {
                payload["output"][0]["role"] = variant;
            } else {
                payload["choices"][0]["index"] = variant;
            }
            for client in WireApi::ALL
                .into_iter()
                .filter(|client| *client != upstream)
            {
                let error = prepare(client, upstream, &input(client), false)
                    .translate_response_bytes(&serde_json::to_vec(&payload).unwrap())
                    .unwrap_err();
                assert_eq!(
                    error.parameter(),
                    Some(parameter),
                    "{client:?} -> {upstream:?}"
                );
            }
        }
    }
}

#[test]
fn converted_messages_and_gemini_responses_validate_supplied_roles_in_json_and_sse() {
    for upstream in [WireApi::Messages, WireApi::Gemini] {
        let native_role = if upstream == WireApi::Messages {
            "assistant"
        } else {
            "model"
        };
        for role in [
            json!("user"),
            json!("system"),
            json!(false),
            json!(0),
            Value::Null,
            json!(""),
            json!(native_role),
        ] {
            let valid = role == native_role || (upstream == WireApi::Gemini && role == "");
            let mut payload = output(upstream);
            if upstream == WireApi::Messages {
                payload["role"] = role.clone();
            } else {
                payload["candidates"][0]["content"]["role"] = role.clone();
            }
            for client in WireApi::ALL
                .into_iter()
                .filter(|client| *client != upstream)
            {
                assert_eq!(
                    prepare(client, upstream, &input(client), false)
                        .translate_response_bytes(&serde_json::to_vec(&payload).unwrap())
                        .is_ok(),
                    valid,
                    "{client:?} <- {upstream:?}, role={role}"
                );
                let mut wire = String::from_utf8(sse(upstream)).unwrap();
                wire = wire.replace(
                    &format!("\"role\":\"{native_role}\""),
                    &format!("\"role\":{role}"),
                );
                let mut bridge = prepare(client, upstream, &input(client), true)
                    .into_stream_bridge()
                    .unwrap();
                for chunk in wire.as_bytes().chunks(3) {
                    bridge.push(chunk);
                }
                bridge.finish();
                assert_eq!(
                    bridge.completed().is_some(),
                    valid,
                    "{client:?} <- {upstream:?}, role={role}"
                );
            }
        }
    }
}

#[test]
fn malformed_gemini_content_cannot_disappear_at_a_stop_or_output_limit() {
    for content in [
        json!(false),
        json!("invalid"),
        json!([]),
        json!({"role":"model","parts":false}),
        json!({"parts":{}}),
        json!({"parts":"invalid"}),
    ] {
        for reason in ["STOP", "MAX_TOKENS", "SAFETY"] {
            let mut payload = output(WireApi::Gemini);
            payload["candidates"][0]["content"] = content.clone();
            payload["candidates"][0]["finishReason"] = reason.into();
            for client in [
                WireApi::Responses,
                WireApi::ChatCompletions,
                WireApi::Messages,
            ] {
                assert!(
                    prepare(client, WireApi::Gemini, &input(client), false)
                        .translate_response_bytes(&serde_json::to_vec(&payload).unwrap())
                        .is_err(),
                    "{client:?}, reason={reason}, content={content}"
                );
                let mut bridge = prepare(client, WireApi::Gemini, &input(client), true)
                    .into_stream_bridge()
                    .unwrap();
                bridge.push(b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hi\"}]}}]}\n\n");
                bridge.push(format!("data: {payload}\n\n").as_bytes());
                bridge.finish();
                assert!(
                    bridge.completed().is_none(),
                    "{client:?}, reason={reason}, content={content}"
                );
            }
        }
    }
}

#[test]
fn converted_tool_calls_must_agree_with_the_terminal_reason_in_json_and_sse() {
    for upstream in [WireApi::ChatCompletions, WireApi::Messages] {
        for calls_present in [false, true] {
            let mut payload = output(upstream);
            let (normal_reason, tool_reason) = if upstream == WireApi::ChatCompletions {
                ("stop", "tool_calls")
            } else {
                ("end_turn", "tool_use")
            };
            let reason = if calls_present {
                normal_reason
            } else {
                tool_reason
            };
            if upstream == WireApi::ChatCompletions {
                payload["choices"][0]["finish_reason"] = reason.into();
                if calls_present {
                    payload["choices"][0]["message"]["tool_calls"] = json!([{"type":"function","id":"new_lookup","function":{"name":"lookup","arguments":"{\"value\":1}"}}]);
                }
            } else {
                payload["stop_reason"] = reason.into();
                if calls_present {
                    payload["content"] = json!([{"type":"tool_use","id":"new_lookup","name":"lookup","input":{"value":1}}]);
                }
            }
            let source = if calls_present {
                tool_stream(upstream)
            } else {
                sse(upstream)
            };
            let original_reason = if calls_present {
                tool_reason
            } else {
                normal_reason
            };
            let field = if upstream == WireApi::ChatCompletions {
                "finish_reason"
            } else {
                "stop_reason"
            };
            let source = String::from_utf8(source).unwrap().replace(
                &format!("\"{field}\":\"{original_reason}\""),
                &format!("\"{field}\":\"{reason}\""),
            );
            for client in WireApi::ALL
                .into_iter()
                .filter(|client| *client != upstream)
            {
                let error = matrix::prepare_portable_tool_history(client, upstream, false)
                    .translate_response_bytes(&serde_json::to_vec(&payload).unwrap())
                    .unwrap_err();
                assert_eq!(
                    error.parameter(),
                    Some("finish_reason"),
                    "{client:?} -> {upstream:?}"
                );
                let mut bridge = matrix::prepare_portable_tool_history(client, upstream, true)
                    .into_stream_bridge()
                    .unwrap();
                for bytes in source.as_bytes().chunks(7) {
                    bridge.push(bytes);
                }
                bridge.finish();
                assert!(bridge.completed().is_none(), "{client:?} -> {upstream:?}");
                let wire = String::from_utf8(
                    std::iter::from_fn(|| bridge.pop_output())
                        .flatten()
                        .collect(),
                )
                .unwrap();
                assert!(
                    wire.contains("\"param\":\"finish_reason\""),
                    "{client:?} -> {upstream:?}: {wire}"
                );
            }
        }
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
fn responses_terminal_diagnostics_distinguish_mismatches_without_echoing_content() {
    for (case, parameter) in [
        ("count", "response.terminal.output_count"),
        ("identity", "response.output_item.identity"),
        ("changed", "response.output_item.changed"),
        ("decode", "response.terminal.decode"),
    ] {
        let mut bridge = prepare(
            WireApi::Messages,
            WireApi::Responses,
            &input(WireApi::Messages),
            true,
        )
        .into_stream_bridge()
        .unwrap();
        let mut terminal = output(WireApi::Responses);
        if case != "decode" {
            let item = terminal["output"][0].clone();
            for kind in ["response.output_item.added", "response.output_item.done"] {
                let event = json!({"type":kind,"output_index":0,"item":item});
                bridge.push(format!("data: {event}\n\n").as_bytes());
            }
        }
        match case {
            "count" => terminal["output"] = json!([]),
            "identity" => terminal["output"][0]["id"] = json!("synthetic-private-id"),
            "changed" => {
                terminal["output"][0]["content"][0]["text"] = json!("synthetic-private-content")
            }
            "decode" => {
                terminal["output"][0]["content"][0]["type"] = json!("synthetic-private-type")
            }
            _ => unreachable!(),
        }
        let event = json!({"type":"response.completed","response":terminal});
        bridge.push(format!("data: {event}\n\n").as_bytes());
        bridge.finish();
        assert!(bridge.completed().is_none());
        let wire = String::from_utf8(
            std::iter::from_fn(|| bridge.pop_output())
                .flatten()
                .collect(),
        )
        .unwrap();
        let error = wire
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|data| serde_json::from_str::<Value>(data).unwrap())
            .find(|event| event["type"] == "error")
            .unwrap();
        assert_eq!(error["error"]["param"], parameter);
        assert!(!wire.contains("synthetic-private"));
        assert!(!wire.contains("event: message_stop"));
    }
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
