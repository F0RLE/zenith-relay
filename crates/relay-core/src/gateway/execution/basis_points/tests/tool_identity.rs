use super::super::response::translate_response_value;
use super::*;

fn response(outer: Value) -> Value {
    json!({"status":"completed","output":[{
        "type":"function_call","call_id":"call_synthetic","name":TRANSPORT_TOOL,
        "arguments":outer.to_string()
    }]})
}

#[test]
fn tool_identity_errors_distinguish_safe_causes_without_names_or_payloads() {
    let mut ambiguous = request_with_tool();
    ambiguous["tools"] = json!([
        {"type":"namespace","name":"first","tools":[{"type":"function","name":"synthetic-private"}]},
        {"type":"namespace","name":"second","tools":[{"type":"function","name":"synthetic-private"}]}
    ]);
    let cases = [
        (request_with_tool(), json!({"code":"{}"}), "missing_name"),
        (
            request_with_tool(),
            json!({"code":json!({"tool":null,"name":"synthetic-private"}).to_string()}),
            "invalid_name",
        ),
        (
            request_with_tool(),
            json!({"code":json!({"name":"   ","arguments":{"cmd":"synthetic-private"}}).to_string()}),
            "invalid_name",
        ),
        (
            request_with_tool(),
            json!({"code":json!({"tool":"exec_command","name":"synthetic-private","args":{"cmd":"synthetic-private"}}).to_string()}),
            "conflicting_names",
        ),
        (
            request_with_tool(),
            json!({"references":["synthetic-private"],"code":"{}"}),
            "unknown_tool",
        ),
        (
            ambiguous,
            json!({"references":["synthetic-private"],"code":"{}"}),
            "ambiguous_name",
        ),
        (
            request_with_tool(),
            json!({"references":["exec_command"],"code":"synthetic-private({\"cmd\":\"synthetic-private\"})"}),
            "reference_name_mismatch",
        ),
    ];
    for (request, outer, reason) in cases {
        let error = translate_response(&serde_json::to_vec(&response(outer)).unwrap(), &request)
            .unwrap_err();
        assert_eq!(error.parameter(), Some("output.run_officejs.tool"));
        let diagnostic = error.to_string();
        assert!(
            diagnostic.contains(&format!("tool_identity_reason={reason}")),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("failure_stage=tool_identity"));
        assert!(diagnostic.contains("repair_skipped=executable_tool"));
        assert!(!format!("{error} {error:?}").contains("synthetic-private"));
    }
}

#[test]
fn legacy_identity_aliases_keep_tool_choice_schema_and_custom_payload_authoritative() {
    let mut request = request_with_tool();
    let reply = response(json!({"code":json!({
        "name":"exec_command","arguments":{"cmd":"synthetic-private"}
    }).to_string()}));
    request["tool_choice"] = json!("none");
    let error = translate_response(&serde_json::to_vec(&reply).unwrap(), &request).unwrap_err();
    assert_eq!(error.parameter(), Some("output.run_officejs.tool_choice"));

    request["tool_choice"] = json!("auto");
    request["tools"][0]["parameters"]["properties"]["cmd"]["type"] = json!("integer");
    let error = translate_response(&serde_json::to_vec(&reply).unwrap(), &request).unwrap_err();
    assert_eq!(error.parameter(), Some("output.run_officejs.args.schema"));

    request["tools"] = json!([{"type":"custom","name":"patch"}]);
    let raw = r#"{"name":"relay.catalog","arguments":{"names":["synthetic-private"]}}"#;
    let reply = response(json!({"references":["patch"],"code":raw}));
    let translated: Value = serde_json::from_slice(
        &translate_response(&serde_json::to_vec(&reply).unwrap(), &request).unwrap(),
    )
    .unwrap();
    assert_eq!(translated["output"][0]["type"], "custom_tool_call");
    assert_eq!(translated["output"][0]["input"], raw);
}

#[test]
fn replayed_call_ids_cannot_bypass_history_after_normalization() {
    let original = format!("call_{}", "synthetic".repeat(16));
    let fitted = super::super::history::fit_item_id(&original);
    assert_ne!(original, fitted);
    for (historical, generated) in [(&original, &fitted), (&fitted, &original)] {
        for kind in [
            "function_call",
            "custom_tool_call",
            "function_call_output",
            "custom_tool_call_output",
        ] {
            let history = json!({"type":kind,"call_id":historical});
            for input in [history.clone(), json!([history])] {
                let mut request = request_with_tool();
                request["input"] = input;
                let mut reply = response(
                    json!({"references":["exec_command"],"code":"{\"cmd\":\"synthetic\"}"}),
                );
                reply["output"][0]["call_id"] = json!(generated);
                let error = translate_response_value(reply.clone(), &request).unwrap_err();
                assert_eq!(error.parameter(), Some("output.run_officejs.call_id"));
                reply["output"][0]["call_id"] = json!("call_synthetic_fresh");
                assert!(translate_response_value(reply, &request).is_ok());
            }
        }
    }
}

#[test]
fn replayed_call_ids_cannot_collide_after_parallel_normalization() {
    let original = format!("call_{}", "synthetic".repeat(16));
    let fitted = super::super::history::fit_item_id(&original);
    for ids in [[&original, &fitted], [&fitted, &original]] {
        let mut reply =
            response(json!({"references":["exec_command"],"code":"{\"cmd\":\"synthetic\"}"}));
        let mut other = reply["output"][0].clone();
        reply["output"][0]["call_id"] = json!(ids[0]);
        other["call_id"] = json!(ids[1]);
        reply["output"].as_array_mut().unwrap().push(other);
        let error = translate_response_value(reply.clone(), &request_with_tool()).unwrap_err();
        assert_eq!(error.parameter(), Some("output.run_officejs.call_id"));
        reply["output"][1]["call_id"] = json!("call_synthetic_fresh");
        assert!(translate_response_value(reply, &request_with_tool()).is_ok());
    }
}
