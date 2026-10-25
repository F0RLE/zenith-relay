use super::*;
use crate::gateway::execution::basis_points::presentation::OPTION;
use crate::gateway::execution::basis_points::{prepare_request, translate_response};

mod identity;

fn catalog_text(prepared: &Value) -> &str {
    prepared["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item.pointer("/content/0/text").and_then(Value::as_str))
        .find(|text| text.contains("Current client tool catalog"))
        .expect("current catalog follows history")
}

fn request() -> Value {
    json!({"model":"synthetic-model","input":"hello","stream":true,"relay_bps_tool_catalog":"discovery","tools":[
        {"type":"namespace","name":"files","tools":[
            {"type":"function","name":"read","description":"Read a file","parameters":{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"],"additionalProperties":false}},
            {"type":"function","name":"write","description":"Write a file","parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}}
        ]},
        {"type":"custom","name":"patch","format":{"type":"grammar","syntax":"lark","definition":"start: \"synthetic\""}}
    ]})
}

fn response(name: &str, arguments: Value) -> Value {
    json!({"id":"resp_synthetic","object":"response","status":"completed","output":[{
        "id":"fc_synthetic","type":"function_call","call_id":"call_synthetic","name":"run_officejs","status":"completed",
        "arguments":json!({"references":[name],"code":arguments.to_string()}).to_string()
    }],"usage":{"input_tokens":100,"output_tokens":20,"total_tokens":120}})
}

#[test]
fn batch_parent_cannot_be_replayed_as_a_catalog_lookup() {
    let mut request = request();
    request["input"] = json!([{"type":"function_call_output","call_id":"call_rb1.2.0.Y2FsbF9zeW50aGV0aWM","output":"synthetic"}]);
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let lookup = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
    assert_eq!(
        session
            .advance(&lookup, &mut json!({}))
            .unwrap_err()
            .parameter(),
        Some("output.relay.catalog.call_id")
    );
}

#[test]
fn noncompleted_native_items_never_advance_catalog() {
    for name in [LOOKUP_TOOL, "files.read"] {
        for status in ["in_progress", "incomplete"] {
            let request = request();
            let mut upstream = prepare_request(&request).unwrap();
            let before = upstream.clone();
            let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
            let loaded = session.loaded.clone();
            let mut reply = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
            reply["output"][0]["call_id"] = json!("call_ready");
            let mut unfinished = response(
                name,
                if name == LOOKUP_TOOL {
                    json!({"names":["files.write"]})
                } else {
                    json!({"path":"synthetic"})
                },
            );
            unfinished["output"][0]["status"] = json!(status);
            reply["output"]
                .as_array_mut()
                .unwrap()
                .push(unfinished["output"][0].clone());
            assert_eq!(
                session
                    .advance(&reply, &mut upstream)
                    .unwrap_err()
                    .parameter(),
                Some("output.run_officejs.status")
            );
            assert_eq!(upstream, before);
            assert_eq!(session.loaded, loaded);
            assert!(session.call_ids.is_empty());
            assert!(session.lookup_contracts.is_empty());
            assert_eq!(session.rounds, 0);
        }
    }
}

#[test]
fn index_omits_schemas_and_preserves_all_identities_then_loads_original_contract() {
    let request = request();
    let mut upstream = prepare_request(&request).unwrap();
    assert_eq!(upstream["stream"], true);
    let mut buffered = request.clone();
    buffered["stream"] = json!(false);
    assert_eq!(prepare_request(&buffered).unwrap()["stream"], false);
    assert!(upstream.get(OPTION).is_none());
    let text = catalog_text(&upstream);
    let index: Value = serde_json::from_str(text.split_once("Tool index:\n").unwrap().1).unwrap();
    assert_eq!(index["function"]["files"], json!(["read", "write"]));
    assert_eq!(index["custom"][""], json!(["patch"]));
    assert!(!text.contains("Read a file"));
    assert!(!text.contains("Argument names:"));
    assert!(!text.contains("\"minLength\":1"));
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let lookup = response(LOOKUP_TOOL, json!({"names":["files.read","patch"]}));
    assert!(session.advance(&lookup, &mut upstream).unwrap());
    let result = upstream["input"].as_array().unwrap().last().unwrap();
    let definitions: Value = serde_json::from_str(result["output"].as_str().unwrap()).unwrap();
    assert_eq!(
        definitions["tools"][0]["parameters"],
        request["tools"][0]["tools"][0]["parameters"]
    );
    assert_eq!(
        definitions["tools"][0]["description"],
        request["tools"][0]["tools"][0]["description"]
    );
    assert_eq!(
        definitions["tools"][1]["format"],
        request["tools"][1]["format"]
    );
    assert_eq!(upstream["metadata"]["agent_iteration"], "2");
    let mut call = response("files.read", json!({"path":"synthetic.txt"}));
    call["output"][0]["call_id"] = json!("call_client");
    assert!(!session.advance(&call, &mut upstream).unwrap());
    let translated: Value = serde_json::from_slice(
        &translate_response(&serde_json::to_vec(&call).unwrap(), &request).unwrap(),
    )
    .unwrap();
    assert_eq!(translated["output"][0]["name"], "read");
    assert_eq!(translated["output"][0]["namespace"], "files");
    let invalid = response("files.read", json!({"path":""}));
    assert!(translate_response(&serde_json::to_vec(&invalid).unwrap(), &request).is_err());
}

#[test]
fn ordinary_text_never_starts_another_generation() {
    let request = request();
    let mut upstream = prepare_request(&request).unwrap();
    let before = upstream.clone();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert!(!session.advance(&json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"hello"}]}]}), &mut upstream).unwrap());
    assert_eq!(upstream, before);
    assert_eq!(session.rounds, 0);
}

#[test]
fn working_tools_are_ready_without_loading_the_entire_catalog() {
    let mut request = request();
    request["tools"].as_array_mut().unwrap().extend([
        json!({"type":"function","name":"exec_command","description":"Run a command","parameters":{"type":"object","properties":{"cmd":{"type":"string"}},"required":["cmd"]}}),
        json!({"type":"custom","name":"apply_patch","description":"Apply a patch"}),
        json!({"type":"function","name":"write_stdin","parameters":{"type":"object"}}),
    ]);
    let session = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert_eq!(
        session.loaded,
        BTreeSet::from([
            "exec_command".into(),
            "apply_patch".into(),
            "write_stdin".into()
        ])
    );
    let upstream = prepare_request(&request).unwrap();
    let contracts = upstream["input"].as_array().unwrap().last().unwrap()["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(contracts.contains("no catalog lookup needed"));
    assert!(contracts.contains("Run a command"));
    assert!(!contracts.contains("Read a file"));
    request["tool_choice"] = json!({"type":"function","name":"files.read"});
    let selected = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert!(!selected.loaded.contains("exec_command"));
    assert_eq!(selected.loaded, BTreeSet::from(["files.read".to_string()]));
    let error = translate_response(
        &serde_json::to_vec(&response("exec_command", json!({"cmd":"echo synthetic"}))).unwrap(),
        &request,
    )
    .unwrap_err();
    assert_eq!(error.parameter(), Some("output.run_officejs.tool_choice"));
}

#[test]
fn eager_catalog_has_byte_and_count_limits_and_respects_explicit_deferral() {
    let mut request = request();
    request["tools"] = json!([
        {"type":"function","name":"exec_command","defer_loading":true,"parameters":{"type":"object"}},
        {"type":"function","name":"apply_patch","description":"x".repeat(17 * 1024),"parameters":{"type":"object"}}
    ]);
    request["tools"].as_array_mut().unwrap().extend((0..10).map(|n| json!({
        "type":"function","name":format!("eager_{n}"),"defer_loading":false,"parameters":{"type":"object"}
    })));
    let session = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert_eq!(session.loaded.len(), 8);
    assert!(!session.loaded.contains("exec_command"));
    assert!(!session.loaded.contains("apply_patch"));
}

#[test]
fn remembered_names_use_current_schemas_and_never_restore_removed_or_disallowed_tools() {
    let mut request = request();
    request["tools"][0]["tools"][1]["parameters"]["properties"]["text"]["minLength"] = json!(7);
    let names = BTreeSet::from(["files.write".to_string(), "removed.tool".to_string()]);
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let mut upstream = prepare_request(&request).unwrap();
    session.preload(&names, &mut upstream).unwrap();
    assert_eq!(session.loaded, BTreeSet::from(["files.write".to_string()]));
    let contracts = upstream["input"].as_array().unwrap().last().unwrap()["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(contracts.contains("\"minLength\":7"));
    assert!(!contracts.contains("removed.tool"));
    assert!(translate_response(
        &serde_json::to_vec(&response("files.write", json!({"text":"short"}))).unwrap(),
        &request
    )
    .is_err());
    request["tool_choice"] = json!({"type":"function","name":"files.read"});
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let mut upstream = prepare_request(&request).unwrap();
    let before = upstream.clone();
    session.preload(&names, &mut upstream).unwrap();
    assert_eq!(session.loaded, BTreeSet::from(["files.read".to_string()]));
    assert_eq!(upstream, before);
}

#[test]
fn remembered_contracts_do_not_expand_the_prompt_without_a_bound() {
    let mut request = request();
    request["tools"][0]["tools"][1]["description"] = json!("x".repeat(17 * 1024));
    let names = BTreeSet::from(["files.read".to_string(), "files.write".to_string()]);
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let mut upstream = prepare_request(&request).unwrap();
    session.preload(&names, &mut upstream).unwrap();
    assert_eq!(session.loaded, BTreeSet::from(["files.read".to_string()]));
    let loaded = upstream["input"].as_array().unwrap().last().unwrap();
    assert_eq!(loaded["type"], "message");
    assert_eq!(loaded["role"], "developer");
    assert!(loaded["content"][0]["text"].as_str().unwrap().len() < 16 * 1024);
}

#[test]
fn catalog_round_limit_bounds_cached_and_new_queries() {
    let mut session = DiscoverySession::new(&request(), true).unwrap().unwrap();
    let mut upstream = prepare_request(&request()).unwrap();
    for round in 0..MAX_LOOKUP_ROUNDS {
        let mut lookup = response(
            LOOKUP_TOOL,
            json!({"query":format!("patch{round}"),"limit":1}),
        );
        lookup["output"][0]["call_id"] = json!(format!("lookup_{round}"));
        assert!(session.advance(&lookup, &mut upstream).unwrap());
    }
    let mut lookup = response(LOOKUP_TOOL, json!({"query":"another query"}));
    lookup["output"][0]["call_id"] = json!("lookup_final");
    assert_eq!(
        session
            .advance(&lookup, &mut upstream)
            .unwrap_err()
            .parameter(),
        Some("output.relay.catalog.limit")
    );

    let mut session = DiscoverySession::new(&request(), true).unwrap().unwrap();
    let mut lookup = response(LOOKUP_TOOL, json!({"names":["patch"]}));
    session.advance(&lookup, &mut upstream).unwrap();
    lookup["output"][0]["call_id"] = json!("lookup_new_identity");
    assert!(session.advance(&lookup, &mut upstream).unwrap());
    let output = upstream["input"].as_array().unwrap().last().unwrap();
    let result: Value = serde_json::from_str(output["output"].as_str().unwrap()).unwrap();
    assert_eq!(result["cached"], true);
    assert_eq!(result["tools"], json!([]));
    assert_eq!(
        result["reused"],
        json!([{"name":"patch","call_id":"call_synthetic"}])
    );
}

#[test]
fn invalid_local_lookups_return_bounded_tool_errors_without_executing_client_tools() {
    for args in [
        json!({"names":["unknown"]}),
        json!({"names":[]}),
        json!({"query":"read","limit":1.5}),
        json!({"query":"read","limit":9}),
        json!({"names":["patch"],"query":"patch"}),
        json!({"query":"read","extra":true}),
    ] {
        let mut session = DiscoverySession::new(&request(), true).unwrap().unwrap();
        let mut upstream = prepare_request(&request()).unwrap();
        assert!(session
            .advance(&response(LOOKUP_TOOL, args), &mut upstream)
            .unwrap());
        let result = upstream["input"].as_array().unwrap().last().unwrap();
        let output: Value = serde_json::from_str(result["output"].as_str().unwrap()).unwrap();
        assert_eq!(output["error"], "invalid_catalog_lookup");
        assert_eq!(output["remainingLookups"], MAX_LOOKUP_ROUNDS - 1);
        assert!(session.loaded.is_empty());
    }
}

#[test]
fn overlapping_lookups_reuse_each_contract_at_its_original_call_id() {
    let request = request();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let mut upstream = prepare_request(&request).unwrap();
    for (call_id, names, new_count, reused) in [
        ("lookup_first", json!(["files.read"]), 1, json!([])),
        (
            "lookup_second",
            json!(["files.read", "files.write", "patch"]),
            2,
            json!([{"name":"files.read","call_id":"lookup_first"}]),
        ),
        (
            "lookup_third",
            json!(["patch", "files.read", "files.write"]),
            0,
            json!([
                {"name":"patch","call_id":"lookup_second"},
                {"name":"files.read","call_id":"lookup_first"},
                {"name":"files.write","call_id":"lookup_second"}
            ]),
        ),
    ] {
        let mut lookup = response(LOOKUP_TOOL, json!({"names":names}));
        lookup["output"][0]["call_id"] = json!(call_id);
        assert!(session.advance(&lookup, &mut upstream).unwrap());
        let output = upstream["input"].as_array().unwrap().last().unwrap();
        assert_eq!(output["call_id"], call_id);
        let result: Value = serde_json::from_str(output["output"].as_str().unwrap()).unwrap();
        assert_eq!(result["tools"].as_array().unwrap().len(), new_count);
        assert_eq!(result["reused"], reused);
        assert_eq!(result["cached"], new_count == 0);
    }
    assert_eq!(upstream.to_string().matches("Read a file").count(), 1);
    assert_eq!(upstream.to_string().matches("Write a file").count(), 1);
    assert_eq!(session.loaded.len(), 3);
}

#[test]
fn reused_lookup_call_ids_are_rejected() {
    let mut session = DiscoverySession::new(&request(), true).unwrap().unwrap();
    let mut upstream = prepare_request(&request()).unwrap();
    let lookup = response(LOOKUP_TOOL, json!({"names":["patch"]}));
    assert!(session.advance(&lookup, &mut upstream).unwrap());
    assert!(session.advance(&lookup, &mut upstream).is_err());
}

#[test]
fn lookup_call_ids_cannot_reuse_single_item_history() {
    let mut request = request();
    let lookup = response(LOOKUP_TOOL, json!({"names":["patch"]}));
    request["input"] = json!({
        "type":"function_call_output", "call_id":lookup["output"][0]["call_id"],
        "output":"synthetic prior result"
    });
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let mut upstream = prepare_request(&request).unwrap();
    let error = session.advance(&lookup, &mut upstream).unwrap_err();
    assert_eq!(error.parameter(), Some("output.relay.catalog.call_id"));
}

#[test]
fn legacy_local_lookup_objects_use_the_same_validated_catalog_contract() {
    let request = request();
    let mut upstream = prepare_request(&request).unwrap();
    let mut lookup = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
    lookup["output"][0]["arguments"] = json!({
        "code":json!({"tool":LOOKUP_TOOL,"args":{"names":["files.read"]}}).to_string()
    })
    .to_string()
    .into();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert!(session.advance(&lookup, &mut upstream).unwrap());
    let output: Value = serde_json::from_str(
        upstream["input"].as_array().unwrap().last().unwrap()["output"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(output["tools"][0]["name"], "files.read");
}

#[test]
fn discovery_budget_excludes_admitted_history_but_bounds_added_state() {
    let mut request = request();
    request["input"] = json!("x".repeat(MAX_CONTINUATION_BYTES));
    let mut upstream = prepare_request(&request).unwrap();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert!(session
        .advance(
            &response(LOOKUP_TOOL, json!({"names":["files.read"]})),
            &mut upstream
        )
        .unwrap());

    let mut lookup = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
    lookup["output"][0]["call_id"] = json!("lookup_too_large");
    lookup["output"].as_array_mut().unwrap().insert(0, json!({
        "type":"message","role":"assistant","content":[{"type":"output_text","text":"x".repeat(MAX_CONTINUATION_BYTES)}]
    }));
    let before = upstream.clone();
    assert_eq!(
        session
            .advance(&lookup, &mut upstream)
            .unwrap_err()
            .parameter(),
        Some("output.relay.catalog.bytes")
    );
    assert_eq!(upstream, before);
}

#[test]
fn incomplete_tool_fragments_preserve_text_and_usage_without_continuing() {
    let request = request();
    let mut upstream = prepare_request(&request).unwrap();
    let before = upstream.clone();
    let mut incomplete = response("files.read", json!({"path":"synthetic.txt"}));
    incomplete["status"] = json!("incomplete");
    incomplete["incomplete_details"] = json!({"reason":"max_output_tokens"});
    incomplete["output"][0]
        .as_object_mut()
        .unwrap()
        .remove("call_id");
    incomplete["output"][0]["arguments"] = json!("{");
    incomplete["output"].as_array_mut().unwrap().insert(0, json!({
        "type":"message","role":"assistant","content":[{"type":"output_text","text":"Partial answer"}]
    }));
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert!(!session.advance(&incomplete, &mut upstream).unwrap());
    assert_eq!(upstream, before);
    session.finish(&mut incomplete);
    let translated: Value = serde_json::from_slice(
        &translate_response(&serde_json::to_vec(&incomplete).unwrap(), &request).unwrap(),
    )
    .unwrap();
    assert_eq!(translated["status"], "incomplete");
    assert_eq!(
        translated["incomplete_details"]["reason"],
        "max_output_tokens"
    );
    assert_eq!(translated["usage"]["input_tokens"], 100);
    assert_eq!(translated["output"].as_array().unwrap().len(), 1);
    assert_eq!(
        translated["output"][0]["content"][0]["text"],
        "Partial answer"
    );
}

#[test]
fn client_calls_cannot_reuse_a_history_or_local_lookup_identity() {
    let mut request = request();
    request["input"] = json!([
        {"type":"function_call","call_id":"call_history","name":"read","namespace":"files","arguments":"{\"path\":\"synthetic\"}"},
        {"type":"function_call_output","call_id":"call_history","output":"synthetic"}
    ]);
    let mut upstream = prepare_request(&request).unwrap();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert!(session
        .advance(
            &response(LOOKUP_TOOL, json!({"names":["files.read"]})),
            &mut upstream
        )
        .unwrap());
    for reused in ["call_history", "call_synthetic", " \t "] {
        let mut client = response("files.read", json!({"path":"synthetic"}));
        client["output"][0]["call_id"] = json!(reused);
        assert_eq!(
            session
                .advance(&client, &mut upstream)
                .unwrap_err()
                .parameter(),
            Some("output.relay.catalog.call_id")
        );
    }
    let mut fresh = response("files.read", json!({"path":"synthetic"}));
    fresh["output"][0]["call_id"] = json!("call_fresh");
    assert!(!session.advance(&fresh, &mut upstream).unwrap());
}

#[test]
fn mixed_catalog_and_client_calls_finish_without_another_generation() {
    let mut request = request();
    request["parallel_tool_calls"] = json!(false);
    let mut upstream = prepare_request(&request).unwrap();
    let before = upstream.clone();
    let mut mixed = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
    let mut client = response("files.read", json!({"path":"synthetic.txt"}))["output"][0].clone();
    client["call_id"] = json!("call_client");
    client["id"] = json!("fc_client");
    mixed["output"].as_array_mut().unwrap().push(client);
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let mut diagnostic = crate::gateway::request::tool_use_diagnostics(&request, true);
    capture(&request, &mut diagnostic);
    let initial = diagnostic.basis_points_catalog.clone().unwrap();
    assert!(!session.advance(&mixed, &mut upstream).unwrap());
    assert_eq!(upstream, before);
    session.record(&mut diagnostic);
    let recorded = diagnostic.basis_points_catalog.unwrap();
    assert_eq!(
        recorded.rendered_schema_bytes,
        initial.rendered_schema_bytes
    );
    assert_eq!(recorded.loaded_tool_count, initial.loaded_tool_count);
    session.finish(&mut mixed);
    let translated: Value = serde_json::from_slice(
        &translate_response(&serde_json::to_vec(&mixed).unwrap(), &request).unwrap(),
    )
    .unwrap();
    assert_eq!(translated["output"].as_array().unwrap().len(), 1);
    assert_eq!(translated["output"][0]["call_id"], "call_client");
    assert_eq!(translated["output"][0]["name"], "read");
    assert_eq!(translated["output"][0]["namespace"], "files");
}

#[test]
fn opaque_reasoning_and_text_replay_unchanged_inside_the_owned_turn() {
    let request = request();
    let mut upstream = prepare_request(&request).unwrap();
    let mut lookup = response(LOOKUP_TOOL, json!({"query":"read file","limit":1}));
    let reasoning = json!({"type":"reasoning","id":"rs_synthetic","encrypted_content":"synthetic-opaque","summary":[]});
    let search = json!({"type":"web_search_call","id":"ws_synthetic","status":"completed"});
    let text = json!({"type":"message","id":"msg_synthetic","role":"assistant","content":[{"type":"output_text","text":"Inspecting the tool contract."}]});
    lookup["output"]
        .as_array_mut()
        .unwrap()
        .insert(0, reasoning.clone());
    lookup["output"]
        .as_array_mut()
        .unwrap()
        .insert(1, text.clone());
    lookup["output"]
        .as_array_mut()
        .unwrap()
        .insert(2, search.clone());
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    session.advance(&lookup, &mut upstream).unwrap();
    assert!(upstream["input"].as_array().unwrap().contains(&reasoning));
    let mut final_response = json!({"output":[]});
    session.finish(&mut final_response);
    assert_eq!(final_response["output"], json!([reasoning, text, search]));
    assert!(final_response["output"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["type"] != "function_call"));
}

#[test]
fn choices_scope_lookup_and_small_catalogs_stay_compact() {
    let mut request = request();
    request["tool_choice"] = json!({"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"files.read"}]});
    let session = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert!(lookup(&session.tools, &json!({"names":["files.write"]})).is_err());
    request["tool_choice"] = json!("none");
    assert!(DiscoverySession::new(&request, true).unwrap().is_none());
    request.as_object_mut().unwrap().remove("tool_choice");
    request.as_object_mut().unwrap().remove(OPTION);
    assert!(DiscoverySession::new(&request, true).unwrap().is_none());
    assert_eq!(prepare_request(&request).unwrap()["stream"], true);
    for mode in [json!(true), json!("unsupported"), json!(null)] {
        request[OPTION] = mode;
        assert_eq!(
            prepare_request(&request).unwrap_err().parameter(),
            Some(OPTION)
        );
    }
}

#[test]
fn automatic_discovery_uses_the_full_catalog_but_retains_turn_local_authority() {
    let mut request = request();
    request.as_object_mut().unwrap().remove(OPTION);
    request["tools"]
        .as_array_mut()
        .unwrap()
        .extend((0..32).map(|index| {
            json!({
                "type":"function","name":format!("synthetic_{index}"),
                "description":"Synthetic catalog description. ".repeat(64),
                "parameters":{"type":"object"}
            })
        }));
    let baseline = prepare_request(&request).unwrap();
    request["tool_choice"] = json!({"type":"function","name":"files.read"});
    let session = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert_eq!(session.tools.len(), 1);
    assert_eq!(session.loaded, BTreeSet::from(["files.read".to_string()]));
    assert!(lookup(&session.tools, &json!({"names":["files.write"]})).is_err());
    let prepared = prepare_request(&request).unwrap();
    assert_eq!(prepared["input"][0], baseline["input"][0]);
    assert!(catalog_text(&prepared).contains("Tool index:"));
    let mut diagnostics = ToolUseDiagnostics {
        client_context: Some(Default::default()),
        ..Default::default()
    };
    capture(&request, &mut diagnostics);
    let catalog = diagnostics.basis_points_catalog.unwrap();
    assert_eq!(catalog.mode, "discovery");
    assert_eq!(catalog.loaded_tool_count, 1);
    request["tool_choice"] = json!("none");
    assert!(DiscoverySession::new(&request, true).unwrap().is_none());
    assert_eq!(
        prepare_request(&request).unwrap()["input"][0],
        baseline["input"][0]
    );
}

#[test]
fn explicitly_selected_preloads_keep_existing_deferral_and_size_limits() {
    let mut request = request();
    request["tool_choice"] = json!({"type":"function","name":"files.read"});
    request["tools"][0]["tools"][0]["defer_loading"] = json!(true);
    assert!(DiscoverySession::new(&request, true)
        .unwrap()
        .unwrap()
        .loaded
        .is_empty());
    request["tools"][0]["tools"][0]
        .as_object_mut()
        .unwrap()
        .remove("defer_loading");
    request["tools"][0]["tools"][0]["description"] = json!("x".repeat(17 * 1024));
    assert!(DiscoverySession::new(&request, true)
        .unwrap()
        .unwrap()
        .loaded
        .is_empty());
}

#[test]
fn synthetic_large_catalog_reduces_initial_prompt_without_removing_tools() {
    let tools = (0..291).map(|i| json!({"type":"function","name":format!("synthetic_{i}"),"description":"Read a synthetic resource.","parameters":{
        "type":"object","properties":(0..12).map(|field| (format!("field_{field}"),json!({"type":"string","description":"Synthetic field description for a resource.","maxLength":500}))).collect::<serde_json::Map<_,_>>(),"required":["field_0"],"additionalProperties":false
    }})).collect::<Vec<_>>();
    let mut request = json!({"model":"synthetic","input":"hello","tools":tools});
    let automatic = prepare_request(&request).unwrap().to_string().len();
    assert!(DiscoverySession::new(&request, true).unwrap().is_some());
    request[OPTION] = json!("compact");
    let compact = prepare_request(&request).unwrap().to_string().len();
    request[OPTION] = json!("full");
    let full = prepare_request(&request).unwrap().to_string().len();
    assert!(automatic < compact / 3);
    assert!(compact < full);
    eprintln!("synthetic tools=291 request_json_bytes full={full} compact={compact} discovery={automatic}; not token or live billing evidence");
}

#[test]
fn initial_index_size_does_not_grow_with_verbose_descriptions_or_argument_names() {
    let tools = (0..291)
        .map(|i| {
            json!({"type":"function","name":format!("synthetic_{i}"),"parameters":{
                "type":"object","properties":{},"additionalProperties":false
            }})
        })
        .collect::<Vec<_>>();
    let mut request = json!({"model":"synthetic","input":"hello","relay_bps_tool_catalog":"discovery","tools":tools});
    let original_tools = client_tools(&request).unwrap();
    let initial_index = super::super::presentation::describe_catalog(
        &original_tools.iter().collect::<Vec<_>>(),
        &request,
    );
    for tool in request["tools"].as_array_mut().unwrap() {
        tool["description"] =
            json!("Synthetic tool usage rules and a detailed example. ".repeat(32));
        tool["parameters"]["properties"] = json!({
            "long_synthetic_parameter_name":{
                "type":"string","description":"Synthetic parameter semantics. ".repeat(16),"minLength":1
            }
        });
    }
    let tools = client_tools(&request).unwrap();
    let verbose_index =
        super::super::presentation::describe_catalog(&tools.iter().collect::<Vec<_>>(), &request);
    assert_eq!(verbose_index, initial_index);
    let index: Value =
        serde_json::from_str(verbose_index.split_once("Tool index:\n").unwrap().1).unwrap();
    assert_eq!(index["function"][""].as_array().unwrap().len(), tools.len());
    for tool in &tools {
        assert!(index["function"][""]
            .as_array()
            .unwrap()
            .contains(&json!(tool.name)));
    }
    let indexed = prepare_request(&request).unwrap().to_string().len();
    request[OPTION] = json!("compact");
    let compact = prepare_request(&request).unwrap().to_string().len();
    assert!(indexed < compact / 10);
    eprintln!("synthetic verbose tools=291 request_json_bytes compact={compact} discovery={indexed}; not token or live billing evidence");
}

#[test]
fn lookup_searches_deferred_descriptions_in_the_original_catalog_language() {
    let mut request = request();
    request["tools"][0]["tools"][0]["name"] = json!("opaque_action");
    request["tools"][0]["tools"][0]["description"] =
        json!("Чтение синтетического документа. Сначала проверить права доступа.");
    let tools = client_tools(&request).unwrap();
    let found = lookup(&tools, &json!({"query":"чтение документа","limit":1})).unwrap();
    assert_eq!(found[0].key(), "files.opaque_action");
    assert_eq!(
        found[0].spec.as_ref(),
        request["tools"][0]["tools"][0].as_object().unwrap()
    );
    assert_eq!(
        lookup(&tools, &json!({"query":"synthetic","limit":1})).unwrap()[0].key(),
        "patch"
    );
}

#[test]
fn automatic_discovery_counts_descriptions_and_custom_formats_not_just_parameters() {
    for kind in ["function", "custom"] {
        let tools = (0..24)
            .map(|i| {
                let mut tool = json!({"type":kind,"name":format!("synthetic_{i}")});
                if kind == "function" {
                    tool["parameters"] = json!({"type":"object","properties":{},"additionalProperties":false});
                    tool["description"] = json!("Synthetic usage guidance. ".repeat(64));
                } else {
                    tool["format"] = json!({"type":"grammar","syntax":"lark","definition":format!("start: \"{}\"", "synthetic".repeat(128))});
                }
                tool
            })
            .collect::<Vec<_>>();
        let mut request = json!({"model":"synthetic","input":"hello","tools":tools});
        assert!(DiscoverySession::new(&request, true).unwrap().is_some());
        request["tools"].as_array_mut().unwrap().pop();
        assert!(DiscoverySession::new(&request, true).unwrap().is_none());
    }
}

#[test]
fn grouped_index_preserves_nested_namespaces_custom_kinds_and_flat_dotted_names() {
    let mut request = request();
    request["tools"][0]["name"] = json!("workspace.files");
    request["tools"].as_array_mut().unwrap().push(json!({
        "type":"function","name":"flat.dotted","parameters":{"type":"object"}
    }));
    let tools = client_tools(&request).unwrap();
    let prepared = prepare_request(&request).unwrap();
    let text = catalog_text(&prepared);
    let index: Value = serde_json::from_str(text.split_once("Tool index:\n").unwrap().1).unwrap();
    let mut indexed = BTreeSet::new();
    for (kind, namespaces) in index.as_object().unwrap() {
        for (namespace, names) in namespaces.as_object().unwrap() {
            for name in names.as_array().unwrap() {
                let name = name.as_str().unwrap();
                let key = if namespace.is_empty() {
                    name.to_string()
                } else {
                    format!("{namespace}.{name}")
                };
                indexed.insert((key, kind.clone()));
            }
        }
    }
    assert_eq!(
        indexed,
        tools
            .iter()
            .map(|tool| (tool.key(), tool.kind.clone()))
            .collect()
    );
    request["tools"].as_array_mut().unwrap().reverse();
    request["tools"][2]["tools"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let reordered = prepare_request(&request).unwrap();
    assert_eq!(prepared["input"][0], reordered["input"][0]);
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let mut upstream = reordered;
    assert!(session
        .advance(
            &response(
                LOOKUP_TOOL,
                json!({"names":["workspace.files.read","flat.dotted","patch"]})
            ),
            &mut upstream
        )
        .unwrap());
    let output: Value = serde_json::from_str(
        upstream["input"].as_array().unwrap().last().unwrap()["output"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(output["tools"][0]["name"], "workspace.files.read");
    assert_eq!(output["tools"][1]["name"], "flat.dotted");
    assert_eq!(output["tools"][2]["type"], "custom");
}

#[test]
fn known_contracts_use_lossless_compact_schemas_after_unchanged_history_and_prefix() {
    let mut request = request();
    request["instructions"] = json!("Synthetic client instructions must remain verbatim.");
    let cold = prepare_request(&request).unwrap();
    request["input"] = json!([
        {"role":"developer","content":"Synthetic client instructions must remain verbatim."},
        {"role":"user","content":"Read a synthetic document."},
        {"type":"function_call","name":"read","namespace":"files","call_id":"known_read","arguments":"{\"path\":\"synthetic.txt\"}"},
        {"type":"function_call_output","call_id":"known_read","output":"Synthetic tool result remains unchanged."},
        {"role":"user","content":"Continue."}
    ]);
    let before = request.clone();
    let prepared = prepare_request(&request).unwrap();
    let input = prepared["input"].as_array().unwrap();
    assert_eq!(input[0], cold["input"][0]);
    assert_eq!(input[1], cold["input"][1]);
    let known = input.last().unwrap()["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(known.contains("Parameter notation:"));
    assert!(known.contains("files.read (function): Read a file"));
    assert!(known.contains("Parameters: object"));
    assert!(known.contains("\"additionalProperties\":false"));
    assert!(known.contains("\"minLength\":1"));
    assert!(!known.contains("Write a file"));
    assert!(!known.contains("patch (custom)"));
    assert!(input
        .iter()
        .any(|item| item["output"] == "Synthetic tool result remains unchanged."));
    assert_eq!(input[2]["content"][0]["text"], request["instructions"]);
    assert_eq!(request, before);
    assert!(translate_response(
        &serde_json::to_vec(&response("files.read", json!({"path":""}))).unwrap(),
        &request
    )
    .is_err());
}

#[test]
fn rendered_schema_diagnostics_count_compact_preloads_and_unique_full_lookups() {
    let mut request = request();
    request["input"] = json!([
        {"type":"function_call","name":"read","namespace":"files","call_id":"known_read","arguments":"{\"path\":\"synthetic.txt\"}"},
        {"type":"function_call_output","call_id":"known_read","output":"synthetic"}
    ]);
    let schema = &request["tools"][0]["tools"][0]["parameters"];
    let compact_bytes = super::super::schema::prompt_schema(schema).len() as u64;
    let full_bytes = schema.to_string().len() as u64;
    let mut diagnostic = crate::gateway::request::tool_use_diagnostics(&request, true);
    capture(&request, &mut diagnostic);
    assert_eq!(
        diagnostic
            .basis_points_catalog
            .as_ref()
            .unwrap()
            .rendered_schema_bytes,
        compact_bytes
    );
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let mut upstream = prepare_request(&request).unwrap();
    for (round, arguments) in [
        json!({"names":["files.read"]}),
        json!({"query":"Read a file","limit":1}),
    ]
    .into_iter()
    .enumerate()
    {
        let mut lookup = response(LOOKUP_TOOL, arguments);
        lookup["output"][0]["call_id"] = json!(format!("lookup_{round}"));
        assert!(session.advance(&lookup, &mut upstream).unwrap());
        session.record(&mut diagnostic);
        let catalog = diagnostic.basis_points_catalog.as_ref().unwrap();
        assert_eq!(catalog.rendered_schema_bytes, compact_bytes + full_bytes);
        assert_eq!(catalog.loaded_tool_count, 1);
        let result = upstream["input"].as_array().unwrap().last().unwrap();
        let result: Value = serde_json::from_str(result["output"].as_str().unwrap()).unwrap();
        assert_eq!(
            result["tools"].as_array().unwrap().len(),
            usize::from(round == 0)
        );
        if round != 0 {
            assert_eq!(
                result["reused"],
                json!([{"name":"files.read","call_id":"lookup_0"}])
            );
        }
    }
}

#[test]
fn namespace_grouping_reduces_a_synthetic_index_without_dropping_tools() {
    let tools = (0..10).map(|group| json!({
        "type":"namespace","name":format!("mcp__synthetic_apps__document_management_{group}"),"tools":(0..30).map(|i| json!({
            "type":"function","name":format!("action_{i}"),"parameters":{"type":"object"}
        })).collect::<Vec<_>>()
    })).collect::<Vec<_>>();
    let request = json!({"tools":tools,"relay_bps_tool_catalog":"discovery","input":"hello"});
    let tools = client_tools(&request).unwrap();
    let prior_flat_list_bytes = tools
        .iter()
        .map(|tool| format!("- {} ({})\n", tool.key(), tool.kind).len())
        .sum::<usize>();
    let grouped =
        super::super::presentation::describe_catalog(&tools.iter().collect::<Vec<_>>(), &request);
    assert!(grouped.len() < prior_flat_list_bytes / 2);
    let index: Value =
        serde_json::from_str(grouped.split_once("Tool index:\n").unwrap().1).unwrap();
    assert_eq!(
        index["function"]
            .as_object()
            .unwrap()
            .values()
            .map(|names| names.as_array().unwrap().len())
            .sum::<usize>(),
        300
    );
    eprintln!("synthetic 300-tool index bytes: prior_flat_list={prior_flat_list_bytes}, grouped_with_lookup_instructions={}; not live token evidence", grouped.len());
}
