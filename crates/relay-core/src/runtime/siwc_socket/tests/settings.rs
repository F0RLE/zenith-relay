use super::*;
use crate::usage::FullRequestReason;

fn continued_request(body: &Value) -> (planner::Checkpoint, Value, Value) {
    let output = json!({"type":"message","id":"msg_test","role":"assistant",
        "content":[{"type":"output_text","text":"synthetic answer"}]});
    let checkpoint = plan(body, None)
        .state
        .complete(&json!({"id":"first","status":"completed","output":[output]}))
        .unwrap();
    let new_input = json!({"role":"user","content":"synthetic next"});
    let mut next = body.clone();
    next["input"]
        .as_array_mut()
        .unwrap()
        .extend([output, new_input.clone()]);
    (checkpoint, next, new_input)
}

#[test]
fn client_tracing_changes_preserve_delta_and_current_wire_metadata() {
    let plain = request(json!([{"role":"user","content":"synthetic"}]));
    for previous in [
        None,
        Some(Value::Null),
        Some(json!({})),
        Some(json!({"turn":"synthetic-before"})),
    ] {
        let mut body = plain.clone();
        if let Some(metadata) = previous {
            body["client_metadata"] = metadata;
        }
        let (checkpoint, next, new_input) = continued_request(&body);
        for current in [
            None,
            Some(Value::Null),
            Some(json!({})),
            Some(json!({"turn":"synthetic-after"})),
        ] {
            let mut next = next.clone();
            next.as_object_mut().unwrap().remove("client_metadata");
            if let Some(metadata) = current {
                next["client_metadata"] = metadata;
            }
            let planned = plan(&next, Some(&checkpoint));
            assert_eq!(planned.full_request_reason, None);
            let encoded: Value = serde_json::from_str(&planned.payload).unwrap();
            assert_eq!(encoded["previous_response_id"], "first");
            assert_eq!(encoded["input"], json!([new_input]));
            assert_eq!(encoded.get("client_metadata"), next.get("client_metadata"));
        }
    }
}

#[test]
fn tracing_exception_does_not_hide_semantic_unknown_or_malformed_settings() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let (checkpoint, next, _) = continued_request(&body);
    for (field, value) in [
        ("client_metadata", json!({"turn":1})),
        ("client_metadata", json!([])),
        ("client_metadata", json!("invalid")),
        ("metadata", json!({"source":"synthetic"})),
        ("instructions", json!("synthetic changed instructions")),
        ("prompt_cache_key", json!("synthetic changed key")),
        ("reasoning", json!({"effort":"low"})),
        ("tools", json!([{"type":"function","name":"lookup"}])),
        ("service_tier", json!("priority")),
        ("synthetic_extension", json!(true)),
    ] {
        let mut changed = next.clone();
        changed["client_metadata"] = json!({"turn":"synthetic changed tracing"});
        changed[field] = value;
        let planned = plan(&changed, Some(&checkpoint));
        assert_eq!(
            planned.full_request_reason,
            Some(FullRequestReason::SocketSettingsChanged),
            "{field}"
        );
        let encoded: Value = serde_json::from_str(&planned.payload).unwrap();
        assert!(encoded.get("previous_response_id").is_none());
        assert_eq!(encoded["input"], changed["input"]);
        assert_eq!(encoded[field], changed[field]);
    }
}

#[test]
fn delivery_obfuscation_changes_keep_current_wire_options_and_delta() {
    let plain = request(json!([{"role":"user","content":"synthetic"}]));
    for previous in [
        None,
        Some(json!({})),
        Some(json!({"include_obfuscation":true})),
        Some(json!({"include_obfuscation":false})),
    ] {
        let mut body = plain.clone();
        if let Some(options) = previous {
            body["stream_options"] = options;
        }
        let (checkpoint, next, new_input) = continued_request(&body);
        for current in [
            None,
            Some(json!({})),
            Some(json!({"include_obfuscation":true})),
            Some(json!({"include_obfuscation":false})),
        ] {
            let mut next = next.clone();
            next.as_object_mut().unwrap().remove("stream_options");
            if let Some(options) = current {
                next["stream_options"] = options;
            }
            let planned = plan(&next, Some(&checkpoint));
            assert_eq!(planned.full_request_reason, None);
            let encoded: Value = serde_json::from_str(&planned.payload).unwrap();
            assert_eq!(encoded["previous_response_id"], "first");
            assert_eq!(encoded["input"], json!([new_input]));
            assert_eq!(encoded.get("stream_options"), next.get("stream_options"));
        }
    }
}

#[test]
fn unknown_and_malformed_stream_options_still_require_full_history() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let (checkpoint, next, _) = continued_request(&body);
    for options in [
        json!({"include_obfuscation":"invalid"}),
        json!({"include_obfuscation":null}),
        json!({"include_obfuscation":true,"synthetic_extension":true}),
        json!([]),
        Value::Null,
    ] {
        let mut next = next.clone();
        next["stream_options"] = options;
        let planned = plan(&next, Some(&checkpoint));
        assert_eq!(
            planned.full_request_reason,
            Some(FullRequestReason::SocketSettingsChanged)
        );
        let encoded: Value = serde_json::from_str(&planned.payload).unwrap();
        assert_eq!(encoded["input"], next["input"]);
        assert_eq!(encoded["stream_options"], next["stream_options"]);
    }
    let mut body = body;
    body["stream_options"] = json!({"include_obfuscation":true,"synthetic_extension":1});
    let (checkpoint, mut next, new_input) = continued_request(&body);
    next["stream_options"]["include_obfuscation"] = json!(false);
    let encoded: Value = serde_json::from_str(&plan(&next, Some(&checkpoint)).payload).unwrap();
    assert_eq!(encoded["input"], json!([new_input]));
    next["stream_options"]["synthetic_extension"] = json!(2);
    assert_eq!(
        plan(&next, Some(&checkpoint)).full_request_reason,
        Some(FullRequestReason::SocketSettingsChanged)
    );
}

#[test]
fn explicit_false_prewarm_matches_default_without_changing_generation() {
    let plain = request(json!([{"role":"user","content":"synthetic"}]));
    for previous in [None, Some(json!({})), Some(json!({"prewarm":false}))] {
        let mut body = plain.clone();
        if let Some(options) = previous {
            body["prompt_cache_options"] = options;
        }
        let (checkpoint, next, new_input) = continued_request(&body);
        for current in [None, Some(json!({})), Some(json!({"prewarm":false}))] {
            let mut next = next.clone();
            next.as_object_mut().unwrap().remove("prompt_cache_options");
            if let Some(options) = current {
                next["prompt_cache_options"] = options;
            }
            let planned = plan(&next, Some(&checkpoint));
            assert_eq!(planned.full_request_reason, None);
            let encoded: Value = serde_json::from_str(&planned.payload).unwrap();
            assert_eq!(encoded["previous_response_id"], "first");
            assert_eq!(encoded["input"], json!([new_input]));
            assert_eq!(
                encoded.get("prompt_cache_options"),
                next.get("prompt_cache_options")
            );
            assert!(encoded.get("generate").is_none());
        }
    }
}

#[test]
fn completed_zero_output_warmup_can_generate_with_no_new_input() {
    for input in [json!([]), json!([{"role":"user","content":"synthetic"}])] {
        let mut body = request(input);
        body["prompt_cache_options"] = json!({"prewarm":true,"ttl":"30m"});
        let checkpoint = plan(&body, None)
            .state
            .complete(&json!({"id":"synthetic-warmup","status":"completed","output":[]}))
            .unwrap();
        for next_options in [json!({"ttl":"30m"}), json!({"prewarm":false,"ttl":"30m"})] {
            let mut next = body.clone();
            next["prompt_cache_options"] = next_options;
            let planned = plan(&next, Some(&checkpoint));
            assert_eq!(planned.full_request_reason, None);
            let encoded: Value = serde_json::from_str(&planned.payload).unwrap();
            assert_eq!(encoded["previous_response_id"], "synthetic-warmup");
            assert_eq!(encoded["input"], json!([]));
            assert!(encoded.get("generate").is_none());
            assert!(
                planned
                    .state
                    .complete(&json!({"id":"synthetic-empty-generation",
                    "status":"completed","output":[]}))
                    .is_none()
            );
        }
        let repeated = plan(&body, Some(&checkpoint));
        assert_eq!(
            repeated.full_request_reason,
            Some(FullRequestReason::SocketNoNewInput)
        );
        let mut changed = body.clone();
        changed
            .as_object_mut()
            .unwrap()
            .remove("prompt_cache_options");
        changed["reasoning"]["effort"] = json!("low");
        assert_eq!(
            plan(&changed, Some(&checkpoint)).full_request_reason,
            Some(FullRequestReason::SocketSettingsChanged)
        );
    }
}
