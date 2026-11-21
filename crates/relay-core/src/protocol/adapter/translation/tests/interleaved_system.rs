use super::*;

fn convert(
    client: WireApi,
    upstream: WireApi,
    history: Value,
    supported: Option<bool>,
    stream: bool,
) -> AdapterResult<crate::PreparedAdapterRequest> {
    let mut request = input(client);
    request[if client == WireApi::Responses {
        "input"
    } else {
        "messages"
    }] = history;
    SourceAdapter::between(client, upstream)
        .unwrap()
        .prepare_request_with_capabilities(
            AdapterRequestContext {
                client_wire_api: client,
                request: &request,
                model: "synthetic-model",
                stream,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "synthetic-inline-system",
                response_id_seed: "synthetic-request",
            },
            &crate::model_metadata::ModelCapabilities {
                reasoning: Some(false),
                interleaved_system: supported,
                ..Default::default()
            },
        )
}

#[test]
fn declared_messages_system_support_preserves_instruction_positions_and_root_policy() {
    let history = json!([
        {"role":"system","content":"Root policy"},
        {"role":"user","content":"Earlier task"},
        {"role":"assistant","content":"Earlier answer"},
        {"role":"user","content":"Current task"},
        {"role":"system","content":"New instruction"},
        {"role":"system","content":"More context"},
        {"role":"assistant","content":"Current answer"},
        {"role":"user","content":"Next task"}
    ]);
    for client in [WireApi::Responses, WireApi::ChatCompletions] {
        for stream in [false, true] {
            let prepared = convert(
                client,
                WireApi::Messages,
                history.clone(),
                Some(true),
                stream,
            )
            .unwrap();
            let body = prepared.upstream_body();
            assert_eq!(
                body["system"],
                json!([{"type":"text","text":"Root policy"}])
            );
            let messages = body["messages"].as_array().unwrap();
            assert_eq!(messages.len(), 7);
            for (message, original) in messages
                .iter()
                .zip(history.as_array().unwrap().iter().skip(1))
            {
                assert_eq!(message["role"], original["role"]);
                assert_eq!(message["content"][0]["text"], original["content"]);
            }
        }
    }
}

#[test]
fn inline_system_support_requires_positive_evidence_for_the_target_contract() {
    let history = json!([
        {"role":"user","content":"Earlier task"},
        {"role":"assistant","content":"Earlier answer"},
        {"role":"user","content":"Current task"},
        {"role":"system","content":"Current instruction"}
    ]);
    for client in [WireApi::Responses, WireApi::ChatCompletions] {
        for stream in [false, true] {
            for (upstream, supported) in [
                (WireApi::Messages, None),
                (WireApi::Messages, Some(false)),
                (WireApi::Gemini, Some(true)),
            ] {
                let error =
                    convert(client, upstream, history.clone(), supported, stream).unwrap_err();
                assert_eq!(error.code(), "adapter_parameter_unsupported");
                assert_eq!(
                    error.parameter(),
                    Some(if client == WireApi::Responses {
                        "input[].role"
                    } else {
                        "messages[].role"
                    })
                );
            }
        }
    }
}

#[test]
fn messages_inline_system_rejects_unrepresentable_placement_roles_and_content() {
    for history in [
        json!([
            {"role":"user","content":"Task"},
            {"role":"assistant","content":"Answer"},
            {"role":"system","content":"Later instruction"}
        ]),
        json!([
            {"role":"user","content":"Task"},
            {"role":"system","content":"Interleaved instruction"},
            {"role":"user","content":"Another task"}
        ]),
        json!([
            {"role":"user","content":"Task"},
            {"role":"developer","content":"Distinct developer instruction"}
        ]),
        json!([
            {"role":"user","content":"Task"},
            {"role":"system","content":[]}
        ]),
    ] {
        for client in [WireApi::Responses, WireApi::ChatCompletions] {
            for stream in [false, true] {
                let error = convert(
                    client,
                    WireApi::Messages,
                    history.clone(),
                    Some(true),
                    stream,
                )
                .unwrap_err();
                assert_eq!(error.code(), "adapter_parameter_unsupported");
                assert_eq!(
                    error.parameter(),
                    Some(if client == WireApi::Responses {
                        "input[].role"
                    } else {
                        "messages[].role"
                    })
                );
            }
        }
    }
}

#[test]
fn consecutive_messages_system_sections_keep_every_instruction_in_order() {
    let mut history = vec![json!({"role":"user","content":"Synthetic task"})];
    history.extend(
        (0..2_000).map(
            |index| json!({"role":"system","content":format!("Synthetic instruction {index}")}),
        ),
    );
    let prepared = convert(
        WireApi::Responses,
        WireApi::Messages,
        history.into(),
        Some(true),
        false,
    )
    .unwrap();
    let messages = prepared.upstream_body()["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2_001);
    assert!(prepared.upstream_body().get("system").is_none());
    for (index, message) in messages.iter().skip(1).enumerate() {
        assert_eq!(message["role"], "system");
        assert_eq!(
            message["content"][0]["text"],
            format!("Synthetic instruction {index}")
        );
    }
}
