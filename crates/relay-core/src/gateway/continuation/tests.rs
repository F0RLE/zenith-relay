use super::*;
use crate::{
    GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeLocalKey, RuntimeSource, WireApi,
};
use serde_json::json;
use std::sync::Arc;

fn runtime() -> GatewayRuntime {
    GatewayRuntime::from_pool(
        ["source", "other-source"]
            .into_iter()
            .map(|id| {
                RuntimeSource::unrestricted(ProviderSource {
                    id: id.into(),
                    name: "Test".into(),
                    base_url: "http://127.0.0.1:9/v1".into(),
                    api_key: "synthetic".into(),
                    wire_api: WireApi::Responses,
                    models: vec!["model".into()],
                })
            })
            .collect(),
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "key".into(),
            secret: "synthetic-key".into(),
        })],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap()
}

#[test]
fn client_history_never_proves_an_opaque_predecessor_is_complete() {
    let runtime = runtime();
    for input in [
        json!([{"role":"assistant","content":"Acknowledged"},{"role":"user","content":"Apply the earlier constraints"}]),
        json!([{"role":"user","content":"Hello"},{"role":"assistant","content":"Hi"},{"role":"user","content":"Continue"}]),
    ] {
        let mut request = json!({"previous_response_id":"resp_unknown","input":input});
        let original = request.clone();
        assert!(prepare_response_continuation(&runtime, "key", &mut request, 10, None).is_err());
        assert!(!drop_materialized_previous_response_id(
            &runtime,
            "key",
            &mut request,
            "model",
            10
        ));
        assert_eq!(request, original);
        runtime.bind_response_affinity(Some("resp_unknown"), "source", 10);
        assert!(!drop_materialized_previous_response_id(
            &runtime,
            "key",
            &mut request,
            "model",
            10
        ));
        assert_eq!(request, original);
        runtime.invalidate_response_affinity(
            runtime
                .response_affinity_key(Some("resp_unknown"))
                .as_deref(),
        );
    }
}

#[test]
fn a_paired_historic_call_cannot_own_an_unknown_tool_output() {
    let runtime = runtime();
    runtime.bind_tool_call_affinity("key", "paired", "source", 10);
    let mut request = json!({"input":[
        {"type":"function_call", "call_id":"paired", "name":"lookup", "arguments":"{}"},
        {"type":"function_call_output", "call_id":"paired", "output":"old result"},
        {"type":"function_call_output", "call_id":"unknown", "output":"new result"}
    ]});
    assert!(prepare_response_continuation(&runtime, "key", &mut request, 10, None).is_err());
}

#[test]
fn unpaired_outputs_require_their_own_shared_owner() {
    let runtime = runtime();
    runtime.bind_tool_call_affinity("key", "historic", "source", 10);
    runtime.bind_tool_call_affinity("key", "current", "other-source", 10);
    runtime.bind_tool_call_affinity("key", "conflicting", "source", 10);
    let mut request = json!({"input":[
        {"type":"function_call", "call_id":"historic", "name":"lookup", "arguments":"{}"},
        {"type":"function_call_output", "call_id":"historic", "output":"old result"},
        {"type":"function_call_output", "call_id":"current", "output":"new result"}
    ]});
    let state = prepare_response_continuation(&runtime, "key", &mut request, 10, None).unwrap();
    assert!(state.requires_affinity_owner);
    assert_eq!(
        runtime
            .response_affinity_candidate(state.response_affinity_key.as_deref().unwrap(), 10)
            .as_deref(),
        Some("other-source")
    );
    request["input"].as_array_mut().unwrap().push(json!({
        "type":"function_call_output", "call_id":"conflicting", "output":"another result"
    }));
    assert!(prepare_response_continuation(&runtime, "key", &mut request, 10, None).is_err());
}

#[test]
fn tool_schema_examples_do_not_establish_or_require_ownership() {
    let runtime = runtime();
    let mut request = json!({
        "tools":[{"type":"function","name":"inspect","parameters":{
            "type":"object","examples":[{
                "type":"function_call", "call_id":"unknown", "name":"lookup", "arguments":"{}"
            }]
        }}],
        "input":[{"type":"function_call_output", "call_id":"unknown", "output":"new result"}]
    });
    assert!(prepare_response_continuation(&runtime, "key", &mut request, 10, None).is_err());

    request["tools"][0]["parameters"]["examples"] = request["input"].clone();
    request["input"] = json!([{"role":"user", "content":"Inspect the provided example"}]);
    let state = prepare_response_continuation(&runtime, "key", &mut request, 10, None).unwrap();
    assert!(!state.requires_affinity_owner);
}

#[test]
fn ownership_validation_checks_tool_outputs_beyond_sixteen_items() {
    let runtime = runtime();
    let mut input = Vec::new();
    for index in 0..17 {
        input.push(json!({"type":"function_call", "call_id":format!("call_{index}"), "name":"lookup", "arguments":"{}"}));
        input.push(json!({"type":"function_call_output", "call_id":format!("call_{index}"), "output":"value"}));
    }
    let mut complete = json!({"input":input});
    assert!(
        !prepare_response_continuation(&runtime, "key", &mut complete, 10, None)
            .unwrap()
            .requires_affinity_owner
    );
    input.push(json!({"type":"function_call_output", "call_id":"unknown", "output":"value"}));
    let mut incomplete = json!({"input":input});
    assert!(prepare_response_continuation(&runtime, "key", &mut incomplete, 10, None).is_err());
}

#[test]
fn saved_plaintext_chain_preserves_early_context_during_model_switch() {
    let runtime = runtime();
    let first = json!({"model":"model","input":"Remember the initial constraint"});
    let output = json!({"id":"resp_saved","output":[{"type":"message","role":"assistant","content":"Acknowledged"}]});
    runtime.capture_native_responses_replay("key", "source", &first, "model", &output, 10);
    runtime.bind_response_affinity(Some("resp_saved"), "source", 10);
    let mut request =
        json!({"previous_response_id":"resp_saved","input":"Continue","model":"new-model"});
    let original = request.clone();
    assert!(!drop_materialized_previous_response_id(
        &runtime,
        "other-key",
        &mut request,
        "new-model",
        10
    ));
    assert_eq!(request, original);
    assert!(drop_materialized_previous_response_id(
        &runtime,
        "key",
        &mut request,
        "new-model",
        10
    ));
    assert_eq!(request["model"], "new-model");
    assert!(request.get("previous_response_id").is_none());
    assert_eq!(
        request["input"][0]["content"][0]["text"],
        "Remember the initial constraint"
    );
    assert_eq!(request["input"][1], output["output"][0]);
    assert_eq!(request["input"][2]["content"][0]["text"], "Continue");
}

#[test]
fn stale_custom_tool_recovery_replays_and_removes_only_the_unanswered_call() {
    let runtime = runtime();
    let first = json!({
        "model":"model",
        "input":[{"type":"message","role":"user","content":"start"}]
    });
    let output = json!({
        "id":"resp_stale_tool",
        "output":[
            {"type":"function_call","call_id":"fc_done","name":"lookup","arguments":"{}"},
            {"type":"function_call_output","call_id":"fc_done","output":"keep this result"},
            {"type":"custom_tool_call","id":"item_stale","call_id":"ctc_stale","name":"patch","input":"{}"}
        ]
    });
    runtime.capture_native_responses_replay("key", "source", &first, "model", &output, 10);
    runtime.bind_response_affinity(Some("resp_stale_tool"), "source", 10);

    let mut request = json!({
        "previous_response_id":"resp_stale_tool",
        "model":"model",
        "input":[{"type":"message","role":"user","content":"continue"}]
    });
    let error = br#"{"error":{"message":"No tool output found for custom tool call ctc_stale."}}"#;

    assert!(recover_stale_tool_history(
        &runtime,
        "key",
        &mut request,
        "model",
        10,
        false,
        error,
    ));
    assert!(request.get("previous_response_id").is_none());
    assert_eq!(request["model"], "model");
    assert_eq!(request["input"].as_array().unwrap().len(), 4);
    assert_eq!(request["input"][1]["call_id"], "fc_done");
    assert_eq!(request["input"][2]["output"], "keep this result");
    assert_eq!(request["input"][3]["content"], "continue");
}

#[test]
fn plaintext_recovery_rejects_saved_tools_and_encrypted_state() {
    for output in [
        json!([{"type":"function_call","call_id":"call_1","name":"lookup","arguments":"{}"}]),
        json!([{"type":"reasoning","encrypted_content":"synthetic","summary":[]}]),
    ] {
        let runtime = runtime();
        runtime.capture_native_responses_replay(
            "key",
            "source",
            &json!({"input":"start"}),
            "model",
            &json!({"id":"resp_saved","output":output}),
            10,
        );
        runtime.bind_response_affinity(Some("resp_saved"), "source", 10);
        let mut request = json!({"previous_response_id":"resp_saved","input":"continue"});
        let original = request.clone();
        assert!(!drop_materialized_previous_response_id(
            &runtime,
            "key",
            &mut request,
            "new-model",
            10
        ));
        assert_eq!(request, original);
    }
}
