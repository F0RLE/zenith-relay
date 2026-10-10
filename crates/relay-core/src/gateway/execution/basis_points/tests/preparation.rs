use super::*;
use serde_json::json;

#[test]
fn encrypted_agent_assignment_is_rejected_before_technical_history_is_removed() {
    for input in [
        json!({"type":"agent_message","encrypted_content":"synthetic-ciphertext","content":[]}),
        json!([
            {"type":"agent_message","encrypted_content":"synthetic-ciphertext"},
            {"type":"tool_search_output","tools":[]},
            {"type":"additional_tools","tools":[]},
            {"type":"compaction_trigger"}
        ]),
        json!([
            {"type":"agent_message","content":[{"type":"encrypted_content","encrypted_content":"synthetic-ciphertext"}]},
            {"role":"user","content":"next"}
        ]),
    ] {
        let request = json!({"model":"gpt-test","input":input});
        assert_eq!(
            prepare_request(&request).unwrap_err().parameter(),
            Some("input.agent_message.encrypted_content")
        );
    }
}

#[test]
fn latest_historical_search_catalog_keeps_nested_namespaces_and_original_schema() {
    let request = json!({
        "model":"gpt-test",
        "input":[
            {"type":"additional_tools","tools":[{"type":"namespace","name":"mcp","tools":[
                {"type":"namespace","name":"repo","tools":[{"type":"function","name":"read","description":"old"}]}
            ]}]},
            {"type":"tool_search_output","tools":[{"type":"namespace","name":"mcp","tools":[
                {"type":"namespace","name":"repo","tools":[{"type":"function","name":"read","description":"latest","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}]}
            ]}]},
            {"role":"user","content":"Read a file"}
        ],
        "tool_choice":{"type":"function","namespace":"mcp.repo","name":"read"}
    });
    let before = request.clone();
    let prepared = prepare_request(&request).unwrap();
    let instructions = prepared["input"][0]["content"][0]["text"].as_str().unwrap();
    assert!(instructions.contains("mcp.repo.read (function): latest"));
    assert!(!instructions.contains("mcp.repo.read (function): old"));
    assert_eq!(request, before);
    assert_eq!(prepared["input"].as_array().unwrap().len(), 2);
    let response = serde_json::to_vec(&json!({"status":"completed","output":[{
        "type":"function_call","name":"run_officejs","call_id":"call_fixture",
        "arguments":json!({"references":["mcp.repo.read"],"code":r#"{"path":"src/main.rs"}"#}).to_string()
    }]})).unwrap();
    let translated: Value =
        serde_json::from_slice(&translate_response(&response, &request).unwrap()).unwrap();
    assert_eq!(translated["output"][0]["namespace"], "mcp.repo");
    assert_eq!(translated["output"][0]["name"], "read");
}

#[test]
fn preparation_wraps_tools_and_omits_empty_context() {
    let mut request = request_with_tool();
    request["context_management"] = json!([]);
    let prepared = prepare_request(&request).unwrap();
    assert_eq!(prepared["stream"], false);
    assert!(prepared.get("reasoning_effort").is_none());
    assert!(prepared.get("context_management").is_none());
    assert!(prepared.get("tools").is_none());
    assert!(prepared.get("tool_choice").is_none());
    assert!(prepared["input"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("exec_command"));
}

#[test]
fn tool_examples_follow_the_declared_tool_instead_of_a_fixed_patch() {
    let prepared = prepare_request(&request_with_tool()).unwrap();
    let instructions = prepared["input"][0]["content"][0]["text"].as_str().unwrap();
    assert!(instructions.contains("Example outer arguments for exec_command (function)"));
    assert!(instructions.contains("\"cmd\":string"));
    assert!(!instructions.contains("Example outer arguments for apply_patch"));
    assert!(!instructions.contains("*** Begin Patch"));

    let function_patch = json!({
        "model": "gpt-6-astra",
        "input": "edit",
        "tools": [{
            "type": "function",
            "name": "apply_patch",
            "parameters": {
                "type": "object",
                "properties": {"patch": {"type": "string"}},
                "required": ["patch"]
            }
        }]
    });
    let instructions = prepare_request(&function_patch).unwrap()["input"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(instructions.contains("Example outer arguments for apply_patch (function)"));
    assert!(instructions.contains("\\\"patch\\\""));
    assert!(!instructions.contains("exec_command"));

    let custom_patch = json!({
        "model": "gpt-6-astra",
        "input": "edit",
        "tools": [{"type": "custom", "name": "apply_patch"}]
    });
    let instructions = prepare_request(&custom_patch).unwrap()["input"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(instructions.contains("Example outer arguments for apply_patch (custom)"));
    assert!(instructions.contains("*** Begin Patch"));
}

#[test]
fn tool_catalog_is_one_message_and_examples_follow_the_bare_name() {
    let request = json!({
        "model": "gpt-6-astra",
        "input": "edit",
        "tools": [
            {"type": "custom", "name": "zeta_tool", "format": "raw"},
            {
                "type": "function",
                "name": "exec_command",
                "parameters": {
                    "type": "object",
                    "properties": {"cmd": {"type": "string"}},
                    "required": ["cmd"],
                    "additionalProperties": false
                }
            },
            {"type": "custom", "name": "apply_patch", "format": {"type": "grammar"}},
            {
                "type": "function",
                "name": "my.exec_command",
                "parameters": {
                    "type": "object",
                    "properties": {"cmd": {"type": "string"}},
                    "required": ["cmd"]
                }
            },
            {
                "type": "namespace",
                "name": "mcp__fixture",
                "tools": [{
                    "type": "function",
                    "name": "_apply_patch",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"patch": {"type": "string"}},
                        "required": ["patch"],
                        "additionalProperties": false
                    }
                }]
            }
        ]
    });
    let prepared = prepare_request(&request).unwrap();
    let input = prepared["input"].as_array().unwrap();
    let instructions = input[0]["content"][0]["text"].as_str().unwrap();
    assert_eq!(input[0]["role"], "developer");
    assert!(instructions.contains("Example outer arguments for exec_command (function)"));
    assert!(
        instructions.contains("Example outer arguments for mcp__fixture._apply_patch (function)")
    );
    assert!(instructions.contains("Example outer arguments for apply_patch (custom)"));
    assert!(!instructions.contains("Example outer arguments for my.exec_command"));
    assert!(!instructions.contains("Example outer arguments for zeta_tool"));
    assert!(instructions.contains("zeta_tool (custom). It receives raw text in input."));
    assert!(
        !instructions.contains("zeta_tool (custom). It receives raw text in input. Input format")
    );
    assert!(instructions.contains("Input format: {\"type\":\"grammar\"}"));
    assert!(instructions.contains("not JSON"));
    assert!(
        instructions.contains("do not route references to run_officejs or functions.run_officejs")
    );
    let apply = instructions.find("apply_patch (custom)").unwrap();
    let zeta = instructions.find("zeta_tool (custom)").unwrap();
    assert!(apply < zeta);
    assert!(!instructions.contains("Reminder:"));
    assert_eq!(input.len(), 2);
    assert_eq!(input[1]["role"], "user");
}

#[test]
fn boolean_property_schema_is_valid_with_closed_additional_properties() {
    let open = json!({
        "model": "gpt-6-astra",
        "input": "edit",
        "tools": [{
            "type": "function",
            "name": "exec_command",
            "inputSchema": {
                "type": "object",
                "properties": {"cmd": true},
                "required": ["cmd"]
            }
        }]
    });
    let open_instructions = prepare_request(&open).unwrap()["input"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(open_instructions.contains("Example outer arguments for exec_command (function)"));

    let closed = json!({
        "model": "gpt-6-astra",
        "input": "edit",
        "tools": [{
            "type": "function",
            "name": "exec_command",
            "input_schema": {
                "type": "object",
                "properties": {"cmd": true},
                "required": ["cmd"],
                "additionalProperties": false
            }
        }]
    });
    let closed_instructions = prepare_request(&closed).unwrap()["input"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(closed_instructions.contains("Example outer arguments"));
}

#[test]
fn disabled_tools_do_not_teach_the_transport() {
    let request = json!({
        "model": "gpt-6-astra",
        "input": "hello",
        "tools": [{
            "type": "function",
            "name": "exec_command",
            "parameters": {"type": "object", "properties": {"cmd": {"type": "string"}}, "required": ["cmd"]}
        }],
        "tool_choice": "none"
    });
    let prepared = prepare_request(&request).unwrap();
    let input = prepared["input"].as_array().unwrap();
    let text = input[0]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Return the answer as assistant text."));
    assert!(!text.contains("run_officejs"));
    assert_eq!(input.len(), 2);
    assert_eq!(input[1]["role"], "user");
}

#[test]
fn structured_text_format_is_rejected_instead_of_dropped() {
    let mut request = request_with_tool();
    request["text"] = json!({"format": {"type": "json_schema", "name": "answer"}});
    assert_eq!(
        prepare_request(&request).unwrap_err().parameter(),
        Some("text.format")
    );
    request["text"] = json!({"format": {"type": "text"}});
    assert!(prepare_request(&request).is_ok());
}

#[test]
fn encrypted_agent_message_is_rejected_for_single_item_input_too() {
    let request = json!({
        "model": "gpt-6-astra",
        "input": {
            "type": "agent_message",
            "content": [{"type": "encrypted_content", "data": "synthetic-ciphertext"}]
        }
    });
    let error = prepare_request(&request).unwrap_err();
    assert_eq!(
        error.parameter(),
        Some("input.agent_message.encrypted_content")
    );
}

#[test]
fn client_tool_cannot_collide_with_the_basis_points_transport_name() {
    let request = json!({
        "model": "gpt-6-astra",
        "input": "hello",
        "tools": [{"type": "function", "name": "run_officejs"}]
    });
    let error = prepare_request(&request).unwrap_err();
    assert_eq!(error.parameter(), Some("tools"));

    let namespaced = json!({
        "model": "gpt-6-astra",
        "input": "hello",
        "tools": [{"type": "namespace", "name": "functions", "tools": [
            {"type": "function", "name": "run_officejs"}
        ]}]
    });
    assert_eq!(
        prepare_request(&namespaced).unwrap_err().parameter(),
        Some("tools")
    );
}

#[test]
fn preparation_preserves_stream_flag_without_forwarding_tool_schema() {
    let mut request = request_with_tool();
    request["stream"] = json!(true);
    request["tool_choice"] = json!("required");
    let prepared = prepare_request(&request).unwrap();
    assert_eq!(prepared["stream"], true);
    assert!(prepared.get("tools").is_none());
    assert!(prepared.get("tool_choice").is_none());
}

#[test]
fn preparation_normalizes_max_effort_and_drops_strictly_unsupported_fields() {
    let mut request = request_with_tool();
    request["reasoning"] = json!({"effort": "max"});
    request["max_output_tokens"] = json!(4096);
    request["parallel_tool_calls"] = json!(true);
    request["temperature"] = json!(0.2);
    request["metadata"] = json!({"request_kind": "codex", "nested": {"drop": true}});

    let prepared = prepare_request(&request).unwrap();
    assert_eq!(prepared["model_selection"], "explicit");
    assert_eq!(prepared["reasoning_effort"], "xhigh");
    assert!(prepared.get("max_output_tokens").is_none());
    assert!(prepared.get("parallel_tool_calls").is_none());
    assert!(prepared.get("temperature").is_none());
    assert_eq!(prepared["metadata"]["request_kind"], "codex");
    assert!(prepared["metadata"].get("nested").is_none());
}

#[test]
fn preparation_adds_stable_basis_points_turn_metadata() {
    let request = json!({
        "model": "gpt-6-astra",
        "prompt_cache_key": "conversation-1",
        "input": [
            {"role":"user","content":[{"type":"input_text","text":"hello"}]},
            {"role":"assistant","content":[{"type":"output_text","text":"hi"}]},
            {"role":"user","content":[{"type":"input_text","text":"continue"}]},
            {"type":"function_call_output","call_id":"call_1","output":"done"}
        ],
        "metadata": {"request_kind":"codex","nested":{"drop":true}}
    });
    let first = prepare_request(&request).unwrap();
    let second = prepare_request(&request).unwrap();
    assert_eq!(first["metadata"], second["metadata"]);
    assert!(first["metadata"]["task_id"].as_str().unwrap().contains('-'));
    assert!(first["metadata"]["turn_id"].as_str().unwrap().contains('-'));
    assert_eq!(first["metadata"]["agent_iteration"], "2");
    assert_eq!(first["metadata"]["request_kind"], "codex");
    assert!(first["metadata"].get("nested").is_none());
}

#[test]
fn unsupported_opaque_continuation_is_not_silently_dropped() {
    let mut request = request_with_tool();
    request["previous_response_id"] = json!("resp_previous");
    let error = prepare_request(&request).unwrap_err();
    assert_eq!(error.parameter(), Some("previous_response_id"));
}

#[test]
fn basis_points_forwards_encrypted_history_on_the_first_attempt() {
    let request = json!({
        "model": "gpt-6-luna",
        "input": [
            {"id":"rs_foreign","type":"reasoning","encrypted_content":"foreign-reasoning","summary":[{"type":"summary_text","text":"old"}]},
            {"id":"cmp_foreign","type":"compaction","encrypted_content":"foreign-compaction"},
            {"id":"cmp_summary","type":"compaction_summary","encrypted_content":"foreign-summary"},
            {"id":"rs_nested","encrypted_content":{"blob":"foreign-nested"}},
            {"id":"cmp_plain","type":"compaction","summary":[]},
            {"id":"rs_empty","type":"reasoning","encrypted_content":"  ","summary":[]},
            {"role":"assistant","content":[{"type":"output_text","text":"previous answer"}]},
            {"type":"function_call","call_id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"printf\"}"},
            {"role":"user","content":[
                {"type":"input_text","text":"Generate an SVG of a pelican riding a bicycle"},
                {"type":"input_image","image_url":"data:image/png;base64,aaaa"}
            ]}
        ]
    });
    let first = prepare_request(&request).unwrap().to_string();
    assert!(first.contains("foreign-reasoning"));
    assert!(first.contains("foreign-compaction"));
    assert!(first.contains("foreign-summary"));
    assert!(first.contains("foreign-nested"));
    assert!(first.contains("cmp_plain"));
    assert!(!first.contains("rs_empty"));
    assert!(first.contains("previous answer"));
    assert!(first.contains("pelican"));
    assert!(first.contains("input_image"));
    assert!(first.contains("exec_command"));
}
