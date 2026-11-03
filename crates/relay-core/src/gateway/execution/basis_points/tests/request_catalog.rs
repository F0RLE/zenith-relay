use super::super::{PreparedClientRequest, prepare_request, response::translate_response_value};
use serde_json::{Value, json};

fn tool(minimum: i64) -> Value {
    json!({"type":"function","name":"lookup","parameters":{
        "type":"object","properties":{"n":{"type":"integer","minimum":minimum}},
        "required":["n"],"additionalProperties":false
    }})
}

fn reply(name: &str, arguments: Value) -> Value {
    json!({"id":"resp_synthetic","status":"completed","model":"synthetic",
        "output":[{"type":"function_call","name":"run_officejs","id":"fc_new",
            "call_id":"call_new","arguments":json!({"references":[name],
                "code":arguments.to_string()}).to_string()}],
        "usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110}})
}

fn outcome(result: Result<Value, crate::AdapterError>) -> Result<Value, (String, Option<String>)> {
    result.map_err(|error| {
        (
            error.code().to_owned(),
            error.parameter().map(str::to_owned),
        )
    })
}

#[test]
fn prepared_catalog_keeps_wire_validation_and_current_tool_choice_identical() {
    for mode in ["compact", "full", "discovery"] {
        for choice in [
            json!("auto"),
            json!("none"),
            json!({"type":"function","name":"lookup"}),
            json!({"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"lookup"}]}),
        ] {
            let request = json!({"model":"synthetic","stream":true,"tool_choice":choice,
                "metadata":{"synthetic":"retained"},"relay_bps_tool_catalog":mode,
                "input":[{"role":"user","content":"Synthetic history"},
                    {"type":"additional_tools","tools":[tool(1)]}],
                "tools":[tool(3), {"type":"namespace","name":"nested","tools":[tool(7)]},
                    {"type":"custom","name":"patch","format":{"type":"text"}}]});
            let expected_wire = prepare_request(&request).unwrap();
            let (prepared, wire) =
                PreparedClientRequest::prepare(request.clone(), &mut None, true).unwrap();
            assert_eq!(
                serde_json::to_vec(&wire).unwrap(),
                serde_json::to_vec(&expected_wire).unwrap()
            );
            assert_eq!(&*prepared, &request);
            for response in [
                reply("lookup", json!({"n":4})),
                reply("lookup", json!({"n":2})),
                reply("nested.lookup", json!({"n":8})),
                reply("missing", json!({})),
                reply("patch", json!("synthetic raw patch")),
                json!({"status":"completed","output":[]}),
                json!({"status":"incomplete","output":[],"incomplete_details":{"reason":"max_output_tokens"}}),
            ] {
                assert_eq!(
                    outcome(prepared.translate_value(response.clone())),
                    outcome(translate_response_value(response, &request)),
                    "mode={mode}, choice={choice}"
                );
            }
        }
    }
}

#[test]
fn prepared_catalog_does_not_share_request_authority() {
    let first = json!({"model":"synthetic","input":"hello","tools":[tool(1)]});
    let mut second = first.clone();
    second["tools"] = json!([tool(10)]);
    let (first, _) = PreparedClientRequest::prepare(first, &mut None, false).unwrap();
    let (second, _) = PreparedClientRequest::prepare(second, &mut None, false).unwrap();
    assert!(
        first
            .translate_value(reply("lookup", json!({"n":2})))
            .is_ok()
    );
    assert!(
        second
            .translate_value(reply("lookup", json!({"n":2})))
            .is_err()
    );
    assert!(
        first
            .translate_value(reply("lookup", json!({"n":2})))
            .is_ok()
    );
}

#[test]
fn prepared_catalog_retains_admission_error_precedence() {
    for extra in [
        json!({"previous_response_id":"resp_previous"}),
        json!({"text":{"format":{"type":"invalid"}}}),
        json!({"input":[{"type":"agent_message","encrypted_content":"synthetic"}]}),
    ] {
        let mut request = json!({"model":"synthetic","tools":[{"type":"function","name":"broken",
            "parameters":{"type":"invalid"}}]});
        request
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let expected = prepare_request(&request).unwrap_err();
        let actual = match PreparedClientRequest::prepare(request, &mut None, false) {
            Ok(_) => panic!("invalid request was accepted"),
            Err(error) => error,
        };
        assert_eq!(actual.code(), expected.code());
        assert_eq!(actual.parameter(), expected.parameter());
    }
}
