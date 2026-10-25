use super::*;

#[test]
fn chat_participant_names_cannot_be_silently_discarded_by_conversion() {
    for role in ["user", "assistant", "system", "developer", "tool"] {
        let mut request = input(WireApi::ChatCompletions);
        request["messages"][0] =
            json!({"role":role,"name":"synthetic_participant","content":"Hello"});
        if role == "tool" {
            request["messages"][0]["tool_call_id"] = "call_test".into();
        }
        let error = decode::request(WireApi::ChatCompletions, &request).unwrap_err();
        assert_eq!(error.parameter(), Some("messages[].name"), "role={role}");
        assert!(prepare(
            WireApi::ChatCompletions,
            WireApi::ChatCompletions,
            &request,
            false
        )
        .is_passthrough());
    }
}

#[test]
fn responses_refusal_content_is_only_valid_in_assistant_history() {
    for role in ["user", "system", "developer"] {
        let request = json!({"input":[{"role":role,"content":[{"type":"refusal","refusal":"Cannot help."}]}]});
        let error = decode::request(WireApi::Responses, &request).unwrap_err();
        assert_eq!(error.parameter(), Some("input[].content"));
    }
}

#[test]
fn converted_image_details_reject_wrong_types_and_conflicting_locations() {
    for client in [WireApi::Responses, WireApi::ChatCompletions] {
        for detail in [json!(false), json!(1), json!([]), json!({})] {
            let image = if client == WireApi::Responses {
                json!({"type":"input_image","image_url":"data:image/png;base64,AA==","detail":detail})
            } else {
                json!({"type":"image_url","image_url":{"url":"data:image/png;base64,AA==","detail":detail}})
            };
            let mut request = input(client);
            let field = if client == WireApi::Responses {
                "input"
            } else {
                "messages"
            };
            request[field] = json!([{"role":"user","content":[image]}]);
            let error = decode::request(client, &request).unwrap_err();
            assert_eq!(error.parameter(), Some("image.detail"));
            assert!(prepare(client, client, &request, false).is_passthrough());
        }
    }
    let request = json!({"messages":[{"role":"user","content":[{
        "type":"image_url","detail":"high",
        "image_url":{"url":"data:image/png;base64,AA==","detail":"low"}
    }]}]});
    assert_eq!(
        decode::request(WireApi::ChatCompletions, &request)
            .unwrap_err()
            .parameter(),
        Some("image.detail")
    );
}

#[test]
fn converted_tool_choices_reject_foreign_shapes_and_hidden_controls() {
    for (client, choices) in [
        (
            WireApi::ChatCompletions,
            vec![
                json!({"type":"auto"}),
                json!({"type":"required"}),
                json!({"type":"custom","function":{"name":"lookup"}}),
                json!({"type":"function","function":{"name":"lookup","arguments":"{}"}}),
                json!({"type":"function","function":{"name":"lookup"},"disable_parallel_tool_use":true}),
            ],
        ),
        (
            WireApi::Messages,
            vec![
                json!({"type":"required"}),
                json!({"type":"function","name":"lookup"}),
                json!({"type":"auto","name":"lookup"}),
                json!({"type":"none","disable_parallel_tool_use":true}),
            ],
        ),
    ] {
        for choice in choices {
            let mut request = input(client);
            request["tools"] = tool_history(client)["tools"].clone();
            request["tool_choice"] = choice;
            assert!(
                decode::request(client, &request).is_err(),
                "{client:?}: {request}"
            );
            assert!(prepare(client, client, &request, false).is_passthrough());
        }
    }
}

#[test]
fn converted_tool_choices_keep_native_forms_and_parallel_policy() {
    for (client, choices) in [
        (
            WireApi::ChatCompletions,
            vec![
                json!("auto"),
                json!("none"),
                json!("required"),
                json!({"type":"function","function":{"name":"lookup"}}),
            ],
        ),
        (
            WireApi::Messages,
            vec![
                json!({"type":"auto"}),
                json!({"type":"none"}),
                json!({"type":"any"}),
                json!({"type":"tool","name":"lookup","disable_parallel_tool_use":true}),
            ],
        ),
    ] {
        for choice in choices {
            let mut request = input(client);
            request["tools"] = tool_history(client)["tools"].clone();
            request["tool_choice"] = choice;
            if client == WireApi::ChatCompletions {
                request["parallel_tool_calls"] = false.into();
            }
            let decoded = decode::request(client, &request).unwrap();
            if client == WireApi::ChatCompletions
                || request["tool_choice"]["disable_parallel_tool_use"] == true
            {
                assert_eq!(decoded.parallel_tools, Some(false));
            }
            for upstream in WireApi::ALL
                .into_iter()
                .filter(|upstream| *upstream != client)
            {
                let prepared = prepare(client, upstream, &request, false);
                let encoded = prepared.upstream_body();
                assert!(
                    decode::request(upstream, encoded).is_ok(),
                    "{upstream:?}: {encoded}"
                );
                if upstream == WireApi::Messages && encoded["tool_choice"]["type"] == "none" {
                    assert_eq!(encoded["tool_choice"], json!({"type":"none"}));
                }
            }
        }
    }
}

#[test]
fn converted_chat_requests_do_not_discard_invalid_tool_fields() {
    for fields in [
        json!({"tool_calls":{}}),
        json!({"tool_calls":"not a call list"}),
        json!({"tool_calls":false}),
        json!({"function_call":[]}),
        json!({"function_call":"not a call"}),
        json!({
            "tool_calls":[{"id":"call_synthetic","type":"function","function":{"name":"lookup","arguments":"{}"}}],
            "function_call":{"name":"lookup","arguments":"{}"}
        }),
    ] {
        let mut message = json!({"role":"assistant","content":"Earlier response"});
        message
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let request = json!({"messages":[
            {"role":"user","content":"Check a value"},
            message,
            {"role":"user","content":"Continue"}
        ]});
        assert!(
            decode::request(WireApi::ChatCompletions, &request).is_err(),
            "{fields}"
        );
        assert!(prepare(
            WireApi::ChatCompletions,
            WireApi::ChatCompletions,
            &request,
            false
        )
        .is_passthrough());
    }
    for calls in [Value::Null, json!([])] {
        let request = json!({"messages":[{"role":"user","content":"Hi","tool_calls":calls}]});
        assert!(decode::request(WireApi::ChatCompletions, &request).is_ok());
    }
}

#[test]
fn converted_chat_tool_results_cannot_hide_new_calls() {
    for field in ["tool_calls", "function_call"] {
        let mut request = tool_history(WireApi::ChatCompletions);
        request["messages"][3][field] = request["messages"][2]["tool_calls"].clone();
        assert!(decode::request(WireApi::ChatCompletions, &request).is_err());
    }
}

#[test]
fn converted_json_tool_calls_reject_blank_ids_without_normalizing_opaque_ids() {
    for upstream in WireApi::ALL {
        for id in ["   ", "\t\n", " call_opaque "] {
            let call = json!({"id":id,"name":"lookup","args":{"value":1}});
            let payload = match upstream {
                WireApi::GeminiInteractions => unreachable!("native-only fixture is separate"),
                WireApi::Responses => json!({"id":"resp_synthetic","status":"completed","output":[{
                    "type":"function_call","id":"fc_synthetic","call_id":id,"name":"lookup","arguments":"{\"value\":1}"
                }]}),
                WireApi::ChatCompletions => {
                    json!({"choices":[{"index":0,"message":{"role":"assistant","tool_calls":[{
                    "id":id,"type":"function","function":{"name":"lookup","arguments":"{\"value\":1}"}
                }]},"finish_reason":"tool_calls"}]})
                }
                WireApi::Messages => json!({"id":"msg_synthetic","content":[{
                    "type":"tool_use","id":id,"name":"lookup","input":{"value":1}
                }],"stop_reason":"tool_use"}),
                WireApi::Gemini => {
                    json!({"candidates":[{"index":0,"content":{"role":"model","parts":[
                    {"functionCall":call}
                ]},"finishReason":"STOP"}]})
                }
            };
            let decoded = response::decode(upstream, &payload, "test");
            if id.trim().is_empty() {
                assert!(decoded.is_err(), "{upstream:?}: {id:?}");
            } else {
                assert!(matches!(
                    &decoded.unwrap().blocks[0], Block::ToolCall { id: actual, .. } if actual == id
                ));
            }
        }
    }
}

#[test]
fn converted_tool_history_rejects_blank_ids_without_normalizing_opaque_ids() {
    for client in WireApi::ALL {
        for id in ["   ", "\t\n", " call_opaque "] {
            let encoded = serde_json::to_string(id).unwrap();
            let request: Value = serde_json::from_str(
                &tool_history(client)
                    .to_string()
                    .replace("\"call_first\"", &encoded),
            )
            .unwrap();
            let resolved = decode::request(client, &request).and_then(|mut request| {
                decode::resolve_tool_history(&mut request.messages)?;
                Ok(request)
            });
            if id.trim().is_empty() {
                assert!(resolved.is_err(), "{client:?}: {id:?}");
            } else {
                let decoded = resolved.unwrap();
                assert!(decoded
                    .messages
                    .iter()
                    .flat_map(|message| &message.blocks)
                    .any(
                        |block| matches!(block, Block::ToolCall { id: actual, .. } if actual == id)
                    ));
                assert!(decoded.messages.iter().flat_map(|message| &message.blocks).any(
                    |block| matches!(block, Block::ToolResult { id: actual, .. } if actual == id)
                ));
            }
        }
    }
}

#[test]
fn converted_tool_streams_reject_blank_ids_before_forwarding_the_call() {
    for upstream in WireApi::ALL {
        for client in WireApi::ALL
            .into_iter()
            .filter(|client| *client != upstream)
        {
            let mut request = input(client);
            request["tools"] = tool_history(client)["tools"].clone();
            let prepared = prepare(client, upstream, &request, true);
            let mut bridge = prepared.into_stream_bridge().unwrap();
            let events = String::from_utf8(tool_stream(upstream))
                .unwrap()
                .replace("call_new_0", "   ");
            bridge.push(events.as_bytes());
            bridge.finish();
            assert!(bridge.completed().is_none(), "{upstream:?} -> {client:?}");
            let mut frames = String::new();
            while let Some(frame) = bridge.pop_output() {
                frames.push_str(std::str::from_utf8(&frame).unwrap());
            }
            assert!(
                frames.contains("adapter_upstream_stream_invalid"),
                "{frames}"
            );
            assert!(!frames.contains("\"call_id\":\"   \""), "{frames}");
            assert!(!frames.contains("\"id\":\"   \""), "{frames}");
        }
    }
}

#[test]
fn message_roles_belong_to_the_client_protocol() {
    for (protocol, foreign_role) in [
        (WireApi::Messages, "model"),
        (WireApi::Messages, "developer"),
        (WireApi::ChatCompletions, "model"),
        (WireApi::Responses, "model"),
        (WireApi::Gemini, "assistant"),
        (WireApi::Gemini, "system"),
    ] {
        let mut request = input(protocol);
        match protocol {
            WireApi::Gemini => {
                request["contents"] = json!([
                    {"role":foreign_role,"parts":[{"text":"Earlier"}]},
                    {"role":"user","parts":[{"text":"Hi"}]}
                ])
            }
            WireApi::Responses => {
                request["input"] = json!([
                    {"role":foreign_role,"content":"Earlier"},
                    {"role":"user","content":"Hi"}
                ])
            }
            _ => {
                request["messages"] = json!([
                    {"role":foreign_role,"content":"Earlier"},
                    {"role":"user","content":"Hi"}
                ])
            }
        }
        assert!(
            decode::request(protocol, &request).is_err(),
            "{protocol:?}: {foreign_role}"
        );
        // This is a conversion boundary, not new validation of native extensions.
        assert!(prepare(protocol, protocol, &request, false).is_passthrough());
    }
}

#[test]
fn gemini_unspecified_role_still_defaults_to_user() {
    let request = json!({"contents":[{"parts":[{"text":"Hi"}]}]});
    let decoded = decode::request(WireApi::Gemini, &request).unwrap();
    assert_eq!(decoded.messages[0].role, Role::User);
}

#[test]
fn gemini_content_parts_do_not_silently_drop_a_second_payload() {
    for payload in [
        json!({"functionCall":{"name":"read","args":{}}}),
        json!({"inlineData":{"mimeType":"image/png","data":"aA=="}}),
        json!({"fileData":{"mimeType":"image/png","fileUri":"https://example.test/image"}}),
    ] {
        let mut part = payload;
        part["text"] = "Visible text".into();
        let request = json!({"contents":[{"role":"user","parts":[part]}]});
        assert!(decode::request(WireApi::Gemini, &request).is_err());
    }
}

#[test]
fn gemini_thought_flags_require_a_boolean() {
    let request = json!({"contents":[{"parts":[{"text":"Hi","thought":"true"}]}]});
    assert!(decode::request(WireApi::Gemini, &request).is_err());
}

#[test]
fn null_gemini_union_fields_do_not_shadow_the_supplied_payload() {
    let request = json!({"contents":[{"role":"model","parts":[{
        "text":null,"inlineData":null,"fileData":null,
        "functionCall":{"id":"call-synthetic","name":"read","args":{}}
    }]}]});
    let decoded = decode::request(WireApi::Gemini, &request).unwrap();
    assert!(
        matches!(&decoded.messages[0].blocks[0], Block::ToolCall { id, .. } if id == "call-synthetic")
    );
}

#[test]
fn gemini_calls_retain_explicit_identity_and_object_arguments_or_fail() {
    for part in [
        json!({"functionCall":{"id":7,"name":"read","args":{}}}),
        json!({"functionCall":{"name":"read","args":[]}}),
        json!({"functionCall":{"name":"read","args":"not an object"}}),
        json!({"functionResponse":{"id":7,"name":"read","response":{"output":"ok"}}}),
    ] {
        let request = json!({"contents":[{"role":"model","parts":[part]}]});
        assert!(decode::request(WireApi::Gemini, &request).is_err());
    }
}

#[test]
fn gemini_function_name_constraints_cannot_be_ignored_in_other_modes() {
    for mode in ["AUTO", "NONE"] {
        let request = json!({
            "contents":[{"parts":[{"text":"Hi"}]}],
            "tools":[{"functionDeclarations":[{"name":"read","parameters":{"type":"object"}}]}],
            "toolConfig":{"functionCallingConfig":{"mode":mode,"allowedFunctionNames":["read"]}}
        });
        assert!(decode::request(WireApi::Gemini, &request).is_err());
    }
}

#[test]
fn null_chat_completion_limit_does_not_erase_the_explicit_token_limit() {
    let request = json!({
        "messages":[{"role":"user","content":"Hi"}],
        "max_tokens":256,"max_completion_tokens":null
    });
    for upstream in [WireApi::Responses, WireApi::Messages, WireApi::Gemini] {
        let prepared = prepare(WireApi::ChatCompletions, upstream, &request, false);
        let limit = match upstream {
            WireApi::Responses => prepared.upstream_body().get("max_output_tokens"),
            WireApi::Messages => prepared.upstream_body().get("max_tokens"),
            _ => prepared
                .upstream_body()
                .pointer("/generationConfig/maxOutputTokens"),
        };
        assert_eq!(limit.and_then(Value::as_u64), Some(256), "{upstream:?}");
    }
}

#[test]
fn conflicting_chat_token_limits_are_not_silently_preferred() {
    let request = json!({
        "messages":[{"role":"user","content":"Hi"}],
        "max_tokens":256,"max_completion_tokens":512
    });
    assert!(decode::request(WireApi::ChatCompletions, &request).is_err());
    let mut matching = request;
    matching["max_completion_tokens"] = 256.into();
    assert_eq!(
        decode::request(WireApi::ChatCompletions, &matching)
            .unwrap()
            .max_tokens,
        Some(256)
    );
}

#[test]
fn gemini_tool_configuration_keeps_documented_empty_defaults() {
    for config in [
        json!({}),
        json!({"functionCallingConfig":{}}),
        json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":[]}}),
    ] {
        let request = json!({
            "contents":[{"parts":[{"text":"Hi"}]}],
            "tools":[{"functionDeclarations":[{"name":"read","parameters":{"type":"object"}}]}],
            "toolConfig":config
        });
        for upstream in [
            WireApi::Responses,
            WireApi::ChatCompletions,
            WireApi::Messages,
        ] {
            let converted = try_convert(WireApi::Gemini, upstream, &request, false);
            assert!(converted.is_ok(), "{upstream:?}: {converted:?}");
        }
    }
}

#[test]
fn initial_system_attachments_after_user_messages_preserve_instruction_and_text_order() {
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
    ] {
        let mut request = input(client);
        let field = if client == WireApi::Responses {
            "input"
        } else {
            "messages"
        };
        request[field] = json!([
            {"role":"user","content":"Hello"},
            {"role":"system","content":"Use synthetic data."},
            {"role":"user","content":"Keep the order."},
            {"role":"system","content":"Use the supplied tools."}
        ]);
        match client {
            WireApi::Responses => request["instructions"] = "Root instruction.".into(),
            WireApi::Messages => request["system"] = "Root instruction.".into(),
            _ => request[field]
                .as_array_mut()
                .unwrap()
                .insert(0, json!({"role":"system","content":"Root instruction."})),
        }
        for upstream in [WireApi::Messages, WireApi::Gemini] {
            if upstream == client {
                continue;
            }
            for stream in [false, true] {
                let prepared = prepare(client, upstream, &request, stream);
                let decoded = decode::request(upstream, prepared.upstream_body()).unwrap();
                assert_eq!(
                    decoded.messages,
                    vec![
                        Message {
                            role: Role::System,
                            blocks: vec![
                                Block::Text("Root instruction.".into()),
                                Block::Text("Use synthetic data.".into()),
                                Block::Text("Use the supplied tools.".into()),
                            ],
                        },
                        Message {
                            role: Role::User,
                            blocks: vec![
                                Block::Text("Hello".into()),
                                Block::Text("Keep the order.".into()),
                            ],
                        },
                    ],
                    "{client:?} -> {upstream:?}, stream={stream}"
                );
            }
        }
    }
}

#[test]
fn mid_conversation_system_messages_are_not_hoisted_into_initial_instructions() {
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
    ] {
        let mut request = input(client);
        let history = json!([
            {"role":"user","content":"Earlier"},
            {"role":"assistant","content":"Earlier reply"},
            {"role":"system","content":"New instruction"},
            {"role":"user","content":"Continue"}
        ]);
        request[if client == WireApi::Responses {
            "input"
        } else {
            "messages"
        }] = history;
        for upstream in WireApi::ALL {
            let converted = try_convert(client, upstream, &request, false);
            if upstream != client && matches!(upstream, WireApi::Messages | WireApi::Gemini) {
                assert_eq!(
                    converted.unwrap_err().code(),
                    "adapter_parameter_unsupported"
                );
            } else {
                let converted = converted.unwrap();
                if upstream != client {
                    let field = if upstream == WireApi::Responses {
                        "input"
                    } else {
                        "messages"
                    };
                    assert_eq!(converted.upstream_body()[field][2]["role"], "system");
                }
            }
        }
    }
}

#[test]
fn null_gemini_error_fields_do_not_turn_successful_tool_results_into_errors() {
    let request = json!({"contents":[{"role":"user","parts":[{
        "functionResponse":{"name":"read","response":{"output":"ok","error":null}}
    }]}]});
    let decoded = decode::request(WireApi::Gemini, &request).unwrap();
    assert!(matches!(
        &decoded.messages[0].blocks[0],
        Block::ToolResult {
            is_error: false,
            ..
        }
    ));
}
