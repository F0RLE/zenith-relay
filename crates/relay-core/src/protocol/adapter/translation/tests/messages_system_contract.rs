use super::*;

fn convert(
    request: &Value,
    upstream: WireApi,
    stream: bool,
) -> AdapterResult<crate::PreparedAdapterRequest> {
    SourceAdapter::between(WireApi::Messages, upstream)
        .unwrap()
        .prepare_request_with_capabilities(
            AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request,
                model: "synthetic-model",
                stream,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "synthetic-system-contract",
                response_id_seed: "synthetic-request",
            },
            &crate::model_metadata::ModelCapabilities {
                reasoning: Some(false),
                ..Default::default()
            },
        )
}

#[test]
fn invalid_source_system_positions_do_not_become_valid_foreign_history() {
    for history in [
        json!([
            {"role":"system","content":"Leading inline instruction"},
            {"role":"user","content":"Task"}
        ]),
        json!([
            {"role":"user","content":"Task"},
            {"role":"assistant","content":"Answer"},
            {"role":"system","content":"Misplaced instruction"}
        ]),
        json!([
            {"role":"user","content":"Earlier task"},
            {"role":"assistant","content":"Earlier answer"},
            {"role":"user","content":"Task"},
            {"role":"system","content":"Instruction before another user turn"},
            {"role":"user","content":"Another task"}
        ]),
        json!([
            {"role":"user","content":"Task"},
            {"role":"system","content":[{
                "type":"tool_addition","tool":{"type":"tool_definition","definition":{
                    "name":"lookup","input_schema":{"type":"object"}
                }}
            }]},
            {"role":"user","content":"Another task"}
        ]),
    ] {
        let mut request = input(WireApi::Messages);
        request["messages"] = history;
        for upstream in [
            WireApi::Responses,
            WireApi::ChatCompletions,
            WireApi::Gemini,
        ] {
            for stream in [false, true] {
                let error = convert(&request, upstream, stream).unwrap_err();
                assert_eq!(error.parameter(), Some("messages[].role"));
            }
        }
        let native = convert(&request, WireApi::Messages, false).unwrap();
        assert!(native.is_passthrough());
        assert_eq!(native.upstream_body()["messages"], request["messages"]);
    }
}

#[test]
fn source_system_content_must_be_text_without_dropping_empty_or_foreign_blocks() {
    for content in [
        Value::Null,
        json!([]),
        json!([{"type":"thinking","thinking":"Not an instruction","signature":""}]),
        json!([{"type":"input_text","text":"Another protocol's block"}]),
    ] {
        let mut request = input(WireApi::Messages);
        request["messages"].as_array_mut().unwrap().push(json!({
            "role":"system","content":content
        }));
        for upstream in [
            WireApi::Responses,
            WireApi::ChatCompletions,
            WireApi::Gemini,
        ] {
            let error = convert(&request, upstream, false).unwrap_err();
            assert_eq!(error.parameter(), Some("messages[].content"));
        }
        assert!(
            convert(&request, WireApi::Messages, false)
                .unwrap()
                .is_passthrough()
        );
    }
    for content in [
        json!([{"type":"thinking","thinking":"Not a root policy","signature":""}]),
        json!([{"type":"input_text","text":"Another protocol's root block"}]),
    ] {
        let mut request = input(WireApi::Messages);
        request["system"] = content;
        let error = convert(&request, WireApi::Responses, false).unwrap_err();
        assert_eq!(error.parameter(), Some("system"));
    }
}

#[test]
fn consecutive_source_system_groups_keep_tool_changes_and_instruction_order() {
    let mut request = super::messages_inline_tools::fixture();
    let messages = request["messages"].as_array_mut().unwrap();
    messages.insert(3, json!({"role":"system","content":"First instruction"}));
    messages.push(json!({"role":"system","content":"Last instruction"}));
    for upstream in [WireApi::Responses, WireApi::ChatCompletions] {
        for stream in [false, true] {
            let prepared = convert(&request, upstream, stream).unwrap();
            let decoded = decode::request(upstream, prepared.upstream_body()).unwrap();
            let systems = decoded
                .messages
                .iter()
                .filter(|message| message.role == Role::System)
                .collect::<Vec<_>>();
            assert_eq!(systems.len(), 3);
            assert_eq!(
                systems[0].blocks,
                vec![Block::Text("First instruction".into())]
            );
            assert_eq!(systems[1].blocks[0], Block::Text("New turn policy".into()));
            assert_eq!(
                systems[2].blocks,
                vec![Block::Text("Last instruction".into())]
            );
            assert_eq!(
                decoded
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>(),
                ["later", "new"]
            );
        }
    }
}
