use super::*;
use serde_json::json;

#[test]
fn service_tier_defaults_inject_speed_without_overriding_client_choice() {
    let mut request = json!({});
    apply_default_service_tier_if_missing(&mut request, DefaultServiceTier::Fast);
    assert_eq!(request["service_tier"], "priority");
    assert_eq!(request_service_tier(&request), DefaultServiceTier::Fast);

    let mut ultrafast = json!({});
    apply_default_service_tier_if_missing(&mut ultrafast, DefaultServiceTier::Ultrafast);
    assert_eq!(ultrafast["service_tier"], "ultrafast");
    assert_eq!(
        request_service_tier(&ultrafast),
        DefaultServiceTier::Ultrafast
    );

    let mut standard = json!({});
    apply_default_service_tier_if_missing(&mut standard, DefaultServiceTier::Standard);
    assert!(standard.get("service_tier").is_none());

    let mut client_selected = json!({"service_tier": "flex"});
    apply_default_service_tier_if_missing(&mut client_selected, DefaultServiceTier::Fast);
    assert_eq!(client_selected["service_tier"], "flex");

    assert_eq!(
        request_service_tier(&json!({"service_tier": "priority"})),
        DefaultServiceTier::Fast
    );
    assert_eq!(
        request_service_tier(&json!({"service_tier": "fast"})),
        DefaultServiceTier::Fast
    );
    assert_eq!(
        request_service_tier(&json!({"service_tier": "ultrafast"})),
        DefaultServiceTier::Ultrafast
    );
    for tier in [None, Some("standard"), Some("default"), Some("flex")] {
        let request = tier.map_or_else(|| json!({}), |tier| json!({"service_tier": tier}));
        assert_eq!(
            request_service_tier(&request),
            DefaultServiceTier::Standard,
            "{tier:?} must remain a non-fast client tier"
        );
    }
}
#[test]
fn native_account_reasoning_and_speed_selections_are_opaque() {
    let mut request = json!({
        "model": "gpt-5.6-terra",
        "service_tier": "flex",
        "reasoning": {
            "effort": "ultra",
            "summary": "detailed",
            "context": "client_selected"
        }
    });

    normalize_account_request(request.as_object_mut().unwrap(), false);

    assert_eq!(request["service_tier"], "flex");
    assert_eq!(request["reasoning"]["effort"], "ultra");
    assert_eq!(request["reasoning"]["summary"], "detailed");
    assert_eq!(request["reasoning"]["context"], "client_selected");
}
#[test]
fn responses_lite_forces_all_turns_reasoning_context_without_losing_effort() {
    let mut request = json!({
        "reasoning": {"effort": "high", "summary": "detailed"}
    });

    normalize_account_request(request.as_object_mut().unwrap(), true);

    assert_eq!(request["reasoning"]["context"], "all_turns");
    assert_eq!(request["reasoning"]["effort"], "high");
    assert_eq!(request["reasoning"]["summary"], "detailed");

    let mut malformed = json!({"reasoning": null});
    normalize_account_request(malformed.as_object_mut().unwrap(), true);
    assert_eq!(malformed["reasoning"], json!({"context": "all_turns"}));
}
#[test]
fn account_requests_normalize_non_array_input() {
    for (input, expected) in [
        (
            json!("hello"),
            json!([{"role":"user","content":[{"type":"input_text","text":"hello"}]}]),
        ),
        (json!("  "), json!([])),
        (
            json!({"role":"user","content":"hello"}),
            json!([{"role":"user","content":"hello"}]),
        ),
    ] {
        let mut request = json!({"input": input});
        normalize_account_request(request.as_object_mut().unwrap(), false);
        assert_eq!(request["input"], expected);
    }
}
#[test]
fn account_requests_drop_unusable_reasoning_ids_when_history_is_not_stored() {
    let mut request = json!({
        "store": true,
        "input": [
            {"id": "rs_orphan", "type": "reasoning", "summary": []},
            {"id": "rs_null", "type": "reasoning", "encrypted_content": null, "summary": []},
            {"id": "rs_valid", "type": "reasoning", "encrypted_content": "signed-content", "summary": []},
            {"id": "msg_1", "type": "message", "role": "user", "content": "hello"}
        ]
    });

    normalize_account_request(request.as_object_mut().unwrap(), false);

    assert_eq!(request["store"], false);
    assert!(request.pointer("/input/0/id").is_none());
    assert!(request.pointer("/input/1/id").is_none());
    assert!(request.pointer("/input/1/encrypted_content").is_none());
    assert_eq!(request.pointer("/input/2/id").unwrap(), "rs_valid");
    assert_eq!(request.pointer("/input/3/id").unwrap(), "msg_1");
}
