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
fn agent_message_saved_history_can_resume_after_host_restart() {
    // The Desktop supplies the same key after reopening its protected vault.
    let protected_key = [19; 32];
    let options = || GatewayRuntimeOptions {
        continuation_state: Some(RuntimeContinuationStores::with_agent_message_key(
            protected_key,
        )),
        ..Default::default()
    };
    let first = bridge_runtime(options(), "synthetic-secret", "https://example.test/v1");
    let sealed = first
        .agent_message_context("key-1")
        .seal_argument(
            "collaboration.send_message",
            "message",
            "synthetic saved assignment",
        )
        .unwrap();
    drop(first);
    let restarted = bridge_runtime(options(), "synthetic-secret", "https://example.test/v1");
    let mut request = serde_json::json!({"input":[{
        "type":"agent_message","author":"/root","recipient":"/root/worker",
        "encrypted_content":sealed
    }]});
    let original = request.clone();
    restarted
        .agent_message_context("key-1")
        .restore_request(&mut request)
        .unwrap();
    assert_eq!(request["input"][0]["role"], "user");
    assert_eq!(
        request["input"][0]["content"][1]["text"],
        "synthetic saved assignment"
    );
    assert!(
        restarted
            .agent_message_context("key-2")
            .restore_request(&mut original.clone())
            .is_err()
    );
    let other_profile = bridge_runtime(
        GatewayRuntimeOptions {
            continuation_state: Some(RuntimeContinuationStores::with_agent_message_key([20; 32])),
            ..Default::default()
        },
        "synthetic-secret",
        "https://example.test/v1",
    );
    assert!(
        other_profile
            .agent_message_context("key-1")
            .restore_request(&mut original.clone())
            .is_err()
    );
    let mut foreign = serde_json::json!({"input":[{"type":"agent_message","encrypted_content":"foreign-provider-ciphertext"}]});
    let unchanged = foreign.clone();
    restarted
        .agent_message_context("key-1")
        .restore_request(&mut foreign)
        .unwrap();
    assert_eq!(foreign, unchanged);
}

#[test]
fn agent_messages_survive_graph_replacement_but_not_another_key_or_host() {
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
    // A late completion on the retired graph must work in its replacement.
    let sealed = old
        .agent_message_context("key-1")
        .seal_argument("collaboration.spawn_agent", "message", "synthetic task")
        .unwrap();
    let request =
        serde_json::json!({"input":[{"type":"agent_message","encrypted_content":sealed}]});
    let mut restored = request.clone();
    next.agent_message_context("key-1")
        .restore_request(&mut restored)
        .unwrap();
    assert_eq!(restored["input"][0]["content"][1]["text"], "synthetic task");
    assert!(
        next.agent_message_context("key-2")
            .restore_request(&mut request.clone())
            .is_err()
    );
    let restarted = bridge_runtime(
        Default::default(),
        "synthetic-secret",
        "https://example.test/v1",
    );
    assert!(
        restarted
            .agent_message_context("key-1")
            .restore_request(&mut request.clone())
            .is_err()
    );
}

#[test]
fn agent_messages_native_replay_restores_stored_arguments_without_mutating_client_output() {
    use serde_json::{Value, json};
    let runtime = bridge_runtime(
        Default::default(),
        "synthetic-secret",
        "https://example.test/v1",
    );
    let sealed = runtime
        .agent_message_context("key-1")
        .seal_argument("delegate", "message", "synthetic task")
        .unwrap();
    let response = json!({"id":"resp_synthetic_agent","status":"completed","output":[{
        "type":"function_call","id":"fc_synthetic","call_id":"call_synthetic","name":"delegate",
        "arguments":json!({"message":sealed}).to_string()
    }]});
    runtime.capture_native_responses_replay(
        "key-1",
        "source-1",
        &json!({"model":"gpt-test","input":"delegate a task"}),
        "gpt-test",
        &response,
        10,
    );
    let replay = runtime
        .load_native_responses_replay("key-1", "resp_synthetic_agent", "source-1", 11)
        .unwrap();
    let next = replay
        .replay_request(
            &json!({"input":[{"type":"function_call_output","call_id":"call_synthetic","output":"started"}]}),
            "gpt-test",
            false,
        )
        .unwrap();
    let arguments: Value =
        serde_json::from_str(next["input"][1]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments["message"], "synthetic task");
    assert!(!next.to_string().contains("zenith-relay-agent-v1:"));
    assert!(response.to_string().contains("zenith-relay-agent-v1:"));
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
