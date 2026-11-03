use super::*;

fn legacy_lookup(name_key: &str, args_key: &str, encoded: bool) -> Value {
    let arguments = json!({"names":["files.read"]});
    let arguments = if encoded {
        Value::String(arguments.to_string())
    } else {
        arguments
    };
    let mut reply = response(LOOKUP_TOOL, json!({}));
    reply["output"][0]["arguments"] = json!({
        "code": json!({(name_key):LOOKUP_TOOL,(args_key):arguments}).to_string()
    })
    .to_string()
    .into();
    reply
}

#[test]
fn legacy_lookup_aliases_load_the_original_contract_and_stay_local() {
    for (name_key, args_key) in [
        ("tool", "args"),
        ("name", "arguments"),
        ("tool", "arguments"),
        ("name", "args"),
    ] {
        for encoded in [false, true] {
            let request = request();
            let mut upstream = prepare_request(&request).unwrap();
            let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
            let mut reply = legacy_lookup(name_key, args_key, encoded);
            assert!(session.advance(&reply, &mut upstream).unwrap());
            let result = upstream["input"].as_array().unwrap().last().unwrap();
            assert_eq!(result["call_id"], "call_synthetic");
            let output: Value = serde_json::from_str(result["output"].as_str().unwrap()).unwrap();
            assert_eq!(output["tools"][0]["name"], "files.read");
            assert_eq!(session.rounds, 1);
            assert!(session.loaded.contains("files.read"));
            session.finish(&mut reply);
            assert!(reply["output"].as_array().unwrap().is_empty());
            assert!(translate_response(&serde_json::to_vec(&reply).unwrap(), &request).is_ok());
        }
    }
}

#[test]
fn legacy_lookup_aliases_in_mixed_output_do_not_escape_to_client_or_regenerate() {
    let mut request = request();
    request["parallel_tool_calls"] = json!(false);
    let mut upstream = prepare_request(&request).unwrap();
    let before = upstream.clone();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let mut reply = legacy_lookup("name", "arguments", false);
    let mut client = response("files.read", json!({"path":"synthetic.txt"}))["output"][0].clone();
    client["call_id"] = json!("call_client");
    client["id"] = json!("fc_client");
    reply["output"].as_array_mut().unwrap().push(client);
    assert!(!session.advance(&reply, &mut upstream).unwrap());
    assert_eq!(upstream, before);
    assert_eq!(session.rounds, 0);
    assert!(session.loaded.is_empty());
    session.finish(&mut reply);
    let translated: Value = serde_json::from_slice(
        &translate_response(&serde_json::to_vec(&reply).unwrap(), &request).unwrap(),
    )
    .unwrap();
    assert_eq!(translated["output"].as_array().unwrap().len(), 1);
    assert_eq!(translated["output"][0]["name"], "read");
    assert_eq!(translated["output"][0]["namespace"], "files");
    assert_eq!(translated["output"][0]["call_id"], "call_client");
}

#[test]
fn conflicting_legacy_lookup_identity_cannot_load_contracts_or_hide_client_calls() {
    let request = request();
    let mut upstream = prepare_request(&request).unwrap();
    let before = upstream.clone();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let mut reply = response(LOOKUP_TOOL, json!({}));
    reply["output"][0]["arguments"] = json!({"code":json!({
        "tool":LOOKUP_TOOL,"name":"files.read","args":{"names":["files.read"]}
    }).to_string()})
    .to_string()
    .into();
    assert!(!session.advance(&reply, &mut upstream).unwrap());
    assert_eq!(upstream, before);
    assert!(session.loaded.is_empty());
    assert_eq!(session.rounds, 0);
    session.finish(&mut reply);
    assert_eq!(reply["output"].as_array().unwrap().len(), 1);
    let error = translate_response(&serde_json::to_vec(&reply).unwrap(), &request).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("tool_identity_reason=conflicting_names")
    );
}
