use super::*;
use serde_json::json;

#[test]
fn tool_policy_snapshot_survives_hot_updates_and_retry_clones() {
    let runtime = capability_test_runtime(&["synthetic"], GatewayRuntimeOptions::default());
    runtime.set_tool_policy(crate::ToolPolicy {
        mode: crate::ToolPolicyMode::Automatic,
    });
    let original =
        json!({"tools":[{"type":"function","name":"keep"},{"type":"function","name":"drop"}]});
    let snapshot = RequestToolPolicy::new(&runtime, &original);
    runtime.set_tool_policy(crate::ToolPolicy::default());
    for mut attempt in [snapshot.clone(), snapshot] {
        let mut body = original.clone();
        attempt.apply(&mut body).unwrap();
        assert_eq!(body["tools"].as_array().unwrap().len(), 2);
        assert_eq!(attempt.diagnostics.client_tool_count, 2);
        assert_eq!(attempt.diagnostics.filtered_tool_count, 0);
        attempt.apply(&mut body).unwrap();
        assert_eq!(attempt.diagnostics.filtered_tool_count, 0);
        assert_eq!(
            attempt.diagnostics.policy_outcome,
            Some(crate::ToolPolicyOutcome::PassThrough)
        );
    }
    let mut normalized_body = original.clone();
    RequestToolPolicy::new(&runtime, &original)
        .apply(&mut normalized_body)
        .unwrap();
    assert_eq!(normalized_body, original);
}
#[test]
fn saved_automatic_policy_does_not_defer_native_responses() {
    let runtime = automatic_tool_policy_test_runtime();
    let original = two_function_tools();
    let mut policy = RequestToolPolicy::new(&runtime, &original);

    let body = original.clone();
    policy.apply_value(&body).unwrap();
    assert_eq!(body, original);
    assert!(!policy.diagnostics.deferred_tool_search);
    assert_eq!(
        policy.diagnostics.policy_outcome,
        Some(crate::ToolPolicyOutcome::PassThrough)
    );
    assert!(!policy.diagnostics.policy_fallback);
}
#[test]
fn direct_non_responses_account_endpoint_keeps_the_catalog_unchanged() {
    let runtime = automatic_tool_policy_test_runtime();
    let original = two_function_tools();
    let mut policy = RequestToolPolicy::new(&runtime, &original);
    let body = original.clone();
    policy.apply_value(&body).unwrap();
    assert_eq!(body, original);
    assert!(!policy.diagnostics.deferred_tool_search);
}
#[test]
fn tool_diagnostics_count_codex_tool_definitions_without_names() {
    let request = json!({
        "tools": [
            {"type": "function", "name": "read_private_file"},
            {"type": "namespace", "name": "collaboration", "tools": [
                {"type": "function", "name": "spawn_agent"},
                {"type": "function", "name": "wait_agent"}
            ]}
        ],
        "input": [{
            "type": "additional_tools",
            "tools": [{"type": "custom", "name": "apply_patch"}]
        }],
        "response": {
            "tools": [{"type": "function", "name": "hidden_function"}]
        },
        "tool_choice": {"type": "allowed_tools", "tools": []}
    });

    let diagnostics = tool_use_diagnostics(&request, true);
    let runtime = capability_test_runtime(&["synthetic"], GatewayRuntimeOptions::default());
    let mut policy = RequestToolPolicy::new(&runtime, &request);
    policy.apply(&mut request.clone()).unwrap();
    let forwarded = policy.diagnostics;

    assert_eq!(diagnostics.client_tool_count, 5);
    assert_eq!(diagnostics.tool_choice, ToolChoiceMode::AllowedTools);
    assert_eq!(forwarded.forwarded_tool_count, 5);
    assert!(
        !serde_json::to_string(&forwarded)
            .unwrap()
            .contains("read_private_file")
    );
}
#[test]
fn responses_lite_keeps_provider_owned_tools_and_choices_opaque_and_serializes_tools() {
    let mut request = json!({
        "model": "gpt-lite",
        "tools": [
            {"type": "function", "name": "lookup"},
            {"type": "namespace", "name": "collaboration", "tools": [
                {"name": "spawn_agent"}
            ]},
            {"type": "web_search"},
            {"type": "future_client_tool", "name": "future_tool"}
        ],
        "tool_choice": {
            "type": "allowed_tools",
            "mode": "required",
            "tools": [
                {"type": "function", "name": "lookup"},
                {"type": "web_search"}
            ]
        },
        "input": [
            {"type": "additional_tools", "tools": [
                {"type": "custom", "name": "patch"},
                {"type": "image_generation"}
            ]},
            {"role": "user", "content": "hello"}
        ]
    });
    let original_tools = request["tools"].clone();
    let original_choice = request["tool_choice"].clone();
    let original_input = request["input"].clone();

    normalize_account_request(request.as_object_mut().unwrap(), true);

    assert_eq!(request["tools"], original_tools);
    assert_eq!(request["tool_choice"], original_choice);
    assert_eq!(request["input"], original_input);
    assert_eq!(request["reasoning"]["context"], "all_turns");
    assert_eq!(request["parallel_tool_calls"], false);

    let mut no_tools = json!({"model": "gpt-lite"});
    normalize_account_request(no_tools.as_object_mut().unwrap(), true);
    assert_eq!(no_tools["parallel_tool_calls"], false);
}

#[test]
fn tool_diagnostics_preserve_explicit_parallel_policy_without_inventing_defaults() {
    for value in [Value::Null, json!(false), json!(true), json!("invalid")] {
        for wrapped in [false, true] {
            let request = json!({"parallel_tool_calls":value});
            let request = if wrapped {
                json!({"response":request})
            } else {
                request
            };
            let diagnostics = tool_use_diagnostics(&request, true);
            assert_eq!(diagnostics.requested_parallel_tool_calls, value.as_bool());
            let wire = serde_json::to_value(&diagnostics).unwrap();
            assert_eq!(
                wire.get("requestedParallelToolCalls"),
                value.as_bool().map(Value::Bool).as_ref()
            );
            assert_eq!(diagnostics.upstream_tool_call_count, None);
            assert_eq!(diagnostics.tool_calls_ready_ms, None);
        }
    }
    assert_eq!(
        tool_use_diagnostics(&json!({}), true).requested_parallel_tool_calls,
        None
    );
}

#[test]
fn lite_defaults_do_not_overwrite_an_explicit_client_policy() {
    for parallel in [false, true] {
        let mut request = json!({"model":"synthetic", "parallel_tool_calls":parallel});
        normalize_account_request(request.as_object_mut().unwrap(), true);
        assert_eq!(request["parallel_tool_calls"], parallel);
    }
}
#[test]
fn tool_output_detection_covers_all_client_tool_result_shapes() {
    for output in [
        json!({"type": "function_call_output", "call_id": "call_function"}),
        json!({"type": "custom_tool_call_output", "call_id": "call_custom"}),
        json!({"type": "tool_search_output", "call_id": "call_search"}),
        json!({"type": "computer_call_output", "call_id": "call_future"}),
    ] {
        assert!(contains_tool_call_output(&json!({"input": [output]})));
    }
    assert!(!contains_tool_call_output(&json!({
        "input": [{"type": "custom_tool_call", "call_id": "call_custom"}]
    })));
    assert!(!contains_tool_call_output(&json!({
        "tools": [{"type":"function", "name":"inspect", "parameters":{
            "type":"object", "examples":[{"type":"function_call_output", "call_id":"example"}]
        }}],
        "input": "Inspect the provided example"
    })));
}
#[test]
fn detailed_capture_follows_debug_setting_without_changing_admitted_requests() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ENABLED: AtomicBool = AtomicBool::new(false);
    let runtime = capability_test_runtime(
        &["synthetic"],
        GatewayRuntimeOptions {
            diagnostics_enabled: || ENABLED.load(Ordering::Relaxed),
            ..Default::default()
        },
    );
    let request = json!({
        "input": "synthetic conversation",
        "tools": [{"type":"function","name":"fixture","parameters":{"type":"object"}}],
        "tool_choice": "required", "parallel_tool_calls": false
    });
    let mut admitted = RequestToolPolicy::new(&runtime, &request);
    assert!(admitted.diagnostics.client_context.is_none());
    ENABLED.store(true, Ordering::Relaxed);
    let mut debug = RequestToolPolicy::new(&runtime, &request);
    for (policy, enabled) in [(&mut admitted, false), (&mut debug, true)] {
        let mut upstream = request.clone();
        policy.apply(&mut upstream).unwrap();
        policy.diagnostics.capture_upstream_context(&upstream);
        assert_eq!(upstream, request);
        assert_eq!(policy.diagnostics.client_tool_count, 1);
        assert_eq!(policy.diagnostics.forwarded_tool_count, 1);
        assert_eq!(policy.diagnostics.tool_choice, ToolChoiceMode::Required);
        assert_eq!(
            policy.diagnostics.requested_parallel_tool_calls,
            Some(false)
        );
        assert_eq!(policy.diagnostics.client_schema_bytes.is_some(), enabled);
        assert_eq!(policy.diagnostics.forwarded_schema_bytes.is_some(), enabled);
        assert_eq!(policy.diagnostics.client_context.is_some(), enabled);
        assert_eq!(policy.diagnostics.upstream_context.is_some(), enabled);
    }
    ENABLED.store(false, Ordering::Relaxed);
    assert!(
        RequestToolPolicy::new(&runtime, &request)
            .diagnostics
            .client_context
            .is_none()
    );
}
