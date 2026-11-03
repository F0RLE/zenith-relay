use super::*;

fn lookup_result(upstream: &Value) -> Value {
    serde_json::from_str(
        upstream["input"].as_array().unwrap().last().unwrap()["output"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn visible_contracts_are_reused_for_history_eager_and_remembered_preloads() {
    for mode in ["history", "eager", "remembered"] {
        let mut request = request();
        match mode {
            "history" => {
                request["input"] = json!([
                    {"type":"function_call","namespace":"files","name":"read","call_id":"past_read","arguments":"{\"path\":\"synthetic\"}"},
                    {"type":"function_call_output","call_id":"past_read","output":"Keep this result unchanged."}
                ]);
            }
            "eager" => request["tools"][0]["tools"][0]["defer_loading"] = json!(false),
            _ => {}
        }
        let original = request.clone();
        let mut upstream = prepare_request(&request).unwrap();
        let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
        if mode == "remembered" {
            session
                .preload(&BTreeSet::from(["files.read".into()]), &mut upstream)
                .unwrap();
        }
        let prefix = upstream["input"].as_array().unwrap().clone();
        assert!(upstream.to_string().contains("Read a file"));
        for round in 0..2 {
            let mut lookup = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
            lookup["output"][0]["call_id"] = json!(format!("lookup_{round}"));
            assert!(session.advance(&lookup, &mut upstream).unwrap());
            let result = lookup_result(&upstream);
            assert_eq!(result["tools"], json!([]), "{mode}");
            assert_eq!(
                result["reused"],
                json!([{"name":"files.read","source":"loaded_tool_contracts"}]),
                "{mode}"
            );
            assert_eq!(result["cached"], true);
        }
        assert_eq!(
            &upstream["input"].as_array().unwrap()[..prefix.len()],
            prefix
        );
        assert_eq!(upstream.to_string().matches("Read a file").count(), 1);
        assert_eq!(request, original);
        assert!(
            translate_response(
                &serde_json::to_vec(&response("files.read", json!({"path":""}))).unwrap(),
                &request
            )
            .is_err()
        );
        assert!(
            translate_response(
                &serde_json::to_vec(&response("files.read", json!({"path":"synthetic"}))).unwrap(),
                &request
            )
            .is_ok()
        );
    }
}

#[test]
fn mixed_lookups_keep_original_new_contracts_and_distinguish_reuse_sources() {
    let mut request = request();
    request["tools"][0]["tools"][0]["defer_loading"] = json!(false);
    let mut upstream = prepare_request(&request).unwrap();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let initial_bytes = session.rendered_schema_bytes.unwrap();
    let full_bytes = request["tools"][0]["tools"][1]["parameters"]
        .to_string()
        .len() as u64;
    for round in 0..2 {
        let mut lookup = response(LOOKUP_TOOL, json!({"names":["files.read","files.write"]}));
        lookup["output"][0]["call_id"] = json!(format!("lookup_{round}"));
        assert!(session.advance(&lookup, &mut upstream).unwrap());
        let result = lookup_result(&upstream);
        assert_eq!(
            session.rendered_schema_bytes,
            Some(initial_bytes + full_bytes)
        );
        assert_eq!(
            result["reused"][0],
            json!({"name":"files.read","source":"loaded_tool_contracts"})
        );
        if round == 0 {
            let mut expected = request["tools"][0]["tools"][1].clone();
            expected["name"] = json!("files.write");
            assert_eq!(result["tools"], json!([expected]));
            assert_eq!(result["cached"], false);
        } else {
            assert_eq!(result["tools"], json!([]));
            assert_eq!(
                result["reused"][1],
                json!({"name":"files.write","call_id":"lookup_0"})
            );
            assert_eq!(result["cached"], true);
        }
    }
}

#[test]
fn contracts_omitted_by_preload_budget_still_return_the_full_definition() {
    let mut request = request();
    request["tools"][0]["tools"][1]["description"] = json!("synthetic".repeat(2048));
    let mut upstream = prepare_request(&request).unwrap();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    session
        .preload(&BTreeSet::from(["files.write".into()]), &mut upstream)
        .unwrap();
    assert!(!session.loaded.contains("files.write"));
    assert!(
        session
            .advance(
                &response(LOOKUP_TOOL, json!({"names":["files.write"]})),
                &mut upstream
            )
            .unwrap()
    );
    let result = lookup_result(&upstream);
    let mut expected = request["tools"][0]["tools"][1].clone();
    expected["name"] = json!("files.write");
    assert_eq!(result["tools"], json!([expected]));
    assert_eq!(result["reused"], json!([]));
}

#[test]
fn reuse_reduces_serialized_continuation_without_pruning_client_context() {
    let mut request = request();
    let tool = &mut request["tools"][0]["tools"][0];
    tool["defer_loading"] = json!(false);
    for index in 0..32 {
        tool["parameters"]["properties"][format!("option_{index}")] = json!({
            "type":"string", "minLength":1, "maxLength":128,
            "description":"Synthetic optional setting. Preserve its description and constraints unchanged."
        });
    }
    let mut original_contract = tool.clone();
    original_contract["name"] = json!("files.read");
    let mut upstream = prepare_request(&request).unwrap();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert!(session.loaded.contains("files.read"));
    let lookup = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
    assert!(session.advance(&lookup, &mut upstream).unwrap());
    let after = serde_json::to_vec(&upstream).unwrap().len();
    let mut previous = upstream.clone();
    previous["input"].as_array_mut().unwrap().last_mut().unwrap()["output"] = json!(
        json!({"tools":[original_contract],"reused":[],"cached":false,"remainingLookups":MAX_LOOKUP_ROUNDS - 1}).to_string()
    );
    let before = serde_json::to_vec(&previous).unwrap().len();
    assert!(before - after > 4096);
    println!(
        "synthetic preloaded lookup continuation: before={before} after={after} saved={}",
        before - after
    );
}

#[test]
fn reused_custom_contract_keeps_its_format_and_never_grants_other_tool_permissions() {
    let mut request = request();
    request["tool_choice"] = json!({"type":"custom","name":"patch"});
    let mut upstream = prepare_request(&request).unwrap();
    assert!(upstream.to_string().contains("Input format:"));
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert_eq!(session.loaded, BTreeSet::from(["patch".into()]));
    let mut lookup = response(LOOKUP_TOOL, json!({"names":["patch"]}));
    assert!(session.advance(&lookup, &mut upstream).unwrap());
    assert_eq!(
        lookup_result(&upstream)["reused"],
        json!([{"name":"patch","source":"loaded_tool_contracts"}])
    );
    lookup["output"][0]["call_id"] = json!("disallowed_lookup");
    lookup["output"][0]["arguments"] = json!(
        json!({"references":[LOOKUP_TOOL],"code":json!({"names":["files.read"]}).to_string()})
            .to_string()
    );
    assert!(session.advance(&lookup, &mut upstream).unwrap());
    assert_eq!(lookup_result(&upstream)["error"], "invalid_catalog_lookup");
    assert_eq!(session.loaded, BTreeSet::from(["patch".into()]));
}

#[test]
fn remembered_contract_reuse_validates_against_the_current_schema() {
    let mut request = request();
    request["tools"][0]["tools"][0]["parameters"]["properties"]["path"]["minLength"] = json!(12);
    let mut upstream = prepare_request(&request).unwrap();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    session
        .preload(&BTreeSet::from(["files.read".into()]), &mut upstream)
        .unwrap();
    let lookup = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
    assert!(session.advance(&lookup, &mut upstream).unwrap());
    assert_eq!(lookup_result(&upstream)["tools"], json!([]));
    let invalid = response("files.read", json!({"path":"too_short"}));
    assert!(translate_response(&serde_json::to_vec(&invalid).unwrap(), &request).is_err());
    let valid = response("files.read", json!({"path":"synthetic_document"}));
    assert!(translate_response(&serde_json::to_vec(&valid).unwrap(), &request).is_ok());
}
