use super::*;
use serde_json::json;

#[test]
fn tool_prompt_is_compact_but_keeps_the_original_schema_for_validation() {
    let schema = json!({"type":"object","properties":{
        "path":{"type":"string","description":"File path","minLength":1},
        "limit":{"type":"integer","minimum":1,"maximum":100},
        "options":{"type":"object","properties":{"enabled":{"type":"boolean"}},"required":["enabled"],"additionalProperties":false}
    },"required":["path","options"],"additionalProperties":false});
    let request = json!({"model":"gpt-test","input":"Find a file","tools":[{"type":"function","name":"lookup","parameters":schema}]});
    let original = request.clone();
    let prepared = prepare_request(&request).unwrap();
    let instructions = prepared["input"][0]["content"][0]["text"].as_str().unwrap();
    assert!(instructions.contains("complete argument contract"));
    assert!(instructions.contains(". Parameters: "));
    assert!(instructions.contains("\"path\":string @"));
    assert!(instructions.contains("\"minLength\":1"));
    assert_eq!(
        instructions
            .matches("\"additionalProperties\":false")
            .count(),
        2
    );
    assert!(instructions.contains("not options borrowed from another tool or a historical call"));
    assert_eq!(request, original);
}

#[test]
fn reordered_schema_keywords_keep_compact_and_discovery_prompts_identical() {
    let first = json!({
        "type":"object","properties":{
            "limit":{"type":"integer","description":"Synthetic limit","minimum":1,"maximum":10}
        },"required":["limit"],"description":"Synthetic tool","additionalProperties":false
    });
    let reordered = json!({
        "description":"Synthetic tool","additionalProperties":false,"required":["limit"],
        "properties":{
            "limit":{"description":"Synthetic limit","maximum":10,"minimum":1,"type":"integer"}
        },"type":"object"
    });
    for mode in ["compact", "discovery"] {
        let mut request = json!({
            "model":"gpt-test","relay_bps_tool_catalog":mode,
            "input":[
                {"role":"user","content":"Synthetic request"},
                {"type":"function_call","name":"lookup","call_id":"synthetic_call","arguments":"{\"limit\":1}"},
                {"type":"function_call_output","call_id":"synthetic_call","output":"Synthetic result"}
            ],
            "tools":[{"type":"function","name":"lookup","parameters":first}]
        });
        let prepared = prepare_request(&request).unwrap();
        request["tools"][0]["parameters"] = reordered.clone();
        assert_eq!(prepare_request(&request).unwrap(), prepared, "mode={mode}");
    }
}

#[test]
fn preparation_expands_compact_responses_messages_without_losing_content_or_extensions() {
    let request = json!({
        "model":"gpt-test",
        "input":[
            {"role":"system","content":"Synthetic instructions"},
            {"role":"user","content":[{"type":"input_text","text":"Hello"},{"type":"input_image","image_url":"https://example.test/image.png"}]},
            {"role":"assistant","content":"Ready","phase":"final_answer"},
            {"type":"message","role":"user","content":"Continue"}
        ]
    });
    let before = request.clone();
    let prepared = prepare_request(&request).unwrap();
    let history = &prepared["input"].as_array().unwrap()[1..];
    assert!(history.iter().all(|item| item["type"] == "message"));
    assert_eq!(
        history[0]["content"],
        json!([{"type":"input_text","text":"Synthetic instructions"}])
    );
    assert_eq!(history[1]["content"], request["input"][1]["content"]);
    assert_eq!(
        history[2]["content"],
        json!([{"type":"output_text","text":"Ready"}])
    );
    assert_eq!(history[2]["phase"], "final_answer");
    assert_eq!(
        history[3]["content"],
        json!([{"type":"input_text","text":"Continue"}])
    );
    assert_eq!(request, before);
}

#[test]
fn preparation_normalizes_messages_bridge_history_before_basis_points_dispatch() {
    use crate::protocol::{AdapterRequestContext, MessagesReasoningMode, SourceAdapter};
    use crate::{CacheWriteTtl, WireApi};

    for stream in [false, true] {
        let request = json!({
            "model":"gpt-test","max_tokens":128,"stream":stream,
            "metadata":{"user_id":"synthetic-user"},
            "system":[{"type":"text","text":"Synthetic instructions","cache_control":{"type":"ephemeral"}}],
            "messages":[
                {"role":"user","content":"Hello"},
                {"role":"assistant","content":"Ready"},
                {"role":"user","content":"Continue"}
            ]
        });
        let bridged = SourceAdapter::MessagesToResponses
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request: &request,
                model: "gpt-test",
                stream,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "synthetic-source",
                response_id_seed: "synthetic-request",
            })
            .unwrap();
        let prepared = prepare_request(bridged.upstream_body()).unwrap();
        let items = prepared["input"].as_array().unwrap();
        assert!(items.iter().all(|item| item["type"] == "message"));
        assert_eq!(
            items.iter().filter(|item| item["role"] == "user").count(),
            2
        );
        assert_eq!(
            items
                .iter()
                .find(|item| item["role"] == "assistant")
                .unwrap()["content"],
            json!([{"type":"output_text","text":"Ready"}])
        );
        assert!(prepared["metadata"].get("user_id").is_none());
        assert_eq!(prepared["metadata"].as_object().unwrap().len(), 3);
        assert_eq!(request["metadata"]["user_id"], "synthetic-user");
        assert_eq!(prepared["stream"], stream);
    }
}

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
        "parallel_tool_calls": false,
        "tool_choice":{"type":"function","namespace":"mcp.repo","name":"read"}
    });
    let before = request.clone();
    let prepared = prepare_request(&request).unwrap();
    let instructions = prepared["input"][0]["content"][0]["text"].as_str().unwrap();
    assert!(instructions.contains("mcp.repo.read (function): latest"));
    assert!(!instructions.contains("mcp.repo.read (function): old"));
    assert!(!instructions.contains("Client tool_choice:"));
    let turn_policy = prepared["input"].as_array().unwrap().last().unwrap()["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(turn_policy.contains("Invoke at most one client tool in this response."));
    assert!(turn_policy.contains(r#""name":"read""#));
    assert_eq!(request, before);
    assert_eq!(prepared["input"].as_array().unwrap().len(), 3);
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
fn turn_tool_controls_preserve_the_prepared_history_prefix() {
    let mut request = request_with_tool();
    request["input"] = json!([
        {"role":"user","content":"Synthetic request"},
        {"role":"assistant","content":"Synthetic previous answer"},
        {"role":"user","content":"Synthetic follow-up"}
    ]);
    request.as_object_mut().unwrap().remove("tool_choice");
    request
        .as_object_mut()
        .unwrap()
        .remove("parallel_tool_calls");
    let baseline = prepare_request(&request).unwrap();
    let prefix = baseline["input"].as_array().unwrap();
    let instructions = prefix[0]["content"][0]["text"].as_str().unwrap();
    assert!(instructions
        .contains("related read-only shell inspections in one declared shell-tool invocation"));
    assert!(instructions.contains("preserve each command's output and failure"));
    assert!(instructions.contains("Do not batch across approval boundaries"));
    for choice in ["auto", "required"] {
        for parallel in [false, true] {
            let mut changed = request.clone();
            changed["tool_choice"] = json!(choice);
            changed["parallel_tool_calls"] = json!(parallel);
            let prepared = prepare_request(&changed).unwrap();
            let items = prepared["input"].as_array().unwrap();
            assert_eq!(&items[..prefix.len()], prefix.as_slice());
            if choice == "auto" && parallel {
                assert_eq!(items, prefix);
                continue;
            }
            assert_eq!(items.len(), prefix.len() + 1);
            assert_eq!(items.last().unwrap()["role"], "developer");
            let policy = items.last().unwrap()["content"][0]["text"]
                .as_str()
                .unwrap();
            assert_eq!(policy.contains("Client tool_choice:"), choice != "auto");
            if choice != "auto" {
                assert!(policy.contains(&format!("Client tool_choice: \"{choice}\".")));
            }
            assert_eq!(policy.contains("at most one client tool"), !parallel);
            assert!(prepared.get("parallel_tool_calls").is_none());
        }
    }
}

#[test]
fn text_only_prefix_keeps_history_but_never_restores_callable_tools() {
    let mut request = request_with_tool();
    request["instructions"] = json!("Preserve synthetic project constraints.");
    request["reasoning"] = json!({"effort":"xhigh"});
    request["prompt_cache_key"] = json!("synthetic-session");
    request["input"] = json!([
        {"role":"developer","content":"Synthetic project rules"},
        {"role":"user","content":"Inspect synthetic data"},
        {"type":"function_call","call_id":"call_synthetic","name":"exec_command","arguments":"{\"cmd\":\"echo synthetic\"}"},
        {"type":"function_call_output","call_id":"call_synthetic","output":"synthetic"}
    ]);
    let regular = prepare_request(&request).unwrap();
    let prefix = regular["input"][1]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    request.as_object_mut().unwrap().remove("tools");
    let mut summary = prepare_request(&request).unwrap();
    super::super::prepare::restore_text_only_prefix(&request, &mut summary, prefix);
    let items = summary["input"].as_array().unwrap();
    assert_eq!(
        &items[..items.len() - 1],
        regular["input"].as_array().unwrap()
    );
    assert!(items.last().unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("No client tools are available"));
    assert_eq!(summary["reasoning_effort"], regular["reasoning_effort"]);
    assert_eq!(summary["metadata"], regular["metadata"]);
    assert_eq!(summary["prompt_cache_key"], regular["prompt_cache_key"]);
    assert!(summary.get("tools").is_none());
    let call = json!({"status":"completed","output":[{"type":"function_call","name":"run_officejs","call_id":"call_not_allowed","arguments":json!({"references":["exec_command"],"code":"{\"cmd\":\"echo synthetic\"}"}).to_string()}]});
    assert!(translate_response(&serde_json::to_vec(&call).unwrap(), &request).is_err());
}

#[test]
fn grouped_shell_inspections_keep_one_call_and_replay_without_rewriting_commands() {
    let arguments = json!({"cmd":"Get-Location\nGet-ChildItem -LiteralPath . -Name"});
    let code = super::super::codec::encode_function_arguments(&arguments).unwrap();
    let native = json!({"status":"completed","output":[{
        "type":"function_call", "name":"run_officejs", "call_id":"call_inspections",
        "arguments":json!({"references":["exec_command"],"code":code}).to_string()
    }]});
    for parallel in [None, Some(false), Some(true)] {
        let mut request = request_with_tool();
        if let Some(parallel) = parallel {
            request["parallel_tool_calls"] = json!(parallel);
        }
        let translated: Value = serde_json::from_slice(
            &translate_response(&serde_json::to_vec(&native).unwrap(), &request).unwrap(),
        )
        .unwrap();
        let calls = translated["output"].as_array().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["call_id"], "call_inspections");
        assert_eq!(calls[0]["name"], "exec_command");
        assert_eq!(
            serde_json::from_str::<Value>(calls[0]["arguments"].as_str().unwrap()).unwrap(),
            arguments
        );
        let result = json!({
            "type":"function_call_output", "call_id":"call_inspections",
            "output":"Synthetic location output\nSynthetic directory listing"
        });
        request["input"] = json!([
            {"role":"user","content":"Inspect the repo"}, calls[0], result,
            {"role":"user","content":"Continue"}
        ]);
        let prepared = prepare_request(&request).unwrap();
        let replayed = prepared["input"].as_array().unwrap();
        let call = replayed
            .iter()
            .find(|item| item["type"] == "function_call")
            .unwrap();
        assert_eq!(call["call_id"], "call_inspections");
        let envelope: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(envelope["references"], json!(["exec_command"]));
        assert_eq!(envelope["code"], code);
        let output = replayed
            .iter()
            .find(|item| item["type"] == "function_call_output")
            .unwrap();
        assert_eq!(output["call_id"], result["call_id"]);
        assert_eq!(output["output"], result["output"]);
    }
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
    let encoded = instructions
        .split_once("Example outer arguments for exec_command (function): ")
        .unwrap()
        .1;
    let example = serde_json::Deserializer::from_str(encoded)
        .into_iter::<Value>()
        .next()
        .unwrap()
        .unwrap();
    assert!(example["code"]
        .as_str()
        .unwrap()
        .starts_with(super::super::FUNCTION_STRING_PREFIX));
    let arguments = super::super::codec::function_arguments(&example["code"]).unwrap();
    assert_eq!(arguments["cmd"], "echo \"hello\"\necho \"world\"");
    assert_eq!(example["references"], json!(["exec_command"]));
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
fn large_catalog_keeps_every_schema_without_repeating_shared_instructions() {
    let schema = json!({
        "type":"object",
        "properties":{"mode":{"type":"string","enum":["read","write"]}},
        "required":["mode"],"additionalProperties":false
    });
    let tools: Vec<_> = (0..64)
        .map(|index| {
            json!({
                "type":"function", "name":format!("fixture_{index}"),
                "description":"Preserve this synthetic description.",
                "parameters":schema
            })
        })
        .collect();
    let request = json!({"input":"hello", "tools":tools});
    let original = request.clone();
    let prepared = prepare_request(&request).unwrap();
    let instructions = prepared["input"][0]["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        instructions
            .matches("pass one JSON object matching it")
            .count(),
        1
    );
    assert_eq!(instructions.matches(". Parameters: ").count(), 64);
    assert_eq!(
        instructions
            .matches("Preserve this synthetic description.")
            .count(),
        64
    );
    assert_eq!(
        instructions
            .matches(&super::super::schema::prompt_schema(&schema))
            .count(),
        64
    );
    assert!(!instructions.contains("Arguments must be one JSON object matching this JSON Schema"));
    for index in 0..64 {
        assert!(instructions.contains(&format!("- fixture_{index} (function)")));
    }
    assert_eq!(request, original);
}

#[test]
fn large_historical_catalog_keeps_current_definitions_without_duplicates() {
    let historical: Vec<_> = (0..200).map(|index| json!({
        "type":"function", "name":format!("fixture_{index}"),
        "description":format!("Historical definition {index}."), "parameters":{"type":"object"}
    })).collect();
    let request = json!({
        "input":[{"type":"additional_tools","tools":historical},{"role":"user","content":"hello"}],
        "tools":[
            {"type":"function","name":"fixture_37","description":"Current definition wins.","parameters":{"type":"object"}},
            {"type":"function","name":"new_fixture","parameters":{"type":"object"}}
        ]
    });
    let prepared = prepare_request(&request).unwrap();
    let instructions = prepared["input"][0]["content"][0]["text"].as_str().unwrap();
    assert_eq!(instructions.matches(". Parameters: ").count(), 201);
    assert_eq!(instructions.matches("- fixture_37 (function)").count(), 1);
    assert!(instructions.contains("Current definition wins."));
    assert!(!instructions.contains("Historical definition 37."));
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
    assert!(prepared["metadata"].get("request_kind").is_none());
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
    assert!(first["metadata"].get("request_kind").is_none());
    assert!(first["metadata"].get("nested").is_none());
}

#[test]
fn borrowed_turn_prefix_keeps_the_existing_metadata_fingerprint() {
    use sha2::Digest;
    let request = json!({
        "input":[
            {"role":"developer","content":"Synthetic policy. ".repeat(2000)},
            {"role":"user","content":"Synthetic question."},
            {"role":"assistant","content":"Synthetic response."},
            {"role":"user","content":"Synthetic next question."},
            {"type":"function_call_output","call_id":"synthetic_call","output":"Synthetic large result. ".repeat(5000)}
        ]
    });
    let input = request["input"].as_array().unwrap();
    let conversation = codec::short_hash(&input[0]);
    let previous_prefix_hash = hex::encode(sha2::Sha256::digest(
        serde_json::to_vec(&Value::Array(input[..4].to_vec())).unwrap(),
    ));
    let metadata = codec::basis_points_metadata(request.as_object().unwrap(), input);
    assert_eq!(
        metadata["task_id"],
        codec::stable_uuid("cpa-oai-basispoints", &conversation)
    );
    assert_eq!(
        metadata["turn_id"],
        codec::stable_uuid(
            "cpa-oai-basispoints/turn",
            &format!("{conversation}/{previous_prefix_hash}")
        )
    );
    assert_eq!(metadata["agent_iteration"], "2");
}

#[test]
fn preparation_keeps_client_metadata_out_of_the_strict_transport_contract() {
    let mut request = request_with_tool();
    request["metadata"] = json!({
        "user_id":"synthetic-user",
        "request_kind":"synthetic",
        "task_id":"client-task",
        "turn_id":"client-turn",
        "agent_iteration":"999",
        "bps_tools_version_id":"client-version",
        "bps_prompt_base_override":"client-override"
    });
    let before = request.clone();
    let prepared = prepare_request(&request).unwrap();
    let mut without_metadata = request.clone();
    without_metadata.as_object_mut().unwrap().remove("metadata");
    let baseline = prepare_request(&without_metadata).unwrap();
    assert_eq!(prepared["metadata"], baseline["metadata"]);
    assert_eq!(prepared["metadata"].as_object().unwrap().len(), 3);
    assert_eq!(request, before);
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
