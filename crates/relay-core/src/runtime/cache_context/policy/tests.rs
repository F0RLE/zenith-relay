use super::*;
use serde_json::json;

#[test]
fn omitted_controls_are_not_inferred_and_explicit_without_markers_is_preserved() {
    let request = json!({"input":[{"role":"user","content":"Synthetic context"}]});
    let omitted = observe(&request);
    assert_eq!(omitted.mode, CachePolicyMode::Omitted);
    assert_eq!(omitted.ttl, CachePolicyTtl::Omitted);
    assert_eq!(omitted.prewarm, CachePrewarm::Omitted);
    assert_eq!(omitted.key, CacheKeyPresence::Omitted);
    assert_eq!(omitted.breakpoints.explicit, 0);
    assert!(omitted.breakpoints.complete);

    let mut explicit = request;
    explicit["prompt_cache_options"] = json!({"mode":"explicit","ttl":"12h","prewarm":false});
    let original = explicit.clone();
    let observed = observe(&explicit);
    assert_eq!(observed.mode, CachePolicyMode::Explicit);
    assert_eq!(observed.ttl, CachePolicyTtl::TwelveHours);
    assert_eq!(observed.prewarm, CachePrewarm::Disabled);
    assert_eq!(observed.breakpoints.explicit, 0);
    assert_eq!(explicit, original);
}

#[test]
fn observations_allowlist_values_without_leaking_unknown_options_or_keys() {
    let observed = observe(&json!({
        "prompt_cache_key":"synthetic-secret-key",
        "prompt_cache_options":{"mode":"synthetic-private-value","ttl":"private-ttl",
            "prewarm":"private-value","comparison_response_id":"private-response"}
    }));
    assert_eq!(observed.mode, CachePolicyMode::Other);
    assert_eq!(observed.ttl, CachePolicyTtl::Other);
    assert_eq!(observed.prewarm, CachePrewarm::Other);
    assert_eq!(observed.key, CacheKeyPresence::Present);
    let encoded = serde_json::to_string(&observed).unwrap();
    for secret in ["synthetic", "private", "comparison_response_id"] {
        assert!(!encoded.contains(secret));
    }
    for malformed in [Value::Null, json!([]), json!(true), json!("implicit")] {
        let observed = observe(&json!({"prompt_cache_options":malformed,"prompt_cache_key":null}));
        assert_eq!(observed.mode, CachePolicyMode::Other);
        assert_eq!(observed.ttl, CachePolicyTtl::Other);
        assert_eq!(observed.prewarm, CachePrewarm::Other);
        assert_eq!(observed.key, CacheKeyPresence::Other);
    }
    assert_eq!(
        observe(&json!({"prompt_cache_key":" \t"})).key,
        CacheKeyPresence::Empty
    );
}

#[test]
fn counts_content_and_tool_output_markers_without_scanning_schemas_or_opaque_state() {
    let block = json!({"type":"input_text","text":"Synthetic",
        "prompt_cache_breakpoint":{"mode":"explicit","ttl":"12h"}});
    let request = json!({"input":[
        {"type":"additional_tools","prompt_cache_breakpoint":{"mode":"explicit"},
            "tools":[{"type":"function","name":"synthetic","parameters":block}]},
        {"role":"developer","content":[block]},
        {"type":"reasoning","content":[block],"encrypted_content":"synthetic-opaque"},
        {"type":"function_call_output","call_id":"synthetic-call","output":[block]},
        {"type":"custom_tool_call_output","call_id":"synthetic-custom","output":[
            {"type":"input_text","text":"Synthetic result","prompt_cache_breakpoint":{"mode":"explicit"}}]},
        {"role":"user","content":[{"type":"input_text","text":"Synthetic question",
            "prompt_cache_breakpoint":{"mode":"synthetic-unknown"}}]}
    ]});
    let observed = observe(&request);
    assert_eq!(observed.breakpoints.explicit, 3);
    assert_eq!(observed.breakpoints.ttl_overrides, 2);
    assert_eq!(observed.breakpoints.other, 1);
    assert_eq!(observed.breakpoints.last_input_item, Some(5));
    assert!(observed.breakpoints.complete);
}

#[test]
fn bounded_inspection_never_reports_a_partial_scan_as_complete() {
    let block = json!({"type":"input_text","text":"Synthetic"});
    let mut request = json!({"input":[{"role":"user","content":vec![block;MAX_INSPECTED_BLOCKS]}]});
    assert!(observe(&request).breakpoints.complete);
    request["input"][0]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "type":"input_text","text":"Synthetic","prompt_cache_breakpoint":{"mode":"explicit"}
        }));
    let observed = observe(&request);
    assert!(!observed.breakpoints.complete);
    assert_eq!(observed.breakpoints.explicit, 0);
}
