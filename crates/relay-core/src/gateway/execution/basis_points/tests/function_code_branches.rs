use super::super::response::translate_response_value;
use super::*;

fn branched_request() -> Value {
    let mut request = request_with_tool();
    request["tools"][0]["strict"] = json!(false);
    request["tools"][0]["parameters"] = json!({
        "type":"object",
        "oneOf":[
            {"properties":{"mode":{"const":"inspect"},"path":{"type":"string"}},
             "required":["mode","path"],"additionalProperties":false},
            {"properties":{"mode":{"const":"execute"},"code":{"type":"string"}},
             "required":["mode","code"],"additionalProperties":false}
        ]
    });
    request
}

fn transport_reply(arguments: &Value, framed: bool) -> Value {
    let payload = if framed {
        super::super::codec::encode_function_arguments(arguments).unwrap()
    } else {
        arguments.to_string()
    };
    json!({"status":"completed","output":[{
        "type":"function_call","call_id":"call_branch","name":TRANSPORT_TOOL,
        "arguments":json!({"references":["exec_command"],"code":payload}).to_string()
    }]})
}

#[test]
fn function_code_branches_preserve_absent_empty_and_nonempty_code_through_replay() {
    let request = branched_request();
    for arguments in [
        json!({"mode":"inspect","path":"synthetic.txt"}),
        json!({"mode":"execute","code":""}),
        json!({"mode":"execute","code":"synthetic code\r\nwith \"quotes\" and \\paths"}),
    ] {
        for framed in [false, true] {
            let translated =
                translate_response_value(transport_reply(&arguments, framed), &request).unwrap();
            let call = &translated["output"][0];
            assert_eq!(call["name"], "exec_command");
            assert_eq!(call["call_id"], "call_branch");
            let decoded: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
            assert_eq!(decoded, arguments);
            assert_eq!(decoded.get("code"), arguments.get("code"));

            let mut continuation = request.clone();
            continuation["input"] = json!([
                {"role":"user","content":"Synthetic branch request"}, call,
                {"type":"function_call_output","call_id":"call_branch","output":"synthetic result"}
            ]);
            let replay = prepare_request(&continuation).unwrap();
            assert!(current_catalog_text(&replay).contains("exec_command"));
            assert!(replay["input"].as_array().unwrap().iter().any(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "call_branch"
            }));
        }
    }
}

#[test]
fn function_code_branches_reject_extra_empty_code_and_invalid_selected_branches() {
    let request = branched_request();
    for arguments in [
        json!({"mode":"inspect","path":"synthetic.txt","code":""}),
        json!({"mode":"inspect","path":"synthetic.txt","code":"synthetic code"}),
        json!({"mode":"execute"}),
        json!({"mode":"execute","code":null}),
        json!({"mode":"execute","code":"","path":"synthetic.txt"}),
        json!({"mode":"unknown","code":""}),
    ] {
        let error =
            translate_response_value(transport_reply(&arguments, false), &request).unwrap_err();
        assert_eq!(
            error.code(),
            crate::AdapterError::upstream_tool_arguments_invalid().code()
        );
    }
}
