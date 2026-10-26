use super::super::response::translate_response_value;
use super::*;
use serde_json::json;

#[test]
fn schema_preparation_timing_preserves_wire_body_and_rejection() {
    for mode in ["compact", "full", "discovery"] {
        let mut request = request_with_tool();
        request["relay_bps_tool_catalog"] = json!(mode);
        let expected = prepare_request(&request).unwrap();
        let mut measured = None;
        let actual =
            super::super::prepare_request_with_timing(&request, &mut measured, true).unwrap();
        assert_eq!(actual, expected);
        assert!(measured.is_some());
        request["tools"][0]["parameters"]["type"] = json!("invalid-synthetic-type");
        let expected = prepare_request(&request).unwrap_err();
        let actual =
            super::super::prepare_request_with_timing(&request, &mut measured, true).unwrap_err();
        assert_eq!(actual.to_string(), expected.to_string());
        assert_eq!(measured, None);
    }
}

#[test]
fn reference_routing_recovers_only_exact_same_tool_legacy_envelopes() {
    let mut request = request_with_tool();
    request["tools"][0]["parameters"]["additionalProperties"] = false.into();
    let arguments = json!({"cmd":"Write-Output \"quoted\"\r\nC:\\synthetic\\file 😀"});
    for (name_key, args_key) in [
        ("tool", "args"),
        ("name", "arguments"),
        ("tool", "arguments"),
        ("name", "args"),
    ] {
        for encoded in [false, true] {
            let nested = if encoded {
                Value::String(arguments.to_string())
            } else {
                arguments.clone()
            };
            let response = json!({"status":"completed","output":[{
                "type":"function_call","call_id":"call_legacy","name":TRANSPORT_TOOL,
                "arguments":json!({"references":["exec_command"],"code":json!({(name_key):"exec_command",(args_key):nested}).to_string()}).to_string()
            }]});
            let translated = translate_response_value(response, &request).unwrap();
            assert_eq!(translated["output"][0]["name"], "exec_command");
            assert_eq!(translated["output"][0]["call_id"], "call_legacy");
            assert_eq!(
                serde_json::from_str::<Value>(
                    translated["output"][0]["arguments"].as_str().unwrap()
                )
                .unwrap(),
                arguments
            );
        }
    }
}

#[test]
fn legacy_envelope_recovery_never_drops_fields_or_bypasses_the_client_schema() {
    let mut request = request_with_tool();
    request["tools"][0]["parameters"]["additionalProperties"] = false.into();
    for payload in [
        json!({"tool":"different_tool","args":{"cmd":"synthetic"}}),
        json!({"tool":"exec_command","args":{"cmd":"synthetic"},"extra":true}),
        json!({"tool":"exec_command","args":{"cmd":"synthetic","extra":true}}),
        json!({"tool":"exec_command","args":{"cmd":42}}),
        json!({"tool":"exec_command","args":{}}),
        json!({"tool":"exec_command","args":{"tool":"exec_command","args":{"cmd":"synthetic"}}}),
        json!({"cmd":"synthetic","extra":true}),
    ] {
        let response = json!({"status":"completed","output":[{
            "type":"function_call","call_id":"call_invalid","name":TRANSPORT_TOOL,
            "arguments":json!({"references":["exec_command"],"code":payload.to_string()}).to_string()
        }]});
        assert_eq!(
            translate_response_value(response, &request)
                .unwrap_err()
                .parameter(),
            Some("output.run_officejs.args.schema")
        );
    }
}

#[test]
fn recovered_namespaced_arguments_keep_stream_and_tool_result_roundtrips() {
    let mut request = request_with_tool();
    let mut tool = request["tools"][0].clone();
    tool["parameters"]["additionalProperties"] = false.into();
    request["tools"] = json!([{"type":"namespace","name":"repo","tools":[tool]}]);
    let arguments = json!({"cmd":"synthetic command"});
    let response = json!({"id":"resp_legacy","status":"completed","output":[{
        "type":"function_call","call_id":"call_legacy_ns","name":TRANSPORT_TOOL,
        "arguments":json!({"references":["repo.exec_command"],"code":json!({
            "tool":"repo.exec_command","args":arguments
        }).to_string()}).to_string()
    }]});
    let translated = translate_response_value(response, &request).unwrap();
    assert_eq!(translated["output"][0]["namespace"], "repo");
    let stream =
        String::from_utf8(synthetic_stream(&serde_json::to_vec(&translated).unwrap()).unwrap())
            .unwrap();
    let events = stream
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .collect::<Vec<_>>();
    let done = events
        .iter()
        .find(|event| event["type"] == "response.function_call_arguments.done")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(done["arguments"].as_str().unwrap()).unwrap(),
        arguments
    );
    request["input"] = json!([
        translated["output"][0],
        {"type":"function_call_output","call_id":"call_legacy_ns","output":"synthetic result"}
    ]);
    let upstream = prepare_request(&request).unwrap();
    let replayed = upstream["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call")
        .unwrap();
    let envelope: Value = serde_json::from_str(replayed["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(envelope["references"], json!(["repo.exec_command"]));
    assert_eq!(
        serde_json::from_str::<Value>(envelope["code"].as_str().unwrap()).unwrap(),
        arguments
    );
}

#[test]
fn declared_envelope_shaped_function_arguments_are_never_unwrapped() {
    let mut request = request_with_tool();
    request["tools"][0]["parameters"] = json!({"type":"object","properties":{
        "tool":{"type":"string"},"args":{"type":"object"}
    },"required":["tool","args"],"additionalProperties":false});
    let arguments = json!({"tool":"exec_command","args":{"cmd":"synthetic"}});
    let response = json!({"status":"completed","output":[{
        "type":"function_call","call_id":"call_literal","name":TRANSPORT_TOOL,
        "arguments":json!({"references":["exec_command"],"code":arguments.to_string()}).to_string()
    }]});
    let translated = translate_response_value(response, &request).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(translated["output"][0]["arguments"].as_str().unwrap())
            .unwrap(),
        arguments
    );
}

#[test]
fn failed_declared_open_or_namespaced_arguments_are_not_reinterpreted() {
    let arguments = json!({"tool":"exec_command","args":{"cmd":"synthetic"}});
    let response = json!({"status":"completed","output":[{
        "type":"function_call","call_id":"call_invalid","name":TRANSPORT_TOOL,
        "arguments":json!({"references":["exec_command"],"code":arguments.to_string()}).to_string()
    }]});
    for schema in [
        json!({"type":"object","properties":{
            "cmd":{"type":"string"},"tool":{"type":"string"},"args":{"type":"object"}
        },"required":["cmd"],"additionalProperties":false}),
        json!({"type":"object","properties":{"cmd":{"type":"string"}},"required":["cmd"]}),
        json!({"type":"object","properties":{"cmd":{"type":"string"}},
            "patternProperties":{"^(tool|args)$":{}},"required":["cmd"],"additionalProperties":false}),
        json!({"type":"object","properties":{"cmd":{"type":"string"}},
            "required":["cmd"],"additionalProperties":true}),
        json!({"type":"object","required":["cmd"],"additionalProperties":false}),
    ] {
        let mut request = request_with_tool();
        request["tools"][0]["parameters"] = schema;
        assert_eq!(
            translate_response_value(response.clone(), &request)
                .unwrap_err()
                .parameter(),
            Some("output.run_officejs.args.schema")
        );
    }

    let mut request = request_with_tool();
    let mut tool = request["tools"][0].clone();
    tool["parameters"]["additionalProperties"] = false.into();
    request["tools"] = json!([{"type":"namespace","name":"repo","tools":[tool]}]);
    for name in ["exec_command", "other.exec_command"] {
        let mut response = response.clone();
        response["output"][0]["arguments"] = json!({
            "references":["repo.exec_command"],"code":json!({"tool":name,"args":{"cmd":"synthetic"}}).to_string()
        }).to_string().into();
        assert_eq!(
            translate_response_value(response, &request)
                .unwrap_err()
                .parameter(),
            Some("output.run_officejs.args.schema")
        );
    }
}

#[test]
fn framed_headers_share_lossless_json_parsing_without_modifying_the_raw_body() {
    use super::super::codec::function_arguments;

    let raw = "Write-Output \"synthetic\"\r\n$x = 'C:\\synthetic\\q'\n\t";
    let header = r#"{"field":"cmd","args":{"workdir":"C:\synthetic\q","label":"row\tvalue"}}"#;
    for newline in ["\n", "\r\n"] {
        let payload = json!(format!(
            "{}{header}{newline}{raw}",
            super::super::FUNCTION_STRING_PREFIX
        ));
        assert_eq!(
            function_arguments(&payload).unwrap(),
            json!({"cmd":raw,"workdir":r"C:\synthetic\q","label":"row\tvalue"})
        );
    }
    for malformed in [
        r#"{"field":"cmd","args":{"workdir":"synthetic\u12X4"}}"#,
        r#"{"field":"cmd","args":{"workdir":"synthetic"}"#,
    ] {
        let payload = json!(format!(
            "{}{malformed}\n{raw}",
            super::super::FUNCTION_STRING_PREFIX
        ));
        let error = function_arguments(&payload).unwrap_err();
        assert_eq!(
            error.parameter(),
            Some("output.run_officejs.args.frame.header")
        );
        assert!(!error.to_string().contains("synthetic"));
    }
}

#[test]
fn framed_separator_compatibility_preserves_every_raw_body_byte() {
    use super::super::codec::function_arguments;

    let header = r#"{"field":"cmd","args":{"workdir":"C:\\synthetic\\project"}}"#;
    let raw = " \n\\nWrite-Output \"synthetic\"\r\n$x = 'C:\\synthetic\\q'\n\t";
    for separator in ["\n", "\r\n", "\r", "\\n", "\\r\\n", "\\r"] {
        let payload = json!(format!(
            "{}{header}{separator}{raw}",
            super::super::FUNCTION_STRING_PREFIX
        ));
        assert_eq!(
            function_arguments(&payload).unwrap(),
            json!({"cmd":raw,"workdir":r"C:\synthetic\project"}),
            "separator: {separator:?}"
        );
    }
}

#[test]
fn framed_separator_compatibility_never_splits_inside_header_strings() {
    use super::super::codec::function_arguments;

    let header = json!({
        "field":"cmd",
        "args":{"label":"first\nsecond\rthird\\nliteral","nested":{"value":"}\\r\\n"}}
    });
    let raw = "\\r\\nsynthetic\n{\"field\":\"not-a-header\"}";
    for header in [
        header.to_string(),
        serde_json::to_string_pretty(&header).unwrap(),
    ] {
        let payload = json!(format!(
            "{}{header}\\n{raw}",
            super::super::FUNCTION_STRING_PREFIX
        ));
        assert_eq!(
            function_arguments(&payload).unwrap(),
            json!({"cmd":raw,"label":"first\nsecond\rthird\\nliteral","nested":{"value":"}\\r\\n"}})
        );
    }
    for suffix in ["", " ", "\\t", "x\\nsynthetic"] {
        let payload = json!(format!(
            "{}{{\"field\":\"cmd\",\"args\":{{}}}}{suffix}",
            super::super::FUNCTION_STRING_PREFIX
        ));
        assert!(function_arguments(&payload).is_err());
    }
}

#[test]
fn framed_multiline_headers_keep_json_and_raw_body_boundaries() {
    use super::super::codec::function_arguments;

    let raw = "\n\r\n\tWrite-Output \"synthetic } ]\"\nrelay-function-string-v1 {}\n";
    let arguments = json!({
        "cmd":raw,
        "metadata":{"label":"synthetic } ] \"quote\"","lines":["first\nsecond", "third"]}
    });
    let header = json!({"field":"cmd","args":{"metadata":arguments["metadata"]}});
    for newline in ["\n", "\r\n"] {
        let pretty = serde_json::to_string_pretty(&header)
            .unwrap()
            .replace('\n', newline);
        // Literal newlines in header strings already belong to BPS's lossless
        // compatibility grammar, just like those in ordinary function JSON.
        let literal = header
            .to_string()
            .replace("first\\nsecond", "first\nsecond");
        for encoded in [pretty, literal] {
            let payload = json!(format!(
                "{}{encoded}{newline}{raw}",
                super::super::FUNCTION_STRING_PREFIX
            ));
            assert_eq!(function_arguments(&payload).unwrap(), arguments);

            let mut request = request_with_tool();
            request["tools"][0]["parameters"] = json!({
                "type":"object","properties":{"cmd":{"type":"string"},"metadata":{"type":"object"}},
                "required":["cmd","metadata"],"additionalProperties":false
            });
            let response = json!({"status":"completed","output":[{
                "type":"function_call","call_id":"call_multiline_frame","name":TRANSPORT_TOOL,
                "arguments":json!({"references":["exec_command"],"code":payload}).to_string()
            }]});
            let translated = translate_response_value(response.clone(), &request).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(
                    translated["output"][0]["arguments"].as_str().unwrap()
                )
                .unwrap(),
                arguments
            );
            request["tools"][0]["parameters"]["properties"]["metadata"]["type"] = json!("string");
            assert_eq!(
                translate_response_value(response, &request)
                    .unwrap_err()
                    .parameter(),
                Some("output.run_officejs.args.schema")
            );
        }
    }
}

#[test]
fn framed_header_separator_errors_never_rewrite_executable_arguments() {
    use super::super::codec::function_arguments;

    for header in [
        r#"{"field":"cmd","args":{"label":"synthetic-private" "extra":1}}"#,
        r#"{"field":"cmd","args":{"label":"synthetic-private "quoted""}}"#,
        "{\n\"field\":\"cmd\",\n\"args\":{\"label\":\"synthetic-private\" \"extra\":1}\n}",
    ] {
        let payload = json!(format!(
            "{}{header}\nsynthetic-private executable body\n{{}}\n",
            super::super::FUNCTION_STRING_PREFIX
        ));
        let error = function_arguments(&payload).unwrap_err();
        assert_eq!(
            error.parameter(),
            Some("output.run_officejs.args.frame.header")
        );
        let diagnostic = error.to_string();
        assert!(
            diagnostic.contains("json_reason=expected_separator"),
            "{diagnostic}"
        );
        assert!(!diagnostic.contains("synthetic-private"));
    }
}

#[test]
fn framed_function_strings_preserve_commands_and_validate_the_original_schema() {
    let mut request = request_with_tool();
    request["tools"][0]["parameters"] = json!({"type":"object","properties":{"cmd":{"type":"string"},"max_output_tokens":{"type":"integer"}},"required":["cmd"],"additionalProperties":false});
    let raw = "Write-Output \"quoted\"\r\n$x = 'C:\\synthetic\\path'\n\t😀\n".repeat(16);
    let code = format!(
        "{}{}\n{raw}",
        super::super::FUNCTION_STRING_PREFIX,
        json!({"field":"cmd","args":{"max_output_tokens":123}})
    );
    let response = json!({"id":"resp_frame","status":"completed","output":[{
        "type":"function_call","id":"fc_frame","call_id":"call_frame","name":TRANSPORT_TOOL,
        "arguments":json!({"references":["exec_command"],"code":code}).to_string()
    }]});
    let bytes = translate_response(&serde_json::to_vec(&response).unwrap(), &request).unwrap();
    let translated: Value = serde_json::from_slice(&bytes).unwrap();
    let arguments: Value =
        serde_json::from_str(translated["output"][0]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments, json!({"cmd":raw,"max_output_tokens":123}));
    let nested = json!({"references":["exec_command"],"code":arguments.to_string()}).to_string();
    assert!(response["output"][0]["arguments"].as_str().unwrap().len() < nested.len());
    for (header, parameter) in [
        (
            json!({"field":"cmd","args":{"cmd":"duplicate"}}),
            "output.run_officejs.args.frame.duplicate_field",
        ),
        (
            json!({"field":"cmd","args":[]}),
            "output.run_officejs.args.frame.args",
        ),
        (
            json!({"field":false,"args":{}}),
            "output.run_officejs.args.frame.field",
        ),
        (
            json!({"field":"cmd","args":{},"extra":true}),
            "output.run_officejs.args.frame.header",
        ),
        (
            json!({"field":"cmd","args":{"max_output_tokens":"wrong type"}}),
            "output.run_officejs.args.schema",
        ),
        (
            json!({"field":"unknown","args":{}}),
            "output.run_officejs.args.schema",
        ),
        (
            json!({"field":"max_output_tokens","args":{"cmd":"synthetic"}}),
            "output.run_officejs.args.schema",
        ),
    ] {
        let mut invalid = response.clone();
        invalid["output"][0]["arguments"] = json!({"references":["exec_command"],"code":format!("{}{}\n{raw}", super::super::FUNCTION_STRING_PREFIX, header)}).to_string().into();
        let error =
            translate_response(&serde_json::to_vec(&invalid).unwrap(), &request).unwrap_err();
        assert_eq!(error.parameter(), Some(parameter));
    }
    for (malformed, parameter) in [
        (
            "relay-function-string-v1 {\"field\":\"cmd\",\"args\":{}}",
            "output.run_officejs.args.frame.separator",
        ),
        (
            "relay-function-string-v1 {invalid}\nsynthetic",
            "output.run_officejs.args.frame.header",
        ),
        (
            "relay-function-string-v1 []\nsynthetic",
            "output.run_officejs.args.frame.header",
        ),
    ] {
        let mut invalid = response.clone();
        invalid["output"][0]["arguments"] = json!({"references":["exec_command"],"code":malformed})
            .to_string()
            .into();
        assert_eq!(
            translate_response(&serde_json::to_vec(&invalid).unwrap(), &request)
                .unwrap_err()
                .parameter(),
            Some(parameter)
        );
    }
    request["tools"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"custom","name":"raw_input","format":{"type":"text"}}));
    let mut custom = response;
    custom["output"][0]["arguments"] = json!({"references":["raw_input"],"code":code})
        .to_string()
        .into();
    let translated = translate_response(&serde_json::to_vec(&custom).unwrap(), &request).unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["input"], code);
}

#[test]
fn response_restores_client_metadata_instead_of_transport_metadata() {
    for status in ["completed", "incomplete"] {
        for metadata in [
            None,
            Some(json!({"user_id":"synthetic-user","request_kind":"synthetic"})),
        ] {
            let mut request = request_with_tool();
            if let Some(metadata) = &metadata {
                request["metadata"] = metadata.clone();
            }
            let response = json!({
                "id":"resp_metadata",
                "status":status,
                "metadata":{"task_id":"transport-task","turn_id":"transport-turn","agent_iteration":"1"},
                "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"OK","annotations":[]}]}]
            });
            let translated =
                translate_response(&serde_json::to_vec(&response).unwrap(), &request).unwrap();
            let translated_value: Value = serde_json::from_slice(&translated).unwrap();
            let expected = metadata.unwrap_or_else(|| json!({}));
            assert_eq!(translated_value["metadata"], expected);
            let stream = String::from_utf8(synthetic_stream(&translated).unwrap()).unwrap();
            let response_events = stream
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .filter_map(|data| serde_json::from_str::<Value>(data).ok())
                .filter_map(|event| event.get("response").cloned())
                .collect::<Vec<_>>();
            assert_eq!(response_events.len(), 3);
            assert!(
                response_events
                    .iter()
                    .all(|event| event["metadata"] == expected)
            );
        }
    }
}

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
fn reference_envelope_accepts_the_same_direct_function_calls_as_legacy() {
    let request = request_with_tool();
    for code in [
        r#"exec_command({"cmd":"pwd"})"#,
        r#"return await exec_command({"cmd":"pwd"})"#,
    ] {
        let body = json!({"status":"completed","output":[{
            "type":"function_call","call_id":"call_direct_reference","name":TRANSPORT_TOOL,
            "arguments":json!({"references":["exec_command"],"code":code}).to_string()
        }]});
        let translated = translate_response(&serde_json::to_vec(&body).unwrap(), &request).unwrap();
        let translated: Value = serde_json::from_slice(&translated).unwrap();
        assert_eq!(translated["output"][0]["name"], "exec_command");
        assert_eq!(translated["output"][0]["arguments"], r#"{"cmd":"pwd"}"#);
    }
}

#[test]
fn referenced_direct_function_calls_keep_identity_schema_and_single_call_checks() {
    for (code, parameter) in [
        (r#"other_tool({"cmd":"pwd"})"#, "output.run_officejs.tool"),
        (
            r#"exec_command({"cmd":42})"#,
            "output.run_officejs.args.schema",
        ),
        (r#"exec_command("pwd")"#, "output.run_officejs.args.object"),
        (
            r#"exec_command({"cmd":"pwd"}); exec_command({"cmd":"ls"})"#,
            "output.run_officejs.args.json.syntax",
        ),
    ] {
        let body = json!({"status":"completed","output":[{
            "type":"function_call","call_id":"call_direct_reference","name":TRANSPORT_TOOL,
            "arguments":json!({"references":["exec_command"],"code":code}).to_string()
        }]});
        let error = translate_response(&serde_json::to_vec(&body).unwrap(), &request_with_tool())
            .unwrap_err();
        assert_eq!(error.parameter(), Some(parameter));
        assert!(!error.message().contains("pwd"));
    }
}

#[test]
fn referenced_direct_looking_custom_input_stays_opaque() {
    let request = json!({"model":"test","tools":[{
        "type":"custom","name":"raw_tool","format":{"type":"text"}
    }]});
    let code = r#"return await other_tool({"cmd":"synthetic"})"#;
    let body = json!({"status":"completed","output":[{
        "type":"function_call","call_id":"call_raw_reference","name":TRANSPORT_TOOL,
        "arguments":json!({"references":["raw_tool"],"code":code}).to_string()
    }]});
    let translated = translate_response(&serde_json::to_vec(&body).unwrap(), &request).unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["type"], "custom_tool_call");
    assert_eq!(translated["output"][0]["name"], "raw_tool");
    assert_eq!(translated["output"][0]["input"], code);
}

#[test]
fn unknown_json_string_escapes_preserve_literal_backslashes() {
    for (code, expected) in [
        (
            r#"{"cmd":"rg \s+\w+ C:\Users\Example\Work"}"#,
            r#"rg \s+\w+ C:\Users\Example\Work"#,
        ),
        (
            r#"{"cmd":"echo \'quoted\' \q \x41 \p{L} \ value"}"#,
            r#"echo \'quoted\' \q \x41 \p{L} \ value"#,
        ),
        (r#"{"cmd":"\\q \\\q"}"#, r#"\q \\q"#),
        (
            r#"{"cmd":"first\nsecond\t\q \"quoted\" \\ 😀"}"#,
            "first\nsecond\t\\q \"quoted\" \\ 😀",
        ),
    ] {
        let body = json!({"status":"completed","output":[{
            "type":"function_call","call_id":"call_literal_escape","name":TRANSPORT_TOOL,
            "arguments":json!({"references":["exec_command"],"code":code}).to_string()
        }]});
        let translated =
            translate_response(&serde_json::to_vec(&body).unwrap(), &request_with_tool()).unwrap();
        let translated: Value = serde_json::from_slice(&translated).unwrap();
        let arguments: Value =
            serde_json::from_str(translated["output"][0]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(arguments, json!({"cmd":expected}));
    }
}

#[test]
fn unknown_escape_compatibility_keeps_schema_checks_and_custom_input_opaque() {
    let mut request = request_with_tool();
    request["tools"][0]["parameters"]["properties"]["cmd"]["enum"] = json!(["allowed"]);
    let code = r#"{"cmd":"\q"}"#;
    let body = json!({"status":"completed","output":[{
        "type":"function_call","call_id":"call_escape_schema","name":TRANSPORT_TOOL,
        "arguments":json!({"references":["exec_command"],"code":code}).to_string()
    }]});
    let error = translate_response(&serde_json::to_vec(&body).unwrap(), &request).unwrap_err();
    assert_eq!(error.parameter(), Some("output.run_officejs.args.schema"));
    request["tools"][0] = json!({"type":"custom","name":"exec_command","format":{"type":"text"}});
    let translated = translate_response(&serde_json::to_vec(&body).unwrap(), &request).unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["type"], "custom_tool_call");
    assert_eq!(translated["output"][0]["input"], code);
}

#[test]
fn unknown_escape_compatibility_never_repairs_unicode_or_structure() {
    for code in [
        r#"{"cmd":"\q \u12X4"}"#,
        r#"{"cmd":"\q \uD800"}"#,
        r#"{"cmd":"\q" "extra":1}"#,
        r#"{"cmd":"\q""#,
        r#"{"cmd":"\q"} {"cmd":"other"}"#,
        "{\"cmd\":\"bad\\\ncontinuation\"}",
    ] {
        assert!(super::super::codec::function_arguments(&json!(code)).is_err());
    }
}

#[test]
fn response_preserves_literal_line_breaks_in_function_json_strings() {
    let request = request_with_tool();
    let raw = "echo \"hello\"\r\n\t😀 C:\\synthetic\\file \\n";
    let code = "{\n\"cmd\":\"echo \\\"hello\\\"\r\n\t😀 C:\\\\synthetic\\\\file \\\\n\"\n}";
    assert!(serde_json::from_str::<Value>(code).is_err());
    let response = json!({"status":"completed","output":[{
        "type":"function_call","call_id":"call_multiline","name":TRANSPORT_TOOL,
        "arguments":json!({"references":["exec_command"],"code":code}).to_string()
    }]});
    let translated = translate_response(&serde_json::to_vec(&response).unwrap(), &request).unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    let arguments: Value =
        serde_json::from_str(translated["output"][0]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments, json!({"cmd":raw}));

    let mut history = request;
    history["input"] = json!([
        {"role":"user","content":"Synthetic command"},
        translated["output"][0],
        {"type":"function_call_output","call_id":"call_multiline","output":"Synthetic result"}
    ]);
    let prepared = prepare_request(&history).unwrap();
    let call = prepared["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call")
        .unwrap();
    let envelope: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(
        super::super::codec::function_arguments(&envelope["code"]).unwrap(),
        arguments
    );
}

#[test]
fn function_json_errors_keep_safe_parser_diagnostics_without_payload() {
    for (code, reason, shape) in [
        (
            r#"{"cmd":"synthetic-private\u12X4"}"#,
            "invalid_escape",
            "json_object",
        ),
        (
            r#"{"cmd":"synthetic-private" "extra":1}"#,
            "expected_separator",
            "json_object",
        ),
        ("synthetic-private", "expected_value", "text"),
        (
            r#"{"cmd":"synthetic-private""#,
            "unexpected_eof",
            "json_object",
        ),
        (
            "```json\n{\"cmd\":\"synthetic-private\"}\n```",
            "expected_value",
            "markdown_fence",
        ),
    ] {
        let body = json!({"status":"completed","output":[{
            "type":"function_call","call_id":"call_json_diagnostic","name":TRANSPORT_TOOL,
            "arguments":json!({"references":["exec_command"],"code":code}).to_string()
        }]});
        let error = translate_response(&serde_json::to_vec(&body).unwrap(), &request_with_tool())
            .unwrap_err();
        let diagnostic = error.to_string();
        assert!(
            diagnostic.contains(&format!("json_reason={reason}")),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains(&format!("payload_shape={shape}")),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("parse_line="));
        assert!(diagnostic.contains("parse_column="));
        assert!(diagnostic.contains(&format!("payload_bytes={}", code.len())));
        assert!(!diagnostic.contains("synthetic-private"));
        assert!(!format!("{error:?}").contains("synthetic-private"));
    }
}

#[test]
fn schema_failures_identify_the_constraint_without_keys_or_values() {
    let request = json!({
        "model":"gpt-test","input":"Synthetic request",
        "tools":[{"type":"function","name":"lookup","parameters":{
            "type":"object","properties":{"synthetic-private-key":{"type":"integer"}},
            "required":["synthetic-private-key"]
        }}]
    });
    let response = json!({"status":"completed","output":[{
        "type":"function_call","call_id":"call_schema","name":TRANSPORT_TOOL,
        "arguments":json!({"references":["lookup"],"code":json!({
            "synthetic-private-key":"synthetic-private-value"
        }).to_string()}).to_string()
    }]});
    let error = translate_response(&serde_json::to_vec(&response).unwrap(), &request).unwrap_err();
    assert_eq!(error.parameter(), Some("output.run_officejs.args.schema"));
    assert!(error.to_string().contains("schema_phase=arguments"));
    assert!(error.to_string().contains("schema_keyword=type"));
    assert!(!format!("{error} {error:?}").contains("synthetic-private"));

    let mut invalid = request;
    invalid["tools"][0]["parameters"] = json!({"$ref":"https://synthetic-private.invalid/schema"});
    let error = prepare_request(&invalid).unwrap_err();
    assert_eq!(error.code(), crate::error_codes::ADAPTER_INVALID_REQUEST);
    assert_eq!(error.parameter(), Some("tools.parameters"));
    assert!(error.to_string().contains("schema_phase=compile"));
    assert!(!format!("{error} {error:?}").contains("synthetic-private"));
}

#[test]
fn reference_envelope_preserves_json_escaping_and_distinguishes_argument_failures() {
    let request = json!({
        "model":"gpt-test","input":"Synthetic tool request",
        "tools":[{"type":"function","name":"write_file","parameters":{
            "type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},
            "required":["path","content"],"additionalProperties":false
        }}]
    });
    let envelope = |code: &str| {
        serde_json::to_vec(&json!({
            "id":"resp_synthetic","status":"completed","output":[{
                "type":"function_call","id":"fc_outer","call_id":"call_synthetic","name":TRANSPORT_TOOL,
                "arguments":json!({"references":["write_file"],"code":code}).to_string()
            }]
        }))
        .unwrap()
    };
    let arguments = json!({"path":"C:\\synthetic\\quoted\"file.svg","content":"<svg>\n\t<text>\"test\" \\ &amp;</text>\r\n</svg>"});
    let translated = translate_response(&envelope(&arguments.to_string()), &request).unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(translated["output"][0]["arguments"].as_str().unwrap())
            .unwrap(),
        arguments
    );
    for (code, parameter) in [
        (
            "{\"path\":\"synthetic-private",
            "output.run_officejs.args.json.eof",
        ),
        ("[\"synthetic-private\"]", "output.run_officejs.args.object"),
        (
            "{\"path\":\"synthetic-private\"}",
            "output.run_officejs.args.schema",
        ),
        (
            "{\"path\":\"synthetic-private\",\"content\":5}",
            "output.run_officejs.args.schema",
        ),
        (
            "{\"path\":\"synthetic\nprivate\" \"content\":\"invalid\"}",
            "output.run_officejs.args.json.syntax",
        ),
        (
            "{\"path\":\"synthetic\nprivate\",\"content\":5}",
            "output.run_officejs.args.schema",
        ),
        (
            "{\"path\":\"synthetic\nprivate",
            "output.run_officejs.args.json.eof",
        ),
        (
            "{\"path\":\"synthetic\nprivate\\u12X4\",\"content\":\"invalid\"}",
            "output.run_officejs.args.json.syntax",
        ),
    ] {
        let error = translate_response(&envelope(code), &request).unwrap_err();
        assert_eq!(error.parameter(), Some(parameter));
        if parameter == "output.run_officejs.args.schema" {
            assert!(error.message().contains("JSON Schema"));
        } else {
            assert!(error.message().contains("one complete JSON object"));
        }
        assert!(!error.message().contains("synthetic-private"));
    }
}

#[test]
fn response_translates_direct_function_call() {
    let request = request_with_tool();
    let translated =
        translate_response(&tool_response(r#"exec_command({"cmd":"pwd"})"#), &request).unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["name"], "exec_command");
    assert_eq!(translated["output"][0]["arguments"], "{\"cmd\":\"pwd\"}");
}

#[test]
fn response_translates_direct_function_call_with_return_await_prefix() {
    let request = request_with_tool();
    let translated = translate_response(
        &tool_response(r#"return await exec_command({"cmd":"pwd"})"#),
        &request,
    )
    .unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["name"], "exec_command");
    assert_eq!(translated["output"][0]["arguments"], "{\"cmd\":\"pwd\"}");
}

#[test]
fn response_translates_namespaced_direct_function_call() {
    let request = json!({
        "model": "gpt-6-astra",
        "input": "Inspect the repo",
        "tools": [{
            "type": "namespace",
            "name": "functions",
            "tools": [{
                "type": "function",
                "name": "exec",
                "parameters": {
                    "type": "object",
                    "properties": {"cmd": {"type": "string"}},
                    "required": ["cmd"]
                }
            }]
        }]
    });
    let translated =
        translate_response(&tool_response(r#"functions.exec({"cmd":"pwd"})"#), &request).unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["name"], "exec");
    assert_eq!(translated["output"][0]["namespace"], "functions");
    assert_eq!(translated["output"][0]["arguments"], "{\"cmd\":\"pwd\"}");
}

#[test]
fn response_translates_direct_custom_call() {
    let request = json!({
        "model": "gpt-6-astra",
        "input": "Apply the patch",
        "tools": [{
            "type": "custom",
            "name": "apply_patch",
            "description": "Apply a patch",
            "format": {"type": "text"}
        }]
    });
    let translated = translate_response(
        &tool_response(r#"apply_patch("diff --git a/a b/a")"#),
        &request,
    )
    .unwrap();
    let translated: Value = serde_json::from_slice(&translated).unwrap();
    assert_eq!(translated["output"][0]["type"], "custom_tool_call");
    assert_eq!(translated["output"][0]["name"], "apply_patch");
    assert_eq!(translated["output"][0]["input"], "diff --git a/a b/a");
}

#[test]
fn direct_tool_call_must_be_a_single_complete_call() {
    let request = request_with_tool();
    for code in [
        r#"functions.exec_command({"cmd":"pwd"}) trailing"#,
        r#"functions.exec_command({"cmd":"pwd"}); functions.exec_command({"cmd":"ls"})"#,
    ] {
        let error = translate_response(&tool_response(code), &request).unwrap_err();
        assert_eq!(error.parameter(), Some("output.run_officejs.code"));
    }
}

#[test]
fn direct_function_call_arguments_must_be_an_object() {
    for code in [
        r#"exec_command("pwd")"#,
        r#"exec_command("{\"cmd\":\"pwd\"}")"#,
    ] {
        let error = translate_response(&tool_response(code), &request_with_tool()).unwrap_err();
        assert_eq!(error.parameter(), Some("output.run_officejs.args.object"));
    }
}

#[test]
fn direct_unknown_tool_call_is_rejected_by_the_catalog() {
    let error = translate_response(
        &tool_response(r#"unknown_tool({"cmd":"pwd"})"#),
        &request_with_tool(),
    )
    .unwrap_err();
    assert_eq!(error.parameter(), Some("output.run_officejs.tool"));
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
fn completed_tool_calls_cannot_reuse_an_identity_from_history() {
    for history in [
        json!({"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"pwd\"}"}),
        json!({"type":"function_call_output","call_id":"call_1","output":"synthetic result"}),
        json!({"type":"custom_tool_call","call_id":"call_1","name":"apply_patch","input":"synthetic patch"}),
        json!({"type":"custom_tool_call_output","call_id":"call_1","output":"synthetic result"}),
    ] {
        for input in [history.clone(), json!([history])] {
            let mut request = request_with_tool();
            request["input"] = input;
            let response = tool_response(r#"{"tool":"exec_command","args":{"cmd":"pwd"}}"#);
            let error = translate_response(&response, &request).unwrap_err();
            assert_eq!(error.parameter(), Some("output.run_officejs.call_id"));

            let mut fresh: Value = serde_json::from_slice(&response).unwrap();
            fresh["output"][0]["call_id"] = json!("call_fresh");
            assert!(translate_response(&serde_json::to_vec(&fresh).unwrap(), &request).is_ok());
        }
    }
}

#[test]
fn incomplete_json_response_preserves_usage_but_never_releases_transport_tools() {
    for tool_choice in [json!("auto"), json!("required")] {
        let mut request = request_with_tool();
        request["tool_choice"] = tool_choice;
        let mut response: Value =
            serde_json::from_slice(&tool_response("invalid-private-code")).unwrap();
        response["status"] = json!("incomplete");
        response["incomplete_details"] = json!({"reason":"max_output_tokens"});
        response["usage"] = json!({"input_tokens":17,"output_tokens":4});
        response["output"].as_array_mut().unwrap().insert(
            0,
            json!({
                "type":"message","id":"msg_partial","role":"assistant",
                "content":[{"type":"output_text","text":"Partial text"}]
            }),
        );
        let translated =
            translate_response(&serde_json::to_vec(&response).unwrap(), &request).unwrap();
        let translated: Value = serde_json::from_slice(&translated).unwrap();
        assert_eq!(translated["status"], "incomplete");
        assert_eq!(translated["output"].as_array().unwrap().len(), 1);
        assert_eq!(
            translated["output"][0]["content"][0]["text"],
            "Partial text"
        );
        assert_eq!(translated["usage"], response["usage"]);
        assert_eq!(
            translated["incomplete_details"],
            response["incomplete_details"]
        );
        let wire =
            String::from_utf8(synthetic_stream(&serde_json::to_vec(&translated).unwrap()).unwrap())
                .unwrap();
        assert!(wire.contains("response.incomplete"));
        assert!(!wire.contains("function_call"));
        assert!(!wire.contains("run_officejs"));
        assert!(!wire.contains("invalid-private-code"));
    }
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
    assert_eq!(
        error.parameter(),
        Some("output.run_officejs.args.json.syntax")
    );
}

#[test]
fn serial_tool_requests_do_not_return_multiple_client_tool_calls() {
    let mut request = request_with_tool();
    request["parallel_tool_calls"] = json!(false);
    assert!(
        translate_response(
            &tool_response(r#"{"tool":"exec_command","args":{"cmd":"pwd"}}"#),
            &request
        )
        .is_ok()
    );
    let envelope = |call_id: &str, command: &str| {
        json!({
            "type": "function_call",
            "id": format!("fc_{call_id}"),
            "call_id": call_id,
            "name": TRANSPORT_TOOL,
            "arguments": json!({
                "code": json!({"tool": "exec_command", "args": {"cmd": command}}).to_string()
            }).to_string()
        })
    };
    let body = serde_json::to_vec(&json!({
        "id": "resp_1",
        "status": "completed",
        "output": [envelope("call_1", "pwd"), envelope("call_2", "ls")]
    }))
    .unwrap();
    let error = translate_response(&body, &request).unwrap_err();
    assert_eq!(
        error.parameter(),
        Some("output.run_officejs.parallel_tool_calls")
    );
}

#[test]
fn parallel_single_tool_frames_preserve_complex_commands_and_custom_input() {
    let mut request = request_with_tool();
    request["parallel_tool_calls"] = json!(true);
    request["tools"].as_array_mut().unwrap().push(json!({
        "type":"custom","name":"raw_input","format":{"type":"text"}
    }));
    let commands = [
        "Write-Output \"quoted\"\r\n$path = 'C:\\synthetic\\a'\n",
        "$json = '{\"items\":[1,2]}'\nWrite-Output \"кириллица 😀\"\n",
    ];
    let mut calls = commands.iter().enumerate().map(|(index, command)| json!({
        "type":"function_call","call_id":format!("call_frame_{index}"),"name":TRANSPORT_TOOL,
        "arguments":json!({"references":["exec_command"],"code":format!(
            "{}{}\n{command}", super::super::FUNCTION_STRING_PREFIX,
            json!({"field":"cmd","args":{}})
        )}).to_string()
    })).collect::<Vec<_>>();
    let raw =
        "*** Begin Patch\n*** Add File: synthetic.txt\n+literal \\\"text\\\"\n*** End Patch\n";
    calls.push(json!({
        "type":"function_call","call_id":"call_raw","name":TRANSPORT_TOOL,
        "arguments":json!({"references":["raw_input"],"code":raw}).to_string()
    }));
    let body = serde_json::to_vec(&json!({
        "id":"resp_parallel_frames","status":"completed","output":calls
    }))
    .unwrap();
    let translated: Value =
        serde_json::from_slice(&translate_response(&body, &request).unwrap()).unwrap();
    assert_eq!(translated["output"].as_array().unwrap().len(), 3);
    for (index, command) in commands.iter().enumerate() {
        let call = &translated["output"][index];
        assert_eq!(call["call_id"], format!("call_frame_{index}"));
        assert_eq!(call["name"], "exec_command");
        let args: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args["cmd"], *command);
    }
    assert_eq!(translated["output"][2]["call_id"], "call_raw");
    assert_eq!(translated["output"][2]["type"], "custom_tool_call");
    assert_eq!(translated["output"][2]["input"], raw);

    request["parallel_tool_calls"] = json!(false);
    assert_eq!(
        translate_response(&body, &request).unwrap_err().parameter(),
        Some("output.run_officejs.parallel_tool_calls")
    );
}

#[test]
fn parallel_responses_keep_repeated_tool_names_and_distinct_call_ids() {
    for parallel in [None, Some(true)] {
        let mut request = request_with_tool();
        if let Some(parallel) = parallel {
            request["parallel_tool_calls"] = json!(parallel);
        }
        let calls: Vec<_> = (0..3).map(|index| json!({
            "type":"function_call", "id":format!("fc_batch_{index}"),
            "call_id":format!("call_batch_{index}"), "name":TRANSPORT_TOOL,
            "arguments":json!({"references":["exec_command"],
                "code":json!({"cmd":format!("synthetic-read-{index}")}).to_string()}).to_string()
        })).collect();
        let body = serde_json::to_vec(&json!({
            "id":"resp_batch", "status":"completed", "output":calls
        }))
        .unwrap();
        let translated: Value =
            serde_json::from_slice(&translate_response(&body, &request).unwrap()).unwrap();
        assert_eq!(translated["output"].as_array().unwrap().len(), 3);
        for (index, call) in translated["output"].as_array().unwrap().iter().enumerate() {
            assert_eq!(call["call_id"], format!("call_batch_{index}"));
            assert_eq!(call["name"], "exec_command");
            let args: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
            assert_eq!(args["cmd"], format!("synthetic-read-{index}"));
        }
    }
}

#[test]
fn tool_call_identity_rejects_blank_values_without_normalizing_opaque_ids() {
    for call_id in [
        "",
        "   ",
        "\t\r\n",
        "call_synthetic",
        "call_provider.1.0",
        " call_with_padding ",
    ] {
        let mut response: Value = serde_json::from_slice(&tool_response(
            r#"{"tool":"exec_command","args":{"cmd":"synthetic"}}"#,
        ))
        .unwrap();
        response["output"][0]["call_id"] = json!(call_id);
        let translated = translate_response(
            &serde_json::to_vec(&response).unwrap(),
            &request_with_tool(),
        );
        if call_id.trim().is_empty() {
            assert_eq!(
                translated.unwrap_err().parameter(),
                Some("output.run_officejs.call_id")
            );
        } else {
            let translated: Value = serde_json::from_slice(&translated.unwrap()).unwrap();
            assert_eq!(translated["output"][0]["call_id"], call_id);
        }
    }
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
fn current_tools_override_history_and_keep_namespaces() {
    let request = json!({
        "model": "gpt-6-astra",
        "tools": [{"type":"namespace","name":"functions","tools":[
            {"type":"custom","name":"exec","description":"LATEST_DEFINITION","format":{"type":"text"}}
        ]}],
        "input": [
            {"type":"additional_tools","tools":[{"type":"namespace","name":"clock","tools":[
                {"type":"function","name":"sleep"}
            ]}]},
            {"type":"additional_tools","tools":[{"type":"namespace","name":"functions","tools":[
                {"type":"function","name":"exec","description":"OLD_DEFINITION"}
            ]}]},
            {"role":"user","content":[{"type":"input_text","text":"Run a command"}]}
        ],
        "tool_choice": {"type":"allowed_tools","mode":"required","tools":[
            {"type":"custom","namespace":"functions","name":"exec"}
        ]}
    });
    let prepared = prepare_request(&request).unwrap();
    let instructions = current_catalog_text(&prepared);
    assert!(instructions.contains("functions.exec (custom): LATEST_DEFINITION"));
    assert!(!instructions.contains("OLD_DEFINITION"));
    assert!(instructions.contains("clock.sleep"));
    let denied = tool_response(r#"{"tool":"clock.sleep","args":{}}"#);
    assert_eq!(
        translate_response(&denied, &request)
            .unwrap_err()
            .parameter(),
        Some("output.run_officejs.tool_choice")
    );
    assert!(
        prepared["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| { item.get("type").and_then(Value::as_str) != Some("additional_tools") })
    );

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
            let replayed = items
                .iter()
                .find(|item| item["type"] == "function_call")
                .unwrap();
            assert_eq!(replayed["name"], TRANSPORT_TOOL, "{kind}");
            assert_eq!(replayed["call_id"], call["call_id"], "{kind}");
            let arguments: Value =
                serde_json::from_str(replayed["arguments"].as_str().unwrap()).unwrap();
            assert_eq!(arguments["references"][0], payload, "{kind}");
            let output = items
                .iter()
                .find(|item| item["type"] == "function_call_output")
                .unwrap();
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
    assert!(
        translate_response(
            serde_json::to_string(&response).unwrap().as_bytes(),
            &request
        )
        .is_err()
    );
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

#[test]
fn model_switch_shrinks_history_ids_without_splitting_tool_pairs() {
    let call_id = "g".repeat(81);
    let message_id = format!("msg_{}", "m".repeat(80));
    assert_eq!(format!("fc_{call_id}").len(), 84);
    assert!(message_id.len() > 64);
    let request = json!({
        "model": "gpt-6-luna",
        "input": [
            {
                "type": "message",
                "id": message_id,
                "role": "user",
                "content": [{"type": "input_text", "text": "earlier"}]
            },
            {
                "type": "function_call",
                "id": format!("fc_{call_id}"),
                "call_id": call_id,
                "name": "exec_command",
                "arguments": "{\"cmd\":\"pwd\"}"
            },
            {
                "type": "function_call_output",
                "call_id": call_id,
                "output": "ok"
            }
        ],
        "tools": [{
            "type": "function",
            "name": "exec_command",
            "description": "Run a command",
            "parameters": {"type": "object"}
        }]
    });
    let prepared = prepare_request(&request).unwrap();
    let items = prepared["input"].as_array().unwrap();
    for item in items {
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            assert!(id.len() <= 64, "{id}");
        }
        if let Some(id) = item.get("call_id").and_then(Value::as_str) {
            assert!(id.len() <= 64, "{id}");
        }
    }
    let call = items
        .iter()
        .find(|item| item["type"] == "function_call" && item["name"] == TRANSPORT_TOOL)
        .unwrap();
    let output = items
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap();
    assert_eq!(call["call_id"], output["call_id"]);
    assert_eq!(call["id"], output["id"]);
    assert!(call["id"].as_str().unwrap().starts_with("fc_"));
    assert_ne!(call["call_id"], call_id);
    let message = items
        .iter()
        .find(|item| item["role"] == "user" && item["content"][0]["text"] == "earlier")
        .unwrap();
    assert!(message["id"].as_str().unwrap().starts_with("msg_"));
    assert_ne!(message["id"], message_id);

    let again = prepare_request(&request).unwrap();
    let again_call = again["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call" && item["name"] == TRANSPORT_TOOL)
        .unwrap();
    assert_eq!(call["id"], again_call["id"]);
    assert_eq!(call["call_id"], again_call["call_id"]);
}

#[test]
fn web_search_ids_keep_the_ws_prefix_when_history_is_shrunk() {
    let long_native = format!("ws_{}", "w".repeat(80));
    let long_foreign = "x".repeat(80);
    let bare_hash = "4b6dcef06ba031e395d715dca0989454";
    let request = json!({
        "model": "gpt-6-luna",
        "input": [
            {"type": "web_search_call", "id": long_native, "status": "completed"},
            {"type": "web_search_call", "id": long_foreign, "status": "completed"},
            {"type": "web_search_call", "id": bare_hash, "status": "completed"}
        ]
    });
    let prepared = prepare_request(&request).unwrap();
    let ids: Vec<&str> = prepared["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "web_search_call")
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 3);
    for id in &ids {
        assert!(id.starts_with("ws_"), "{id}");
        assert!(id.len() <= 64, "{id}");
        assert_ne!(*id, bare_hash);
    }
    assert_ne!(ids[0], long_native);
    assert_ne!(ids[1], long_foreign);
    let again = prepare_request(&request).unwrap();
    let again_ids: Vec<&str> = again["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "web_search_call")
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, again_ids);
}

#[test]
fn maximum_reasoning_and_ciphertext_stay_on_the_first_basis_points_attempt() {
    let request = json!({
        "model": "gpt-6-luna",
        "reasoning": {"effort": "max"},
        "input": [
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "continue"}]},
            {
                "type": "reasoning",
                "id": "rs_31297b9f6a839c76b27b068f1efb7bd2",
                "encrypted_content": "synthetic-ciphertext",
                "summary": [{"type": "summary_text", "text": "Checked the previous result."}]
            }
        ]
    });
    let prepared = prepare_request(&request).unwrap();
    assert_eq!(prepared["reasoning_effort"], "xhigh");
    let reasoning = prepared["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "reasoning")
        .unwrap();
    assert_eq!(reasoning["encrypted_content"], "synthetic-ciphertext");
    assert_eq!(reasoning["id"], "rs_31297b9f6a839c76b27b068f1efb7bd2");
    assert_eq!(
        reasoning["summary"][0]["text"],
        "Checked the previous result."
    );
}

#[test]
fn reasoning_summary_without_ciphertext_stays_in_history() {
    let request = json!({
        "model": "gpt-6-luna",
        "reasoning": {"effort": "ultra"},
        "input": [{
            "type": "reasoning",
            "id": "rs_visible",
            "summary": [{"type": "summary_text", "text": "The file was already checked."}]
        }]
    });
    let prepared = prepare_request(&request).unwrap();
    assert_eq!(prepared["reasoning_effort"], "ultra");
    let reasoning = prepared["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "reasoning")
        .unwrap();
    assert!(reasoning.get("encrypted_content").is_none());
    assert_eq!(
        reasoning["summary"][0]["text"],
        "The file was already checked."
    );
}
