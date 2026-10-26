use super::*;

fn bridge_runtime(options: GatewayRuntimeOptions, secret: &str, endpoint: &str) -> GatewayRuntime {
    let mut provider = source("source-1", secret, &["gpt-test"]);
    provider.base_url = endpoint.to_owned();
    provider.wire_api = WireApi::Messages;
    let mut source = RuntimeSource::unrestricted(provider);
    source.protocol_config.endpoint_hint = Some(WireApi::Messages);
    source.protocol_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::Responses,
        adapter: SourceAdapter::ResponsesToMessages,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: vec!["gpt-test".to_owned()],
    }];
    GatewayRuntime::from_pool(
        vec![source],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        options,
        Arc::new(|_| {}),
    )
    .unwrap()
}

#[test]
fn hot_runtime_replacement_preserves_continuations_and_late_completions() {
    for late_completion in [false, true] {
        let shared = RuntimeContinuationStores::default();
        let options = GatewayRuntimeOptions {
            continuation_state: Some(shared.clone()),
            ..Default::default()
        };
        let old = bridge_runtime(
            options.clone(),
            "synthetic-secret",
            "https://example.test/v1",
        );
        let scope = CandidateScope::default();
        let old_route = old
            .executor_route("source-1", "gpt-test", &scope, &[WireApi::Responses], false)
            .unwrap();
        let prepared = crate::prepare_responses_to_messages_scoped(
            &serde_json::json!({"model":"gpt-test","input":"synthetic user turn"}),
            "gpt-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
            "source-1",
        )
        .unwrap();
        let response = crate::protocol::translate_messages_response(
            prepared,
            &serde_json::json!({
                "id":"msg-synthetic", "stop_reason":"end_turn",
                "content":[{"type":"text","text":"private-history-sentinel"}]
            }),
        )
        .unwrap();
        let save = || {
            let now_ms = current_time_ms();
            old.save_adapter_response("key-1", "source-1", &response, now_ms);
            old.bind_response_route_affinity(Some(&response.response_id), &old_route, now_ms);
        };
        if !late_completion {
            save();
        }
        let next = bridge_runtime(
            options.clone(),
            "synthetic-secret",
            "https://example.test/v1",
        );
        old.retire_for_replacement();
        if late_completion {
            save();
        }
        let next_route = next
            .executor_route("source-1", "gpt-test", &scope, &[WireApi::Responses], false)
            .unwrap();
        let request = serde_json::json!({
            "model":"gpt-test", "previous_response_id":response.response_id, "input":"continue"
        });
        assert!(
            next.load_adapter_continuation(&next_route, "key-1", &request, current_time_ms())
                .unwrap()
                .is_some()
        );
        assert!(Arc::ptr_eq(
            &old.adapter_continuation_store,
            &next.adapter_continuation_store
        ));
        assert_eq!(
            next.load_adapter_continuation_state(
                "other-key",
                &response.response_id,
                "source-1",
                current_time_ms()
            ),
            Err(crate::AdapterError::continuation_missing())
        );
        assert_eq!(
            next.load_adapter_continuation_state(
                "key-1",
                &response.response_id,
                "other-source",
                current_time_ms()
            ),
            Err(crate::AdapterError::continuation_mismatch())
        );

        for (secret, endpoint) in [
            ("changed-secret", "https://example.test/v1"),
            ("synthetic-secret", "https://other.example.test/v1"),
        ] {
            let changed = bridge_runtime(options.clone(), secret, endpoint);
            let route = changed
                .executor_route("source-1", "gpt-test", &scope, &[WireApi::Responses], false)
                .unwrap();
            assert_eq!(
                changed.load_adapter_continuation(&route, "key-1", &request, current_time_ms()),
                Err(crate::AdapterError::continuation_mismatch())
            );
        }
        let restarted = bridge_runtime(
            Default::default(),
            "synthetic-secret",
            "https://example.test/v1",
        );
        assert_eq!(
            restarted.load_adapter_continuation_state(
                "key-1",
                &response.response_id,
                "source-1",
                current_time_ms()
            ),
            Err(crate::AdapterError::continuation_missing())
        );
        assert_eq!(format!("{shared:?}"), "RuntimeContinuationStores { .. }");
    }
}

#[test]
fn hot_runtime_replacement_preserves_bounded_native_replay() {
    let options = GatewayRuntimeOptions {
        continuation_state: Some(RuntimeContinuationStores::default()),
        ..Default::default()
    };
    let old = bridge_runtime(
        options.clone(),
        "synthetic-secret",
        "https://example.test/v1",
    );
    let next = bridge_runtime(options, "synthetic-secret", "https://example.test/v1");
    old.retire_for_replacement();
    old.capture_native_responses_replay(
        "key-1",
        "source-1",
        &serde_json::json!({"model":"gpt-test", "input":"synthetic user turn"}),
        "gpt-test",
        &serde_json::json!({
            "id":"resp_native_synthetic", "status":"completed",
            "output":[{"type":"message", "role":"assistant", "status":"completed",
                "content":[{"type":"output_text", "text":"synthetic answer"}]}]
        }),
        current_time_ms(),
    );
    assert!(
        next.load_native_responses_replay(
            "key-1",
            "resp_native_synthetic",
            "source-1",
            current_time_ms()
        )
        .is_some()
    );
    assert!(
        next.load_native_responses_replay(
            "other-key",
            "resp_native_synthetic",
            "source-1",
            current_time_ms()
        )
        .is_none()
    );
    assert!(
        next.load_native_responses_replay(
            "key-1",
            "resp_native_synthetic",
            "other-source",
            current_time_ms()
        )
        .is_none()
    );
}
