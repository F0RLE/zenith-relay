use super::*;

#[test]
fn messages_client_metadata_and_cache_hints_do_not_block_openai_routes() {
    let value = json!({
        "model": "test",
        "max_tokens": 128,
        "metadata": {"user_id": "synthetic-user"},
        "cache_control": {"type": "ephemeral", "ttl": "5m"},
        "system": [{"type": "text", "text": "Use the supplied tools.", "cache_control": {"type": "ephemeral", "ttl": "1h"}}],
        "messages": [{"role": "user", "content": [{"type": "text", "text": "Hello.", "cache_control": {"type": "ephemeral"}}]}],
        "tools": [{"name": "lookup", "description": "Look up a synthetic path.", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}, "cache_control": {"type": "ephemeral"}}]
    });
    for upstream in [WireApi::Responses, WireApi::ChatCompletions] {
        let prepared = prepare(WireApi::Messages, upstream, &value, false);
        let body = prepared.upstream_body();
        assert_eq!(body["metadata"]["user_id"], "synthetic-user");
        assert_eq!(
            body["tools"][0]["name"]
                .as_str()
                .or_else(|| body["tools"][0]["function"]["name"].as_str()),
            Some("lookup")
        );
        assert!(!body.to_string().contains("cache_control"));
        let decoded = decode::request(upstream, body).unwrap();
        assert!(decoded
            .messages
            .iter()
            .any(|message| message.role == Role::System
                && message.blocks == vec![Block::Text("Use the supplied tools.".into())]));
        assert!(decoded
            .messages
            .iter()
            .any(|message| message.role == Role::User
                && message.blocks == vec![Block::Text("Hello.".into())]));
    }
}

#[test]
fn messages_cache_hints_preserve_tool_results_images_and_foreign_cache_policy() {
    let value = json!({
        "model":"test", "max_tokens":128,
        "messages":[
            {"role":"user","content":[{"type":"text","text":"Look up a path."},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"c3ludGhldGlj"},"cache_control":{"type":"ephemeral","ttl":"1h"}}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"call_lookup","name":"lookup","input":{"path":"synthetic"},"cache_control":{"type":"ephemeral"}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_lookup","content":[{"type":"text","text":"synthetic result","cache_control":{"type":"ephemeral"}}],"cache_control":{"type":"ephemeral"}}]}
        ]
    });
    let mut uncached = value.clone();
    for pointer in [
        "/messages/1/content/0",
        "/messages/2/content/0",
        "/messages/2/content/0/content/0",
        "/messages/0/content/1",
    ] {
        uncached
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("cache_control");
    }
    let capabilities = crate::model_metadata::ModelCapabilities {
        reasoning: Some(false),
        ..Default::default()
    };
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let convert = |request| {
            SourceAdapter::between(WireApi::Messages, upstream)
                .unwrap()
                .prepare_request_with_capabilities(
                    AdapterRequestContext {
                        client_wire_api: WireApi::Messages,
                        request,
                        model: "test",
                        stream: false,
                        reasoning_mode: MessagesReasoningMode::Disabled,
                        cache_write_ttl: CacheWriteTtl::Provider,
                        previous: None,
                        response_scope: "source-test",
                        response_id_seed: "request-test",
                    },
                    &capabilities,
                )
                .unwrap()
        };
        let prepared = convert(&value);
        let body = prepared.upstream_body();
        assert_eq!(body, convert(&uncached).upstream_body());
        assert!(!body.to_string().contains("cache_control"));
        assert!(body.to_string().contains("synthetic result"));
        assert!(body.to_string().contains("c3ludGhldGlj"));
    }
}

#[test]
fn messages_invalid_cache_hints_fail_at_every_supported_location() {
    for hint in [
        json!("ephemeral"),
        json!({}),
        json!({"type":"permanent"}),
        json!({"type":"ephemeral","ttl":"24h"}),
        json!({"type":"ephemeral","ttl":null}),
        json!({"type":"ephemeral","required":true}),
    ] {
        let mut base = input(WireApi::Messages);
        base["system"] = json!([{"type":"text","text":"Synthetic instructions."}]);
        base["messages"][0]["content"] = json!([{"type":"text","text":"Hello."}]);
        base["tools"] = json!([{"name":"lookup","input_schema":{"type":"object"}}]);
        for pointer in ["", "/system/0", "/messages/0/content/0", "/tools/0"] {
            let mut value = base.clone();
            value.pointer_mut(pointer).unwrap()["cache_control"] = hint.clone();
            let error = decode::request(WireApi::Messages, &value).unwrap_err();
            assert_eq!(error.parameter(), Some("cache_control"), "{pointer}");
        }
    }
}

#[test]
fn messages_metadata_is_validated_and_empty_user_id_is_not_fabricated() {
    for metadata in [
        json!("synthetic"),
        json!({"user_id":1}),
        json!({"user_id":"x".repeat(513)}),
        json!({"other":"synthetic"}),
    ] {
        let mut value = input(WireApi::Messages);
        value["metadata"] = metadata;
        let error = decode::request(WireApi::Messages, &value).unwrap_err();
        assert_eq!(error.parameter(), Some("metadata"));
    }
    let mut value = input(WireApi::Messages);
    value["metadata"] = json!({"user_id":null});
    let prepared = prepare(WireApi::Messages, WireApi::Gemini, &value, false);
    assert!(prepared.upstream_body().get("metadata").is_none());
}

#[test]
fn portable_metadata_roundtrips_without_silently_dropping_unsupported_keys() {
    for client in [
        WireApi::Messages,
        WireApi::Responses,
        WireApi::ChatCompletions,
    ] {
        let mut value = input(client);
        value["metadata"] = json!({"user_id":"synthetic-user"});
        for upstream in [
            WireApi::Messages,
            WireApi::Responses,
            WireApi::ChatCompletions,
        ] {
            let prepared = prepare(client, upstream, &value, false);
            assert_eq!(prepared.upstream_body()["metadata"], value["metadata"]);
        }
        let decoded = decode::request(client, &value).unwrap();
        let mut fields = serde_json::Map::new();
        let error =
            options::write_metadata(&mut fields, &decoded.metadata, WireApi::Gemini).unwrap_err();
        assert_eq!(error.parameter(), Some("metadata"));
    }
    let mut fields = serde_json::Map::new();
    let metadata = BTreeMap::from([("foreign_key".into(), "synthetic-value".into())]);
    let error = options::write_metadata(&mut fields, &metadata, WireApi::Messages).unwrap_err();
    assert_eq!(error.parameter(), Some("metadata"));
    for client in [WireApi::Responses, WireApi::ChatCompletions] {
        let mut value = input(client);
        value["metadata"] = json!({"opaque":"synthetic-value"});
        let decoded = decode::request(client, &value).unwrap();
        assert_eq!(decoded.metadata["opaque"], "synthetic-value");
        for metadata in [json!({"opaque":true}), json!({"x".repeat(65):"value"})] {
            value["metadata"] = metadata;
            assert!(decode::request(client, &value).is_err());
        }
    }
}

#[test]
fn cache_policy_support_does_not_weaken_required_generation_features_or_native_requests() {
    let mut value = input(WireApi::Messages);
    value["cache_control"] = json!({"type":"ephemeral","ttl":"1h"});
    value["metadata"] = json!({"user_id":"synthetic-user"});
    let native = prepare(WireApi::Messages, WireApi::Messages, &value, false);
    assert_eq!(
        native.upstream_body()["cache_control"],
        value["cache_control"]
    );
    assert_eq!(native.upstream_body()["metadata"], value["metadata"]);
    for (field, content) in [
        (
            "context_management",
            json!({"edits":[{"type":"clear_tool_uses_20250919"}]}),
        ),
        ("service_tier", json!("auto")),
        ("synthetic_unknown_control", json!(true)),
    ] {
        let mut request = value.clone();
        request[field] = content;
        let error = decode::request(WireApi::Messages, &request).unwrap_err();
        assert_eq!(
            error.code(),
            crate::error_codes::ADAPTER_PARAMETER_UNSUPPORTED
        );
        assert_eq!(
            error.parameter(),
            if field == "synthetic_unknown_control" {
                None
            } else {
                Some(field)
            }
        );
    }
    value["messages"][0]["content"] = json!([{"type":"thinking","thinking":"Synthetic reasoning.","cache_control":{"type":"ephemeral"}}]);
    assert!(decode::request(WireApi::Messages, &value).is_err());
}

#[test]
fn responses_metadata_is_echoed_in_json_and_stream_response_objects() {
    let mut value = input(WireApi::Responses);
    value["metadata"] = json!({"user_id":"synthetic-user"});
    for upstream in [WireApi::Messages, WireApi::ChatCompletions] {
        let completed = prepare(WireApi::Responses, upstream, &value, false)
            .translate_response_bytes(&serde_json::to_vec(&output(upstream)).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(completed.response_body()["metadata"], value["metadata"]);
        let mut bridge = prepare(WireApi::Responses, upstream, &value, true)
            .into_stream_bridge()
            .unwrap();
        bridge.push(&sse(upstream));
        bridge.finish();
        assert_eq!(
            bridge.completed().unwrap().response_body["metadata"],
            value["metadata"]
        );
        let mut count = 0;
        while let Some(frame) = bridge.pop_output() {
            let frame = String::from_utf8(frame).unwrap();
            let event = frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .map(|data| serde_json::from_str::<Value>(data).unwrap())
                .unwrap();
            if event.get("response").is_some() {
                assert_eq!(event["response"]["metadata"], value["metadata"]);
                count += 1;
            }
        }
        assert_eq!(count, 2);
    }
}
