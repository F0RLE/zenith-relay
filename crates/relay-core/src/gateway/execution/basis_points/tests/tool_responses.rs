use super::*;
use serde_json::json;

#[test]
fn response_translates_outer_call_to_client_tool_call() {
    let request = request_with_tool();
    let body = json!({
        "id":"resp_1",
        "status":"completed",
        "output":[{"type":"function_call","id":"fc_outer","call_id":"call_1","name":TRANSPORT_TOOL,"arguments":serde_json::to_string(&json!({"code":serde_json::to_string(&json!({"tool":"exec_command","args":{"cmd":"pwd"}})).unwrap()})).unwrap()}]
    });
    let translated =
        translate_response(serde_json::to_string(&body).unwrap().as_bytes(), &request).unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["name"], "exec_command");
    assert_eq!(translated["output"][0]["call_id"], "call_1");
    assert_eq!(translated["output"][0]["arguments"], "{\"cmd\":\"pwd\"}");
}

#[test]
fn response_translates_v014_direct_reference_envelope() {
    let request = request_with_tool();
    let body = json!({
        "id":"resp_direct",
        "status":"completed",
        "output":[{"type":"function_call","id":"fc_outer","call_id":"call_1","name":TRANSPORT_TOOL,"arguments":serde_json::to_string(&json!({
            "summary":"Run client tool exec_command",
            "extended_summary":"Relay a shell command through the external client",
            "destructive":false,
            "references":["exec_command"],
            "code":"{\"cmd\":\"pwd\"}"
        })).unwrap()}]
    });
    let translated =
        translate_response(serde_json::to_string(&body).unwrap().as_bytes(), &request).unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["name"], "exec_command");
    assert_eq!(translated["output"][0]["arguments"], "{\"cmd\":\"pwd\"}");
}

#[test]
fn unknown_historical_tool_calls_are_restored_to_the_transport() {
    let request = json!({
        "model": "gpt-6-astra",
        "input": [
            {"type":"function_call","id":"fc_native","call_id":"native_1","name":"native_account_tool","arguments":"{\"value\":1}"},
            {"type":"function_call_output","call_id":"native_1","output":"native result"},
            {"type":"message","role":"user","content":[{"type":"input_text","text":"continue"}]}
        ],
        "tools": [{"type":"function","name":"exec_command","parameters":{"type":"object"}}]
    });
    let prepared = prepare_request(&request).unwrap();
    let input = prepared["input"].as_array().unwrap();
    assert_eq!(input[1]["name"], TRANSPORT_TOOL);
    assert_eq!(input[1]["call_id"], "native_1");
    let arguments: Value = serde_json::from_str(input[1]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments["references"], json!(["native_account_tool"]));
    assert_eq!(arguments["code"], "{\"value\":1}");
    assert_eq!(input[2]["type"], "function_call_output");
    assert_eq!(input[2]["output"], "native result");
    assert!(input[2].get("name").is_none());
    assert_eq!(input[3], request["input"][2]);
}

#[test]
fn namespaced_client_tool_calls_use_a_fully_qualified_reference() {
    let request = json!({
        "model": "gpt-6-astra",
        "input": [{"type":"function_call","call_id":"call_1","name":"exec","namespace":"functions","arguments":"{\"cmd\":\"pwd\"}"}],
        "tools": [{"type":"namespace","name":"functions","tools":[{"type":"function","name":"exec","parameters":{"type":"object"}}]}]
    });
    let prepared = prepare_request(&request).unwrap();
    let call = &prepared["input"][1];
    assert_eq!(call["name"], TRANSPORT_TOOL);
    let arguments: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments["references"], json!(["functions.exec"]));
    assert_eq!(arguments["code"], "{\"cmd\":\"pwd\"}");
}

fn tool_response(code: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "id": "resp_1",
        "status": "completed",
        "output": [{
            "type": "function_call",
            "call_id": "call_1",
            "name": TRANSPORT_TOOL,
            "arguments": json!({"code": code}).to_string()
        }]
    }))
    .unwrap()
}

#[test]
fn malformed_tool_code_is_classified_without_exposing_its_contents() {
    let code = r#"{"tool":"exec_command","args":{"cmd":"echo "private""}}"#;
    let error = translate_response(&tool_response(code), &request_with_tool()).unwrap_err();
    assert_eq!(error.code(), "adapter_upstream_response_invalid");
    assert_eq!(error.parameter(), Some("output.run_officejs.code"));
    assert!(!error.message().contains("private"));
}

#[test]
fn function_tool_arguments_must_be_an_object() {
    let code = r#"{"tool":"exec_command","args":"pwd"}"#;
    let error = translate_response(&tool_response(code), &request_with_tool()).unwrap_err();
    assert_eq!(error.parameter(), Some("output.run_officejs.args"));
}

#[test]
fn ambiguous_namespace_tool_name_is_rejected() {
    let request = json!({
        "tools": [
            {"type":"namespace","name":"first","tools":[{"type":"function","name":"js"}]},
            {"type":"namespace","name":"second","tools":[{"type":"function","name":"js"}]}
        ]
    });
    let error =
        translate_response(&tool_response(r#"{"tool":"js","args":{}}"#), &request).unwrap_err();
    assert_eq!(error.parameter(), Some("output.run_officejs.tool"));

    let translated = translate_response(
        &tool_response(r#"{"tool":"second.js","args":{}}"#),
        &request,
    )
    .unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["name"], "js");
    assert_eq!(translated["output"][0]["namespace"], "second");
}

#[test]
fn additional_tools_replace_earlier_definitions_and_keep_namespaces() {
    let request = json!({
        "model": "gpt-6-astra",
        "tools": [{"type":"namespace","name":"functions","tools":[
            {"type":"function","name":"exec","description":"OLD_DEFINITION"}
        ]}],
        "input": [
            {"type":"additional_tools","tools":[{"type":"namespace","name":"clock","tools":[
                {"type":"function","name":"sleep"}
            ]}]},
            {"type":"additional_tools","tools":[{"type":"namespace","name":"functions","tools":[
                {"type":"custom","name":"exec","description":"LATEST_DEFINITION","format":{"type":"text"}}
            ]}]},
            {"role":"user","content":[{"type":"input_text","text":"Run a command"}]}
        ],
        "tool_choice": {"type":"allowed_tools","mode":"required","tools":[
            {"type":"custom","namespace":"functions","name":"exec"}
        ]}
    });
    let prepared = prepare_request(&request).unwrap();
    let instructions = prepared["input"][0]["content"][0]["text"].as_str().unwrap();
    assert!(instructions.contains("functions.exec (custom): LATEST_DEFINITION"));
    assert!(!instructions.contains("OLD_DEFINITION"));
    assert!(!instructions.contains("clock.sleep"));
    assert!(prepared["input"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| { item.get("type").and_then(Value::as_str) != Some("additional_tools") }));

    let response = tool_response(r#"{"tool":"functions.exec","args":"pwd"}"#);
    let translated = translate_response(&response, &request).unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["type"], "custom_tool_call");
    assert_eq!(translated["output"][0]["namespace"], "functions");
    assert_eq!(translated["output"][0]["input"], "pwd");
}

#[test]
fn disallowed_tool_calls_are_rejected_even_if_the_tool_is_in_history() {
    let mut request = request_with_tool();
    request["tool_choice"] = json!("none");
    let response = tool_response(r#"{"tool":"exec_command","args":{"cmd":"pwd"}}"#);
    let error = translate_response(&response, &request).unwrap_err();
    assert_eq!(error.parameter(), Some("output.run_officejs.tool_choice"));

    request["tool_choice"] = json!({"type":"allowed_tools","tools":[]});
    let error = translate_response(&response, &request).unwrap_err();
    assert_eq!(error.parameter(), Some("output.run_officejs.tool_choice"));

    request["tool_choice"] = json!({"type":"auto"});
    assert!(translate_response(&response, &request).is_ok());
}

#[test]
fn history_replays_client_tools_without_the_current_catalog() {
    for (kind, call, payload) in [
        (
            "function",
            json!({"type":"function_call","id":"fc_old","call_id":"call_fn","name":"exec","namespace":"functions","arguments":"{\"cmd\":\"printf OK\"}"}),
            "functions.exec",
        ),
        (
            "custom",
            json!({"type":"custom_tool_call","id":"fc_old","call_id":"call_custom","name":"apply_patch","input":"  text(\"already executed\");\r\n\t"}),
            "apply_patch",
        ),
    ] {
        for tools in [
            Value::Null,
            json!([]),
            json!([{"type":"function","name":"other"}]),
        ] {
            let mut request = json!({
                "model": "gpt-6-astra",
                "input": [
                    call,
                    {"type": if kind == "custom" { "custom_tool_call_output" } else { "function_call_output" }, "call_id": call["call_id"], "name": call["name"], "output": "already executed"},
                    {"type":"message","role":"user","content":"Summarize this conversation."}
                ]
            });
            if !tools.is_null() {
                request["tools"] = tools;
            }
            let prepared = prepare_request(&request).unwrap();
            let items = prepared["input"].as_array().unwrap();
            let replayed = &items[items.len() - 3];
            assert_eq!(replayed["name"], TRANSPORT_TOOL, "{kind}");
            assert_eq!(replayed["call_id"], call["call_id"], "{kind}");
            let arguments: Value =
                serde_json::from_str(replayed["arguments"].as_str().unwrap()).unwrap();
            assert_eq!(arguments["references"][0], payload, "{kind}");
            let output = &items[items.len() - 2];
            assert_eq!(output["type"], "function_call_output", "{kind}");
            assert_eq!(output["call_id"], call["call_id"], "{kind}");
            assert!(output.get("name").is_none(), "{kind}");
        }
    }
    let qualified = json!({
        "model": "gpt-6-astra",
        "input": [
            {"type":"function_call","call_id":"call_qualified","name":"functions.exec","namespace":"functions","arguments":"{\"cmd\":\"printf OK\"}"},
            {"type":"function_call_output","call_id":"call_qualified","output":"ok"}
        ]
    });
    let prepared = prepare_request(&qualified).unwrap();
    let replayed = prepared["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == TRANSPORT_TOOL)
        .unwrap();
    let arguments: Value = serde_json::from_str(replayed["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments["references"][0], "functions.exec");
}

#[test]
fn custom_tool_and_output_keep_the_client_call_contract() {
    let request = json!({
        "model": "gpt-6-astra",
        "input": [
            {"type":"custom_tool_call","call_id":"call_1","name":"apply_patch","input":"diff --git a/a b/a"},
            {"type":"custom_tool_call_output","call_id":"call_1","output":"ok"}
        ],
        "tools": [{"type":"custom","name":"apply_patch","description":"Apply a patch","format":{"type":"text"}}]
    });
    let prepared = prepare_request(&request).unwrap();
    assert_eq!(prepared["input"][1]["name"], TRANSPORT_TOOL);
    assert_eq!(prepared["input"][2]["type"], "function_call_output");
    assert_eq!(prepared["input"][2]["id"], "fc_call_1");

    let body = json!({
        "id":"resp_1",
        "status":"completed",
        "output":[{"type":"function_call","id":"fc_outer","call_id":"call_2","name":TRANSPORT_TOOL,"arguments":serde_json::to_string(&json!({"code":serde_json::to_string(&json!({"tool":"apply_patch","args":"diff --git a/a b/a"})).unwrap()})).unwrap()}]
    });
    let translated =
        translate_response(serde_json::to_string(&body).unwrap().as_bytes(), &request).unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["type"], "custom_tool_call");
    assert_eq!(translated["output"][0]["input"], "diff --git a/a b/a");
}

#[test]
fn required_tool_choice_without_a_tool_is_rejected_before_dispatch() {
    let request = json!({
        "model": "gpt-6-astra",
        "input": "hello",
        "tool_choice": "required"
    });
    let error = prepare_request(&request).unwrap_err();
    assert_eq!(error.parameter(), Some("tool_choice"));
}

#[test]
fn required_allowed_tools_rejects_a_response_without_a_call() {
    let mut request = request_with_tool();
    request["tool_choice"] = json!({
        "type": "allowed_tools",
        "mode": "required",
        "tools": [{"type": "function", "name": "exec_command"}]
    });
    let prepared = prepare_request(&request).unwrap();
    assert!(prepared.get("tool_choice").is_none());
    let response = json!({"id": "resp_1", "status": "completed", "output": []});
    assert!(translate_response(
        serde_json::to_string(&response).unwrap().as_bytes(),
        &request
    )
    .is_err());
}

#[test]
fn synthetic_stream_has_terminal_responses_events() {
    let body = json!({
        "id":"resp_1",
        "status":"completed",
        "output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"hello","annotations":[]}]}]
    });
    let stream = synthetic_stream(serde_json::to_string(&body).unwrap().as_bytes()).unwrap();
    let stream = String::from_utf8(stream).unwrap();
    assert!(stream.contains("response.created"));
    assert!(stream.contains("response.output_text.delta"));
    assert!(stream.contains("response.output_text.done"));
    assert!(stream.contains("response.completed"));
    assert!(stream.ends_with("data: [DONE]\n\n"));
}

#[test]
fn incomplete_response_is_not_reported_as_completed() {
    let request = json!({"model": "gpt-6-astra", "input": "hello"});
    let incomplete = json!({
        "id": "resp_1", "status": "incomplete", "output": [],
        "incomplete_details": {"reason": "max_output_tokens"}
    });
    let body = translate_response(&serde_json::to_vec(&incomplete).unwrap(), &request).unwrap();
    let stream = String::from_utf8(synthetic_stream(&body).unwrap()).unwrap();
    assert!(stream.contains("event: response.incomplete\n"));
    assert!(!stream.contains("event: response.completed\n"));
    assert!(stream.contains("max_output_tokens"));

    for response in [
        json!({"status": "completed"}),
        json!({"status": "failed", "output": []}),
        json!({"status": "cancelled", "output": []}),
        json!({"output": []}),
    ] {
        assert!(translate_response(&serde_json::to_vec(&response).unwrap(), &request).is_err());
    }
}
