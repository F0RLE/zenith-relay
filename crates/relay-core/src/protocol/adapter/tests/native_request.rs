use super::*;
use serde_json::json;

#[test]
fn native_prepared_request_is_transparent_for_opaque_tools() {
    let request = json!({
        "model": "alias",
        "input": "inspect",
        "reasoning": {"effort": "high", "summary": "auto"},
        "tools": [{
            "type": "computer_use_preview",
            "name": "PowerShell",
            "display_width": 1200,
            "display_height": 800
        }]
    });
    let prepared = SourceAdapter::Native
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &request,
            model: "resolved-model",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            previous: None,
            response_scope: "native-route",
            response_id_seed: "request-1",
        })
        .unwrap();

    assert!(prepared.is_passthrough());
    assert_eq!(prepared.upstream_body()["model"], "resolved-model");
    assert_eq!(prepared.upstream_body()["reasoning"], request["reasoning"]);
    assert_eq!(prepared.upstream_body()["tools"], request["tools"]);
    assert!(prepared
        .translate_response_bytes(br#"{}"#)
        .unwrap()
        .is_none());
}
#[test]
fn native_context_management_remains_client_owned() {
    let request = json!({
        "model": "alias",
        "input": "hello",
        "context_management": [{"type": "compaction", "compact_threshold": 1000}]
    });

    let responses = SourceAdapter::Native
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &request,
            model: "resolved-model",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            previous: None,
            response_scope: "responses-route",
            response_id_seed: "request-context-management",
        })
        .unwrap();
    assert_eq!(
        responses.upstream_body()["context_management"],
        request["context_management"]
    );

    for wire_api in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        let prepared = SourceAdapter::Native
            .prepare_request(AdapterRequestContext {
                client_wire_api: wire_api,
                request: &request,
                model: "resolved-model",
                stream: false,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                previous: None,
                response_scope: "non-responses-route",
                response_id_seed: "request-context-management",
            })
            .unwrap();
        assert_eq!(
            prepared.upstream_body()["context_management"],
            request["context_management"]
        );
    }
}
#[test]
fn response_bridges_reject_context_management_before_translation() {
    let request = json!({
        "model": "alias",
        "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hello"}]}],
        "context_management": [{"type": "compaction", "compact_threshold": 1000}]
    });

    for adapter in [
        SourceAdapter::ResponsesToChatCompletions,
        SourceAdapter::ResponsesToMessages,
        SourceAdapter::ResponsesToGemini,
    ] {
        let error = adapter
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Responses,
                request: &request,
                model: "resolved-model",
                stream: false,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                previous: None,
                response_scope: "bridge-route",
                response_id_seed: "request-context-management",
            })
            .unwrap_err();
        assert_eq!(error.code(), "adapter_compaction_unsupported");
        assert!(error.is_route_incompatible());
    }
}
#[test]
fn compaction_history_requires_native_responses_without_mutating_input() {
    for kind in ["compaction", "compaction_summary"] {
        for input in [
            json!([{ "type": kind, "encrypted_content": "opaque-fixture" }]),
            json!({ "type": kind, "encrypted_content": "opaque-fixture" }),
        ] {
            let request = json!({"model": "alias", "input": input});
            let original = request.clone();
            for adapter in [
                SourceAdapter::Native,
                SourceAdapter::ResponsesToChatCompletions,
                SourceAdapter::ResponsesToMessages,
                SourceAdapter::ResponsesToGemini,
            ] {
                let result = adapter.prepare_request(AdapterRequestContext {
                    client_wire_api: WireApi::Responses,
                    request: &request,
                    model: "resolved-model",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
                    previous: None,
                    response_scope: "compaction-route",
                    response_id_seed: "compaction-request",
                });
                match result {
                    Ok(prepared) => {
                        assert_eq!(adapter, SourceAdapter::Native);
                        assert_eq!(prepared.upstream_body()["input"], original["input"]);
                    }
                    Err(error) => {
                        assert_ne!(adapter, SourceAdapter::Native);
                        assert_eq!(error.code(), "adapter_compaction_unsupported");
                        assert!(error.is_route_incompatible());
                        assert!(!error.is_upstream_failure());
                    }
                }
                assert_eq!(request, original);
            }
        }
    }
}
#[test]
fn native_responses_request_keeps_image_input_opaque() {
    let request = json!({
        "model": "alias",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_image",
                "image_url": "data:image/png;base64,YQ=="
            }]
        }]
    });
    let prepared = SourceAdapter::Native
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &request,
            model: "resolved-model",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            previous: None,
            response_scope: "native-route",
            response_id_seed: "request-2",
        })
        .unwrap();

    assert_eq!(prepared.upstream_body()["model"], "resolved-model");
    assert_eq!(prepared.upstream_body()["input"], request["input"]);
}
#[test]
fn response_bridges_share_the_complete_client_tool_catalog() {
    let request = json!({
        "model": "bridge-test",
        "tools": [{
            "type": "function",
            "name": "root_tool",
            "parameters": {"type": "object"}
        }],
        "input": [
            {
                "type": "additional_tools",
                "tools": [{
                    "type": "function",
                    "name": "deferred_tool",
                    "parameters": {"type": "object"}
                }]
            },
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "inspect"}]
            }
        ]
    });
    let messages = SourceAdapter::ResponsesToMessages
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &request,
            model: "claude-test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            previous: None,
            response_scope: "messages-route",
            response_id_seed: "messages-tools",
        })
        .unwrap();
    assert_eq!(messages.upstream_body()["tools"][0]["name"], "root_tool");
    assert_eq!(
        messages.upstream_body()["tools"][1]["name"],
        "deferred_tool"
    );

    let gemini = SourceAdapter::ResponsesToGemini
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &request,
            model: "gemini-test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            previous: None,
            response_scope: "gemini-route",
            response_id_seed: "gemini-tools",
        })
        .unwrap();
    assert_eq!(
        gemini.upstream_body()["tools"][0]["functionDeclarations"][0]["name"],
        "root_tool"
    );
    assert_eq!(
        gemini.upstream_body()["tools"][0]["functionDeclarations"][1]["name"],
        "deferred_tool"
    );
}
#[test]
fn scoped_bridge_ids_keep_same_upstream_id_isolated_between_routes() {
    let first = prepare_responses_to_messages_scoped(
        &request(Value::String("first".to_string())),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
        "source-a/responses-bridge",
    )
    .unwrap();
    let second = prepare_responses_to_messages_scoped(
        &request(Value::String("second".to_string())),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
        "source-b/responses-bridge",
    )
    .unwrap();
    let upstream = json!({
        "id": "msg_same",
        "stop_reason": "end_turn",
        "content": [{"type": "text", "text": "ok"}]
    });
    let first = translate_messages_response(first, &upstream).unwrap();
    let second = translate_messages_response(second, &upstream).unwrap();

    assert_ne!(first.response_id, second.response_id);
    assert_eq!(
        first.response_body["output"][0]["id"],
        format!("msg_{}", first.response_id)
    );
    assert_eq!(
        second.response_body["output"][0]["id"],
        format!("msg_{}", second.response_id)
    );
}
