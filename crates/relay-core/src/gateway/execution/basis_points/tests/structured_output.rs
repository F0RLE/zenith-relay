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
