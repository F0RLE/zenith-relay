use super::*;

fn request() -> Value {
    json!({"model":"test","input":"Return an answer","stream":true,
        "relay_bps_output_validation":true,
        "text":{"format":{"type":"json_schema","name":"answer","strict":false,
            "schema":{"type":"object","properties":{"count":{"type":"integer"}},
                "required":["count"],"additionalProperties":false}}}})
}

fn response(text: &str) -> Vec<u8> {
    serde_json::to_vec(
        &json!({"id":"resp_test","model":"test","status":"completed",
        "output":[{"type":"message","role":"assistant","id":"msg_test",
            "content":[{"type":"output_text","text":text}]}]}),
    )
    .unwrap()
}

#[test]
fn explicit_non_strict_schema_needs_no_private_flag_and_validates_before_release() {
    let mut request = request();
    request
        .as_object_mut()
        .unwrap()
        .remove("relay_bps_output_validation");
    let prepared = prepare_request(&request).unwrap();
    assert!(prepared.get("text").is_none());
    let instruction = prepared["input"][1]["content"][0]["text"].as_str().unwrap();
    assert!(instruction.contains(&request["text"]["format"].to_string()));
    assert!(instruction.contains("not native constrained decoding"));
    let valid = translate_response(&response("{\"count\":3}"), &request).unwrap();
    let stream = String::from_utf8(response::stream::synthetic_stream(&valid).unwrap()).unwrap();
    assert!(stream.contains("response.completed"));
    for invalid in [
        "not-json",
        "{}",
        "{\"count\":\"three\"}",
        "{\"count\":3,\"unexpected\":true}",
        "{\"count\":3} {}",
    ] {
        assert_eq!(
            translate_response(&response(invalid), &request)
                .unwrap_err()
                .parameter(),
            Some("output.structured_output")
        );
    }
}

#[test]
fn automatic_output_validation_honors_text_format_defaults_and_explicit_opt_out() {
    let mut request = request();
    request
        .as_object_mut()
        .unwrap()
        .remove("relay_bps_output_validation");
    request["text"]["format"]["strict"] = json!(true);
    assert!(prepare_request(&request).is_err());
    request["text"]["format"]
        .as_object_mut()
        .unwrap()
        .remove("strict");
    for strict in [None, Some(Value::Null), Some(json!(false))] {
        if let Some(strict) = strict {
            request["text"]["format"]["strict"] = strict;
        }
        let original = request.clone();
        let prepared = prepare_request(&request).unwrap();
        assert!(prepared.get("text").is_none());
        assert!(translate_response(&response("{\"count\":3}"), &request).is_ok());
        assert!(translate_response(&response("{}"), &request).is_err());
        assert_eq!(request, original);
        request["relay_bps_output_validation"] = json!(false);
        assert!(prepare_request(&request).is_err());
        request
            .as_object_mut()
            .unwrap()
            .remove("relay_bps_output_validation");
    }
}

#[test]
fn opt_in_is_local_and_preserves_the_schema_without_native_strict_claims() {
    let request = request();
    let prepared = prepare_request(&request).unwrap();
    assert!(prepared.get("relay_bps_output_validation").is_none());
    assert!(prepared.get("text").is_none());
    let instruction = prepared["input"][1]["content"][0]["text"].as_str().unwrap();
    assert!(instruction.contains("not native constrained decoding"));
    assert!(instruction.contains(&request["text"]["format"].to_string()));
    assert_eq!(prepared["stream"], true);
    let valid = translate_response(&response("{\"count\":3}"), &request).unwrap();
    let events = String::from_utf8(response::stream::synthetic_stream(&valid).unwrap()).unwrap();
    assert!(events.contains("response.completed"));
    assert!(events.contains("response.output_text.delta"));
    for invalid in [
        "not-json",
        "{}",
        "{\"count\":\"three\"}",
        "{\"count\":3} {}",
    ] {
        let error = translate_response(&response(invalid), &request).unwrap_err();
        assert_eq!(error.parameter(), Some("output.structured_output"));
    }
}

#[test]
fn strict_invalid_modes_and_external_refs_fail_before_dispatch() {
    let mut request = request();
    request["text"]["format"]["strict"] = json!(true);
    assert_eq!(
        prepare_request(&request).unwrap_err().parameter(),
        Some("text.format.strict")
    );
    request["text"]["format"]["strict"] = json!(false);
    request["text"]["format"]["schema"] = json!({"$ref":"https://example.invalid/remote"});
    assert_eq!(
        prepare_request(&request).unwrap_err().parameter(),
        Some("text.format.schema")
    );
    request["relay_bps_output_validation"] = json!("true");
    assert_eq!(
        prepare_request(&request).unwrap_err().parameter(),
        Some("relay_bps_output_validation")
    );
    request["relay_bps_output_validation"] = json!(false);
    assert_eq!(
        prepare_request(&request).unwrap_err().parameter(),
        Some("text.format")
    );
}

#[test]
fn default_json_object_validates_without_a_private_flag_and_preserves_opt_out() {
    let mut request = json!({"model":"test","input":"Return an object",
        "text":{"format":{"type":"json_object"}}});
    let original = request.clone();
    for stream in [false, true] {
        request["stream"] = json!(stream);
        let prepared = prepare_request(&request).unwrap();
        assert!(prepared.get("text").is_none());
        assert!(prepared.get("relay_bps_output_validation").is_none());
        assert!(translate_response(&response("{\"count\":3}"), &request).is_ok());
        for invalid in ["not-json", "[]", "null", "3", "{} {}", "```json\n{}\n```"] {
            assert_eq!(
                translate_response(&response(invalid), &request)
                    .unwrap_err()
                    .parameter(),
                Some("output.structured_output")
            );
        }
    }
    request.as_object_mut().unwrap().remove("stream");
    assert_eq!(request, original);
    request["relay_bps_output_validation"] = json!(false);
    assert_eq!(
        prepare_request(&request).unwrap_err().parameter(),
        Some("text.format")
    );
}

#[test]
fn validation_option_does_not_buffer_or_reject_plain_text_without_a_format() {
    use super::super::structured_output::requires_buffering;

    for text in [
        None,
        Some(Value::Null),
        Some(json!({})),
        Some(json!({"format":null})),
        Some(json!({"format":{"type":"text"}})),
    ] {
        let mut request = json!({"model":"test","input":"hello","stream":true,
            "relay_bps_output_validation":true});
        if let Some(text) = text {
            request["text"] = text;
        }
        assert!(
            !requires_buffering(&request),
            "plain text needs no JSON validation buffer"
        );
        assert!(prepare_request(&request).is_ok());
        assert!(translate_response(&response("ordinary text"), &request).is_ok());
    }
}

#[test]
fn native_strict_rejection_names_the_actual_constraint_without_private_opt_in() {
    let mut request = request();
    request
        .as_object_mut()
        .unwrap()
        .remove("relay_bps_output_validation");
    request["text"]["format"]["strict"] = json!(true);
    for explicit in [false, true] {
        if explicit {
            request["relay_bps_output_validation"] = json!(true);
        }
        let error = prepare_request(&request).unwrap_err();
        assert_eq!(error.parameter(), Some("text.format.strict"));
        assert_eq!(error.code(), "adapter_parameter_unsupported");
    }
}
