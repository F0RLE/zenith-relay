use super::*;
use crate::WireApi;
use serde_json::{json, Value};

fn prepare(adapter: SourceAdapter, request: &Value) -> AdapterResult<PreparedAdapterRequest> {
    adapter.prepare_request(AdapterRequestContext {
        client_wire_api: WireApi::Responses,
        request,
        model: "synthetic-model",
        stream: false,
        reasoning_mode: MessagesReasoningMode::Adaptive,
        cache_write_ttl: Default::default(),
        previous: None,
        response_scope: "synthetic-route",
        response_id_seed: "synthetic-request",
    })
}

const BRIDGES: [SourceAdapter; 3] = [
    SourceAdapter::ResponsesToChatCompletions,
    SourceAdapter::ResponsesToMessages,
    SourceAdapter::ResponsesToGemini,
];

#[test]
fn codex_transport_controls_do_not_block_inference_or_leak_to_upstream() {
    let request = json!({
        "input":"Synthetic compacted summary",
        "store":false,
        "include":["reasoning.encrypted_content"],
        "prompt_cache_key":"synthetic-affinity",
        "client_metadata":{"source":"synthetic-client"},
        "text":{"format":{"type":"text"}},
        "reasoning":{"effort":"high","summary":"auto"},
        "parallel_tool_calls":true
    });
    for adapter in BRIDGES {
        let prepared = prepare(adapter, &request).unwrap();
        let upstream_body = prepared.upstream_body();
        for field in ["include", "prompt_cache_key", "client_metadata"] {
            assert!(upstream_body.get(field).is_none(), "{adapter:?}: {field}");
        }
        assert!(upstream_body
            .to_string()
            .contains("Synthetic compacted summary"));
        assert!(!upstream_body.to_string().contains("encrypted_content"));
    }
    let native = prepare(SourceAdapter::Native, &request).unwrap();
    for field in [
        "include",
        "prompt_cache_key",
        "client_metadata",
        "reasoning",
    ] {
        assert_eq!(native.upstream_body()[field], request[field]);
    }
}

#[test]
fn bridge_parameter_errors_name_only_known_fields_and_preserve_input() {
    for (extension, parameter) in [
        (json!({"text":{"verbosity":"high"}}), "text.verbosity"),
        (
            json!({"reasoning":{"summary":"detailed"}}),
            "reasoning.summary",
        ),
        (
            json!({"include":["message.output_text.logprobs"]}),
            "include",
        ),
        (json!({"store":true}), "store"),
        (json!({"background":true}), "background"),
        (
            json!({"stream_options":{"reasoning_summary_delivery":"sequential_cutoff"}}),
            "stream_options",
        ),
        (
            json!({"arbitrary-sensitive-field":"synthetic-value"}),
            "request",
        ),
        (
            json!({"input":[
                {"role":"user","content":"Synthetic history"},
                {"type":"reasoning","encrypted_content":"synthetic-opaque"}
            ]}),
            "input.encrypted_content",
        ),
    ] {
        let mut request = json!({"input":"Synthetic input"});
        request
            .as_object_mut()
            .unwrap()
            .extend(extension.as_object().unwrap().clone());
        let original = request.clone();
        for adapter in BRIDGES {
            let error = prepare(adapter, &request).unwrap_err();
            assert_eq!(
                error.code(),
                crate::error_codes::ADAPTER_PARAMETER_UNSUPPORTED
            );
            assert_eq!(error.parameter(), Some(parameter), "{adapter:?}");
            assert_eq!(request, original);
        }
        assert!(prepare(SourceAdapter::Native, &request).is_ok());
    }
}

#[test]
fn malformed_client_controls_are_not_ignored() {
    for (field, value) in [
        ("client_metadata", json!({"trace":12})),
        ("client_metadata", json!(["trace"])),
        ("include", json!("reasoning.encrypted_content")),
        ("prompt_cache_key", json!([12])),
    ] {
        let mut request = json!({"input":"Synthetic input"});
        request[field] = value;
        for adapter in BRIDGES {
            assert_eq!(
                prepare(adapter, &request).unwrap_err().code(),
                crate::error_codes::ADAPTER_INVALID_REQUEST
            );
        }
    }
}

#[test]
fn codex_item_trace_metadata_does_not_block_messages_before_dispatch() {
    for effort in ["low", "medium"] {
        let request = json!({
            "model":"synthetic-model", "stream":true, "store":false,
            "include":["reasoning.encrypted_content"],
            "prompt_cache_key":"synthetic-affinity",
            "client_metadata":{"source":"synthetic-client"},
            "reasoning":{"effort":effort},
            "tool_choice":"auto", "parallel_tool_calls":false,
            "tools":[{"type":"custom","name":"apply_patch","format":{"type":"text"}}],
            "input":[
                {"type":"message","role":"developer",
                    "content":[{"type":"input_text","text":"Synthetic instructions"}],
                    "internal_chat_message_metadata_passthrough":{
                        "turn_id":"synthetic-turn","create_time":123.5}},
                {"type":"message","role":"user",
                    "content":[{"type":"input_text","text":"Synthetic task"}],
                    "internal_chat_message_metadata_passthrough":{
                        "turn_id":"synthetic-turn","create_time":124}}
            ]
        });
        let original = request.clone();
        let adapter = SourceAdapter::ResponsesToMessages;
        // Reproduce the former item guard's exact generic failure, without
        // depending on a captured client body or changing the shared guard.
        let old_error = super::contracts::validate_bridge_fields(
            &request["input"][0],
            &["type", "role", "content", "id", "status", "phase"],
        )
        .unwrap_err();
        assert_eq!(
            old_error.code(),
            crate::error_codes::ADAPTER_PARAMETER_UNSUPPORTED
        );
        assert_eq!(old_error.parameter(), None);
        // The gateway performs this lookup before request translation, even
        // on the first turn. Exercise the actual pre-dispatch boundary.
        assert_eq!(
            adapter
                .continuation_key(WireApi::Responses, &request, "synthetic-model")
                .unwrap(),
            None
        );
        let prepared = prepare(adapter, &request).unwrap();
        let body = prepared.upstream_body();
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], effort);
        assert_eq!(body["system"][0]["text"], "Synthetic instructions");
        assert_eq!(body["messages"][0]["content"][0]["text"], "Synthetic task");
        assert_eq!(body["tool_choice"]["disable_parallel_tool_use"], true);
        assert_eq!(
            body["tools"][0]["input_schema"]["required"],
            json!(["input"])
        );
        assert!(!body.to_string().contains("synthetic-turn"));
        assert!(!body
            .to_string()
            .contains("internal_chat_message_metadata_passthrough"));
        assert_eq!(request, original);
        assert_eq!(
            prepare(SourceAdapter::Native, &request)
                .unwrap()
                .upstream_body()["input"],
            request["input"]
        );
    }
}

#[test]
fn codex_item_metadata_keeps_semantic_and_shape_guards() {
    const PARAMETER: &str = "input.internal_chat_message_metadata_passthrough";
    for (metadata, code) in [
        (
            json!("invalid"),
            crate::error_codes::ADAPTER_INVALID_REQUEST,
        ),
        (
            json!({"turn_id":42}),
            crate::error_codes::ADAPTER_INVALID_REQUEST,
        ),
        (
            json!({"create_time":"invalid"}),
            crate::error_codes::ADAPTER_INVALID_REQUEST,
        ),
        (
            json!({"content_item_kinds":["untrusted"]}),
            crate::error_codes::ADAPTER_PARAMETER_UNSUPPORTED,
        ),
        (
            json!({"unknown-sensitive-field":"synthetic-value"}),
            crate::error_codes::ADAPTER_PARAMETER_UNSUPPORTED,
        ),
    ] {
        let request = json!({"input":[{"role":"user","content":"Synthetic task",
            "internal_chat_message_metadata_passthrough":metadata}]});
        for adapter in BRIDGES {
            let error = adapter
                .continuation_key(WireApi::Responses, &request, "synthetic-model")
                .unwrap_err();
            assert_eq!(error.code(), code);
            assert_eq!(error.parameter(), Some(PARAMETER));
            assert_eq!(prepare(adapter, &request).unwrap_err(), error);
        }
    }
    let request = json!({"input":[{"role":"user","content":"Synthetic task",
        "unknown-sensitive-field":"synthetic-value"}]});
    assert_eq!(
        prepare(SourceAdapter::ResponsesToMessages, &request)
            .unwrap_err()
            .parameter(),
        Some("input")
    );
}

#[test]
fn codex_item_metadata_preserves_signed_messages_tool_roundtrip() {
    let adapter = SourceAdapter::ResponsesToMessages;
    let first_input = json!([{"role":"user","content":"Synthetic task",
        "internal_chat_message_metadata_passthrough":{"turn_id":"synthetic-turn","create_time":123}}]);
    let mut request = json!({
        "input":first_input, "reasoning":{"effort":"medium"},
        "tools":[{"type":"custom","name":"apply_patch","format":{"type":"text"}}]
    });
    let first = prepare(adapter, &request).unwrap();
    let mut stream = first.into_stream_bridge().unwrap();
    for event in [
        json!({"type":"message_start","message":{"id":"synthetic-message"}}),
        json!({"type":"content_block_start","index":0,
            "content_block":{"type":"thinking","thinking":"Synthetic thought","signature":"synthetic-signature"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,
            "content_block":{"type":"tool_use","id":"synthetic-call","name":"apply_patch","input":{}}}),
        json!({"type":"content_block_delta","index":1,
            "delta":{"type":"input_json_delta","partial_json":"{\"input\":\"synthetic patch\"}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},
            "usage":{"output_tokens":12}}),
        json!({"type":"message_stop"}),
    ] {
        stream.push(format!("data: {event}\n\n").as_bytes());
    }
    stream.finish();
    let completed = stream.completed().expect("valid tool stream completes");
    let mut input = first_input.as_array().unwrap().clone();
    input.extend(
        completed.response_body["output"]
            .as_array()
            .unwrap()
            .clone(),
    );
    input.push(json!({"type":"custom_tool_call_output","call_id":"synthetic-call",
        "output":"Synthetic patch applied",
        "internal_chat_message_metadata_passthrough":{"turn_id":"synthetic-turn","create_time":124}}));
    request["input"] = input.into();
    let key = adapter
        .continuation_key(WireApi::Responses, &request, "synthetic-model")
        .unwrap()
        .unwrap();
    let mut store = MessagesBridgeStore::new(8, 1_000);
    store.insert(
        "key",
        &completed.response_id,
        "synthetic-route",
        completed.continuation.clone(),
        1,
    );
    let state = store.get("key", &key, "synthetic-route", 2).unwrap();
    // Changing trace metadata must not break ownership lookup of the same
    // portable history. Ownership remains scoped by the store's route and key.
    request["input"][0]["internal_chat_message_metadata_passthrough"]["turn_id"] =
        "synthetic-reloaded-turn".into();
    assert_eq!(
        adapter
            .continuation_key(WireApi::Responses, &request, "synthetic-model")
            .unwrap(),
        Some(key.clone())
    );
    assert!(store.get("other-key", &key, "synthetic-route", 2).is_err());
    assert!(store.get("key", &key, "other-route", 2).is_err());
    let continued = adapter
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &request,
            model: "synthetic-model",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: Default::default(),
            previous: Some(state),
            response_scope: "synthetic-route",
            response_id_seed: "synthetic-next",
        })
        .unwrap();
    let messages = &continued.upstream_body()["messages"];
    assert_eq!(messages.as_array().unwrap().len(), 3);
    assert_eq!(
        messages[1]["content"][0]["signature"],
        "synthetic-signature"
    );
    assert_eq!(messages[1]["content"][1]["id"], "synthetic-call");
    assert_eq!(
        messages[1]["content"][1]["input"],
        json!({"input":"synthetic patch"})
    );
    assert_eq!(messages[2]["content"][0]["tool_use_id"], "synthetic-call");
    let response = continued
        .translate_response_bytes(
            &serde_json::to_vec(&json!({
                "id":"synthetic-final","type":"message","role":"assistant",
                "content":[{"type":"text","text":"Synthetic done"}],"stop_reason":"end_turn"
            }))
            .unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(response.response_body()["status"], "completed");
    assert_eq!(
        response.response_body()["output"][0]["content"][0]["text"],
        "Synthetic done"
    );
}
