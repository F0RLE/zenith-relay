use super::*;
use crate::{
    GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeLocalKey, RuntimeSource, WireApi,
};
use serde_json::json;
use std::sync::Arc;

fn runtime() -> GatewayRuntime {
    runtime_for(WireApi::Responses)
}

fn runtime_for(wire_api: WireApi) -> GatewayRuntime {
    GatewayRuntime::from_pool(
        ["source", "other-source"]
            .into_iter()
            .map(|id| {
                RuntimeSource::unrestricted(ProviderSource {
                    id: id.into(),
                    name: "Test".into(),
                    base_url: "http://127.0.0.1:9/v1".into(),
                    api_key: "synthetic".into(),
                    wire_api,
                    models: vec!["model".into()],
                    proxy_url: None,
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
fn interactions_continuations_require_the_original_key_model_and_exact_route() {
    let runtime = runtime_for(WireApi::GeminiInteractions);
    let route = runtime
        .executor_route(
            "source",
            "model",
            &Default::default(),
            &[WireApi::GeminiInteractions],
            false,
        )
        .unwrap();
    let request =
        json!({"model":"model","previous_interaction_id":"synthetic-interaction","input":"next"});
    assert!(interactions::prepare(&runtime, "key", &request, 10).is_err());
    interactions::bind_response(
        &runtime,
        "key",
        &route,
        &json!({"id":"synthetic-interaction","status":"completed","outputs":[]}),
        10,
    );
    assert!(interactions::prepare(&runtime, "key", &request, 10).is_ok());
    assert!(interactions::prepare(&runtime, "other-key", &request, 10).is_err());
    assert!(runtime
        .validate_continuation_route(&route, "key", &request, 10)
        .is_ok());
    let mut changed = route.clone();
    changed.source_model = "other-model".into();
    assert!(runtime
        .validate_continuation_route(&changed, "key", &request, 10)
        .is_err());
    let other = runtime
        .executor_route(
            "other-source",
            "model",
            &Default::default(),
            &[WireApi::GeminiInteractions],
            false,
        )
        .unwrap();
    assert!(runtime
        .validate_continuation_route(&other, "key", &request, 10)
        .is_err());
    assert!(runtime
        .validate_continuation_route(&route, "other-key", &request, 10)
        .is_err());
}

#[test]
fn gemini_continuations_keep_the_original_owner_caller_model_and_contents() {
    let runtime = runtime_for(WireApi::Gemini);
    let original = json!({"model":"model","contents":[{"role":"user","parts":[{"text":"Original prompt"}]}],"continuationToken":"c3ludGhldGlj"});
    let reference = gemini::request_reference("key", &original)
        .unwrap()
        .unwrap();
    let route = runtime
        .executor_route(
            "source",
            "model",
            &Default::default(),
            &[WireApi::Gemini],
            false,
        )
        .unwrap();
    let response = json!({"candidates":[{"index":0,"finishReason":"CONTINUATION","continuationToken":"c3ludGhldGlj"}]});
    gemini::bind_response(&runtime, "key", &route, &original, &response, 10);
    assert!(runtime.has_response_affinity_binding(
        &runtime.response_affinity_key(Some(&reference)).unwrap(),
        10
    ));
    let mut request = original.clone();
    let state = gemini::prepare(&runtime, "key", &request, 10).unwrap();
    assert!(state.requires_affinity_owner);
    assert_eq!(
        runtime
            .response_affinity_candidate(state.response_affinity_key.as_deref().unwrap(), 10)
            .as_deref(),
        Some("source")
    );
    assert!(!prepare_response_owner_change(
        &runtime,
        "key",
        &mut request,
        "model",
        true,
        10
    ));
    assert!(gemini::prepare(&runtime, "another-key", &request, 10).is_err());
    for (field, value) in [
        ("model", json!("other-model")),
        (
            "contents",
            json!([{"role":"user","parts":[{"text":"Changed prompt"}]}]),
        ),
        ("continuationToken", json!("b3RoZXI=")),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        assert!(gemini::prepare(&runtime, "key", &changed, 10).is_err());
    }
    assert_eq!(request, original);
}

#[test]
fn owned_predecessor_disambiguates_reused_call_ids_without_materialized_replay() {
    let runtime = runtime();
    let route = |id| {
        runtime
            .executor_route(
                id,
                "model",
                &Default::default(),
                &[WireApi::Responses],
                false,
            )
            .unwrap()
    };
    let owner = route("source");
    let other = route("other-source");
    let initial = json!({"input":[
        // Provider-owned references cannot become a self-contained local replay.
        {"type":"item_reference","id":"synthetic-provider-owned-item"},
        {"type":"reasoning","encrypted_content":"opaque-synthetic-state","summary":[]},
        {"role":"user","content":"Synthetic request"}
    ]});
    let response = json!({"id":"resp_encrypted_owner","status":"completed","output":[{
        "type":"function_call","call_id":"call_reused","name":"lookup","arguments":"{}"
    }]});
    runtime.capture_native_responses_replay("key", "source", &initial, "model", &response, 10);
    assert!(
        runtime
            .load_native_responses_replay("key", "resp_encrypted_owner", "source", 10)
            .is_none(),
        "provider-owned references must not become portable replay"
    );
    runtime.bind_response_route_affinity(Some("resp_encrypted_owner"), &owner, 10);
    runtime.bind_tool_call_route_affinity("key", "call_reused", &other, 11);
    runtime.bind_tool_call_route_affinity("other-key", "call_reused", &other, 11);
    let mut request = json!({"model":"model","previous_response_id":"resp_encrypted_owner",
        "input":[{"type":"function_call_output","call_id":"call_reused","output":"Synthetic result"}]});
    assert!(runtime
        .validate_continuation_route(&owner, "key", &request, 12)
        .is_ok());
    assert!(runtime
        .validate_continuation_route(&other, "key", &request, 12)
        .is_err());
    assert!(runtime
        .validate_continuation_route(&owner, "other-key", &request, 12)
        .is_err());
    let mut other_model = owner.clone();
    other_model.source_model = "other-model".into();
    assert!(runtime
        .validate_continuation_route(&other_model, "key", &request, 12)
        .is_err());
    request["input"][0]["call_id"] = json!("call_foreign");
    runtime.bind_tool_call_route_affinity("key", "call_foreign", &other, 11);
    assert!(runtime
        .validate_continuation_route(&owner, "key", &request, 12)
        .is_err());

    request["input"][0]["call_id"] = json!("call_reused");
    let mut replacement = response.clone();
    replacement["output"][0]["call_id"] = json!("call_replacement");
    runtime.capture_native_responses_replay("key", "source", &initial, "model", &replacement, 13);
    assert!(
        runtime
            .validate_continuation_route(&owner, "key", &request, 14)
            .is_err(),
        "a reused response ID must not accumulate tool-call permissions"
    );
    runtime.capture_native_responses_replay("key", "source", &initial, "model", &response, 15);
    assert!(runtime
        .validate_continuation_route(&owner, "key", &request, 16)
        .is_ok());

    // Renewing route hints must not revive expired call-ownership evidence.
    let expired = crate::RESPONSE_AFFINITY_TTL_MS + 16;
    runtime.bind_response_route_affinity(Some("resp_encrypted_owner"), &owner, expired);
    runtime.bind_tool_call_route_affinity("key", "call_reused", &other, expired);
    assert!(runtime
        .validate_continuation_route(&owner, "key", &request, expired)
        .is_err());
}

#[test]
fn saved_predecessor_disambiguates_a_reused_tool_call_id() {
    let runtime = runtime();
    let route = |id| {
        runtime
            .executor_route(
                id,
                "model",
                &Default::default(),
                &[WireApi::Responses],
                false,
            )
            .unwrap()
    };
    let owner = route("source");
    let other = route("other-source");
    let response = json!({"id":"resp_owned","status":"completed","output":[{
        "type":"function_call","call_id":"call_reused","name":"lookup","arguments":"{}"
    }]});
    runtime.capture_native_responses_replay(
        "key",
        "source",
        &json!({"input":"Synthetic first turn"}),
        "model",
        &response,
        10,
    );
    runtime.bind_response_route_affinity(Some("resp_owned"), &owner, 10);
    // Another conversation can report the same provider-supplied call ID.
    // Its global hint must not override this response's validated history.
    runtime.bind_tool_call_route_affinity("key", "call_reused", &other, 11);
    let request = json!({
        "model":"model","previous_response_id":"resp_owned",
        "input":[{"type":"function_call_output","call_id":"call_reused","output":"Synthetic result"}]
    });
    assert!(runtime
        .validate_continuation_route(&owner, "key", &request, 12)
        .is_ok());
    assert!(runtime
        .validate_continuation_route(&other, "key", &request, 12)
        .is_err());
    runtime.bind_tool_call_route_affinity("other-key", "call_reused", &other, 11);
    assert!(runtime
        .validate_continuation_route(&owner, "other-key", &request, 12)
        .is_err());

    let mut orphan = request.clone();
    orphan["input"][0]["call_id"] = json!("call_foreign");
    runtime.bind_tool_call_route_affinity("key", "call_foreign", &other, 11);
    assert!(runtime
        .validate_continuation_route(&owner, "key", &orphan, 12)
        .is_err());
    runtime.bind_response_route_affinity(Some("resp_missing_history"), &owner, 10);
    let mut missing = request;
    missing["previous_response_id"] = json!("resp_missing_history");
    assert!(runtime
        .validate_continuation_route(&owner, "key", &missing, 12)
        .is_err());
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
fn owner_change_releases_paired_history_without_rewriting_it() {
    let runtime = runtime();
    for (call_kind, output_kind) in [
        ("function_call", "function_call_output"),
        ("custom_tool_call", "custom_tool_call_output"),
    ] {
        runtime.bind_tool_call_affinity("key", "paired", "source", 10);
        let mut request = json!({"model":"model", "input":[
            {"type":call_kind, "call_id":"paired", "name":"lookup", "arguments":"{}", "input":"query"},
            {"type":output_kind, "call_id":"paired", "output":"synthetic result"}
        ]});
        let original = request.clone();
        let state = prepare_response_continuation(&runtime, "key", &mut request, 10, None).unwrap();
        assert!(state.response_affinity_key.is_some());
        assert!(!state.requires_affinity_owner);
        assert!(prepare_response_owner_change(
            &runtime,
            "key",
            &mut request,
            "model",
            state.requires_affinity_owner,
            10,
        ));
        assert_eq!(request, original);
    }
}

#[test]
fn owner_change_cannot_release_opaque_or_unpaired_state_even_with_a_soft_flag() {
    let runtime = runtime();
    runtime.bind_response_affinity(Some("resp_opaque"), "source", 10);
    runtime.bind_tool_call_affinity("key", "unpaired", "source", 10);
    for requires_owner in [true, false] {
        for mut request in [
            json!({"previous_response_id":"resp_opaque", "input":"continue"}),
            json!({"input":[{"type":"function_call_output", "call_id":"unpaired", "output":"result"}]}),
            json!({"input":[{"type":"custom_tool_call_output", "call_id":"unpaired", "output":"result"}]}),
        ] {
            let original = request.clone();
            assert!(!prepare_response_owner_change(
                &runtime,
                "key",
                &mut request,
                "model",
                requires_owner,
                10,
            ));
            assert_eq!(request, original);
        }
    }
}

#[test]
fn owner_change_materializes_native_tool_history_without_resetting_cache_policy() {
    for (call, result) in [
        (
            json!({"type":"function_call", "id":"fc_native", "call_id":"call_native",
                "name":"lookup", "arguments":"{\"query\":\"synthetic\"}"}),
            json!({"type":"function_call_output", "call_id":"call_native", "output":"result"}),
        ),
        (
            json!({"type":"custom_tool_call", "id":"ctc_native", "call_id":"call_native",
                "name":"lookup", "input":"synthetic"}),
            json!({"type":"custom_tool_call_output", "call_id":"call_native", "output":"result"}),
        ),
    ] {
        let runtime = runtime();
        let initial = json!({"input":[{"role":"user", "content":"keep the initial constraint"}]});
        let reasoning = json!({"type":"reasoning", "id":"rs_native", "summary":[],
            "encrypted_content":"synthetic-completed-ciphertext"});
        runtime.capture_native_responses_replay(
            "key",
            "source",
            &initial,
            "model",
            &json!({"id":"resp_native", "status":"completed", "output":[reasoning, call]}),
            10,
        );
        runtime.bind_response_affinity(Some("resp_native"), "source", 10);
        let mut request = json!({
            "model":"model", "previous_response_id":"resp_native", "input":[result],
            "instructions":"current instructions", "prompt_cache_key":"synthetic-chat",
            "prompt_cache_options":{"mode":"implicit", "ttl":"30m"},
            "reasoning":{"effort":"high"}, "parallel_tool_calls":false,
            "tools":[{"type":"function", "name":"current_tool", "parameters":{"type":"object"}}]
        });
        let original = request.clone();
        assert!(prepare_response_owner_change(
            &runtime,
            "key",
            &mut request,
            "model",
            true,
            11,
        ));
        let mut expected = original;
        expected
            .as_object_mut()
            .unwrap()
            .remove("previous_response_id");
        expected["stream"] = json!(false);
        expected["input"] = json!([initial["input"][0], reasoning, call, result]);
        assert_eq!(request, expected);
    }
}

#[test]
fn native_owner_change_keeps_unavailable_foreign_or_unpaired_history_bound() {
    let runtime = runtime();
    runtime.capture_native_responses_replay(
        "key",
        "source",
        &json!({"input":"initial context"}),
        "model",
        &json!({"id":"resp_native", "status":"completed", "output":[
            {"type":"reasoning", "encrypted_content":"synthetic-ciphertext", "summary":[]},
            {"type":"function_call", "call_id":"call_native", "name":"lookup", "arguments":"{}"}
        ]}),
        10,
    );
    runtime.bind_response_affinity(Some("resp_native"), "source", 10);
    let base = json!({"model":"model", "previous_response_id":"resp_native", "input":[
        {"type":"function_call_output", "call_id":"call_native", "output":"result"}
    ]});
    let mut cases = vec![
        ("other-key", "model", 11, base.clone()),
        ("key", "different-model", 11, base.clone()),
        (
            "key",
            "model",
            11 + crate::RESPONSE_AFFINITY_TTL_MS,
            base.clone(),
        ),
    ];
    for (field, value) in [
        ("previous_response_id", json!("resp_unknown")),
        ("conversation", json!("conv_unknown")),
        ("context_management", json!([])),
        ("truncation", json!("auto")),
        (
            "input",
            json!([{"type":"item_reference", "id":"item_unknown"}]),
        ),
        (
            "input",
            json!([{"type":"function_call_output", "call_id":"wrong", "output":"result"}]),
        ),
    ] {
        let mut request = base.clone();
        request[field] = value;
        cases.push(("key", "model", 11, request));
    }
    // Run expiration last: the bounded store may evict expired entries.
    cases.sort_by_key(|(_, _, now, _)| *now);
    for (key, model, now, mut request) in cases {
        let original = request.clone();
        assert!(!prepare_response_owner_change(
            &runtime,
            key,
            &mut request,
            model,
            true,
            now
        ));
        assert_eq!(request, original);
    }
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
