use super::*;
use serde_json::json;

#[test]
fn pool_default_applies_only_when_client_did_not_select_a_tier() {
    let mut request = json!({});
    let policy = ServiceTierPolicy::pool_owned(&request);

    policy.prepare_for_candidate(&mut request, DefaultServiceTier::Fast, WireApi::Responses);
    assert_eq!(request["service_tier"], "priority");

    policy.prepare_for_candidate(
        &mut request,
        DefaultServiceTier::Standard,
        WireApi::Responses,
    );
    assert!(request.get("service_tier").is_none());
    assert_eq!(
        policy.effective_tier(&request, DefaultServiceTier::Standard, WireApi::Responses),
        DefaultServiceTier::Standard
    );

    policy.prepare_for_candidate(&mut request, DefaultServiceTier::Fast, WireApi::Responses);
    assert_eq!(request["service_tier"], "priority");
    assert_eq!(
        policy.effective_tier(&request, DefaultServiceTier::Fast, WireApi::Responses),
        DefaultServiceTier::Fast
    );

    let mut explicit = json!({"service_tier": "flex"});
    let explicit_policy = ServiceTierPolicy::pool_owned(&explicit);
    explicit_policy.prepare_for_candidate(
        &mut explicit,
        DefaultServiceTier::Fast,
        WireApi::Responses,
    );
    assert_eq!(explicit["service_tier"], "flex");
    assert_eq!(
        explicit_policy.effective_tier(&explicit, DefaultServiceTier::Fast, WireApi::Responses),
        DefaultServiceTier::Standard
    );

    explicit_policy.prepare_for_candidate(
        &mut explicit,
        DefaultServiceTier::Standard,
        WireApi::Responses,
    );
    assert_eq!(explicit["service_tier"], "flex");
}

#[test]
fn explicit_speed_survives_retries_independently_of_pool_defaults() {
    for value in ["fast", "priority", "ultrafast", "default", "flex"] {
        let mut request = json!({"service_tier": value});
        let policy = ServiceTierPolicy::pool_owned(&request);
        for default in [
            DefaultServiceTier::Fast,
            DefaultServiceTier::Standard,
            DefaultServiceTier::Ultrafast,
        ] {
            policy.prepare_for_candidate(&mut request, default, WireApi::Responses);
            assert_eq!(request["service_tier"], value);
        }
    }
}

#[test]
fn pool_native_fast_spelling_is_preserved_when_the_candidate_matches() {
    for value in ["fast", "priority"] {
        let mut request = json!({"service_tier": value});
        let policy = ServiceTierPolicy::pool_owned(&request);
        policy.prepare_for_candidate(&mut request, DefaultServiceTier::Fast, WireApi::Responses);
        assert_eq!(request["service_tier"], value);
    }
}

#[test]
fn client_owned_service_tier_preserves_explicit_value() {
    let mut request = json!({"service_tier": "flex"});
    let policy = ServiceTierPolicy::client_owned(&request);

    policy.prepare_for_candidate(&mut request, DefaultServiceTier::Fast, WireApi::Responses);
    assert_eq!(request["service_tier"], "flex");
    assert_eq!(
        policy.effective_tier(&request, DefaultServiceTier::Fast, WireApi::Responses),
        DefaultServiceTier::Standard
    );

    let mut implicit = json!({});
    let implicit_policy = ServiceTierPolicy::client_owned(&implicit);
    implicit_policy.prepare_for_candidate(
        &mut implicit,
        DefaultServiceTier::Fast,
        WireApi::Responses,
    );
    assert!(implicit.get("service_tier").is_none());
}

#[test]
fn pool_owned_service_tier_does_not_inject_into_messages() {
    let mut request = json!({"service_tier": "priority"});
    let policy = ServiceTierPolicy::pool_owned(&request);

    policy.prepare_for_candidate(&mut request, DefaultServiceTier::Fast, WireApi::Messages);
    assert_eq!(request["service_tier"], "priority");
    assert_eq!(
        policy.effective_tier(&request, DefaultServiceTier::Fast, WireApi::Messages),
        DefaultServiceTier::Fast
    );
}

#[test]
fn pool_owned_service_tier_injects_ultrafast_and_tracks_it() {
    let mut request = json!({});
    let policy = ServiceTierPolicy::pool_owned(&request);

    policy.prepare_for_candidate(
        &mut request,
        DefaultServiceTier::Ultrafast,
        WireApi::Responses,
    );
    assert_eq!(request["service_tier"], "ultrafast");
    assert_eq!(
        policy.effective_tier(&request, DefaultServiceTier::Ultrafast, WireApi::Responses),
        DefaultServiceTier::Ultrafast
    );

    policy.prepare_for_candidate(
        &mut request,
        DefaultServiceTier::Standard,
        WireApi::Responses,
    );
    assert!(request.get("service_tier").is_none());
}

#[test]
fn empty_context_management_is_omitted_but_non_empty_policy_is_preserved() {
    for value in [Value::Null, json!([]), json!({})] {
        let mut request = json!({"context_management": value});
        normalize_account_request(request.as_object_mut().unwrap(), false);
        assert!(request.get("context_management").is_none());
    }

    let mut request = json!({
        "context_management": [{"type": "compaction", "compact_threshold": 1_000}]
    });
    normalize_basis_points_request(request.as_object_mut().unwrap());
    assert_eq!(
        request["context_management"],
        json!([{"type": "compaction", "compact_threshold": 1_000}])
    );
}

fn const_branches(values: &[Value]) -> Value {
    Value::Array(
        values
            .iter()
            .map(|value| json!({"const": value, "description": "choice"}))
            .collect(),
    )
}

#[test]
fn codex_tool_schema_normalization_flattens_large_pure_unions() {
    let values: Vec<Value> = (0..8).map(|value| json!(value)).collect();
    let mut request = json!({
        "tools": [{
            "type": "function",
            "name": "lookup",
            "parameters": {
                "type": "object",
                "properties": {
                    "kind": {"oneOf": const_branches(&values)}
                }
            }
        }]
    });

    normalize_account_request(request.as_object_mut().unwrap(), false);

    assert_eq!(
        request["tools"][0]["parameters"]["properties"]["kind"]["enum"],
        Value::Array(values)
    );
    assert!(request["tools"][0]["parameters"]["properties"]["kind"]
        .get("oneOf")
        .is_none());
}

#[test]
fn codex_tool_schema_normalization_inlines_local_definitions() {
    let mut request = json!({
        "tools": [{
            "type": "function",
            "name": "lookup",
            "parameters": {
                "type": "object",
                "$defs": {
                    "call": {
                        "type": "object",
                        "properties": {
                            "localId": {"type": "string"}
                        },
                        "required": ["localId"]
                    }
                },
                "properties": {
                    "edits": {
                        "anyOf": [{
                            "type": "array",
                            "items": {
                                "anyOf": [
                                    {"type": "null"},
                                    {"$ref": "#/$defs/call"}
                                ]
                            }
                        }]
                    }
                }
            }
        }]
    });

    normalize_account_request(request.as_object_mut().unwrap(), false);

    let parameters = &request["tools"][0]["parameters"];
    assert!(parameters.get("$defs").is_none());
    assert_eq!(
        parameters["properties"]["edits"]["anyOf"][0]["items"]["anyOf"][1],
        json!({
            "type": "object",
            "properties": {
                "localId": {"type": "string"}
            },
            "required": ["localId"]
        })
    );
}

#[test]
fn codex_tool_schema_normalization_preserves_unresolved_local_refs() {
    let mut request = json!({
        "tools": [{
            "type": "function",
            "name": "lookup",
            "parameters": {
                "type": "object",
                "$defs": {
                    "known": {"type": "string"}
                },
                "properties": {
                    "value": {"$ref": "#/$defs/missing"}
                }
            }
        }]
    });

    normalize_account_request(request.as_object_mut().unwrap(), false);

    let parameters = &request["tools"][0]["parameters"];
    assert_eq!(parameters["properties"]["value"]["$ref"], "#/$defs/missing");
    assert_eq!(parameters["$defs"]["known"]["type"], "string");
}

#[test]
fn compact_normalization_removes_transport_fields_and_preserves_new_fields() {
    let mut request = json!({
        "store": false,
        "stream": false,
        "max_output_tokens": 4,
        "model": "gpt-test",
        "input": "compact this",
        "reasoning": {"effort": "high"},
        "future_compaction_option": {"enabled": true}
    });

    normalize_compact_account_request(request.as_object_mut().unwrap(), false);

    assert!(request.get("store").is_none());
    assert!(request.get("stream").is_none());
    assert!(request.get("max_output_tokens").is_none());
    assert_eq!(request["reasoning"]["effort"], "high");
    assert_eq!(request["future_compaction_option"]["enabled"], true);
    assert_eq!(request["input"][0]["content"][0]["text"], "compact this");
}

#[test]
fn codex_tool_schema_normalization_reaches_nested_namespace_tools() {
    let values: Vec<Value> = (0..8)
        .map(|value| json!(format!("choice-{value}")))
        .collect();
    let mut request = json!({
        "tools": [{
            "type": "namespace",
            "tools": [{
                "type": "custom",
                "name": "nested",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "kind": {"anyOf": const_branches(&values)}
                    }
                }
            }]
        }]
    });

    normalize_account_request(request.as_object_mut().unwrap(), false);

    assert_eq!(
        request["tools"][0]["tools"][0]["parameters"]["properties"]["kind"]["enum"],
        Value::Array(values)
    );
}

#[test]
fn codex_tool_schema_normalization_preserves_non_pure_or_duplicate_unions() {
    let values: Vec<Value> = (0..8).map(|value| json!(value)).collect();
    let mut request = json!({
        "tools": [{
            "type": "function",
            "parameters": {
                "properties": {
                    "constrained": {"oneOf": [
                        {"const": "a"}, {"const": "b", "type": "string"},
                        {"const": "c"}, {"const": "d"}, {"const": "e"},
                        {"const": "f"}, {"const": "g"}, {"const": "h"}
                    ]},
                    "duplicate": {"oneOf": const_branches(&[
                        values[0].clone(), values[1].clone(), values[2].clone(), values[3].clone(),
                        values[4].clone(), values[5].clone(), values[6].clone(), values[6].clone()
                    ])}
                }
            }
        }]
    });
    let original = request["tools"].clone();

    normalize_account_request(request.as_object_mut().unwrap(), false);

    assert_eq!(request["tools"], original);
}

#[test]
fn codex_tool_schema_normalization_treats_equivalent_numbers_as_duplicates() {
    let mut request = json!({
        "tools": [{
            "type": "function",
            "parameters": {
                "properties": {
                    "kind": {"oneOf": [
                        {"const": 1}, {"const": 1.0}, {"const": 2}, {"const": 3},
                        {"const": 4}, {"const": 5}, {"const": 6}, {"const": 7}
                    ]}
                }
            }
        }]
    });

    normalize_account_request(request.as_object_mut().unwrap(), false);

    assert!(request["tools"][0]["parameters"]["properties"]["kind"]
        .get("enum")
        .is_none());
    assert!(request["tools"][0]["parameters"]["properties"]["kind"]
        .get("oneOf")
        .is_some());
}
