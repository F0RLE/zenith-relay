use super::*;

fn state(session: &DiscoverySession) -> Value {
    json!({
        "loaded": session.loaded,
        "lookup_contracts": session.lookup_contracts,
        "rendered_schema_bytes": session.rendered_schema_bytes,
        "call_ids": session.call_ids,
        "rounds": session.rounds,
        "continuation_bytes": session.continuation_bytes,
        "prior_messages": session.prior_messages,
    })
}

fn assert_rejected_without_changes(
    session: &mut DiscoverySession,
    reply: &Value,
    upstream: &mut Value,
    parameter: &str,
) {
    let before_state = state(session);
    let before_upstream = upstream.clone();
    assert_eq!(
        session.advance(reply, upstream).unwrap_err().parameter(),
        Some(parameter)
    );
    assert_eq!(
        state(session),
        before_state,
        "rejected lookup changed session"
    );
    assert_eq!(
        *upstream, before_upstream,
        "rejected lookup changed request"
    );
}

#[test]
fn late_invalid_item_identity_does_not_commit_earlier_lookups() {
    for mixed in [false, true] {
        for invalid in [None, Some(Value::Null), Some(json!(7)), Some(json!(" \t"))] {
            let request = request();
            let mut upstream = prepare_request(&request).unwrap();
            let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
            let mut reply = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
            let mut next = response(LOOKUP_TOOL, json!({"names":["files.write"]}));
            next["output"][0]["call_id"] = json!("call_next");
            match invalid {
                Some(value) => next["output"][0]["id"] = value,
                None => {
                    next["output"][0].as_object_mut().unwrap().remove("id");
                }
            }
            reply["output"]
                .as_array_mut()
                .unwrap()
                .push(next["output"][0].take());
            if mixed {
                let mut client = response("files.read", json!({"path":"synthetic.txt"}));
                client["output"][0]["call_id"] = json!("call_client");
                reply["output"]
                    .as_array_mut()
                    .unwrap()
                    .push(client["output"][0].take());
            }
            assert_rejected_without_changes(
                &mut session,
                &reply,
                &mut upstream,
                "output.relay.catalog.call_id",
            );
        }
    }
}

#[test]
fn rejected_budget_keeps_prior_round_and_unseen_contracts_unchanged() {
    let request = request();
    let mut upstream = prepare_request(&request).unwrap();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    assert!(
        session
            .advance(
                &response(LOOKUP_TOOL, json!({"names":["files.read"]})),
                &mut upstream
            )
            .unwrap()
    );
    let mut reply = response(LOOKUP_TOOL, json!({"names":["files.write"]}));
    reply["output"][0]["call_id"] = json!("call_next");
    reply["output"].as_array_mut().unwrap().insert(
        0,
        json!({
            "type":"message","role":"assistant",
            "content":[{"type":"output_text","text":"x".repeat(MAX_CONTINUATION_BYTES)}]
        }),
    );
    assert_rejected_without_changes(
        &mut session,
        &reply,
        &mut upstream,
        "output.relay.catalog.bytes",
    );
}

#[test]
fn invalid_continuation_input_does_not_commit_lookup_state() {
    for invalid in [
        None,
        Some(Value::Null),
        Some(json!("invalid")),
        Some(json!({})),
    ] {
        let request = request();
        let mut upstream = prepare_request(&request).unwrap();
        match invalid {
            Some(value) => upstream["input"] = value,
            None => {
                upstream.as_object_mut().unwrap().remove("input");
            }
        }
        let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
        assert_rejected_without_changes(
            &mut session,
            &response(LOOKUP_TOOL, json!({"names":["files.read"]})),
            &mut upstream,
            "input",
        );
    }
}

#[test]
fn invalid_iteration_does_not_commit_request_or_lookup_state() {
    for invalid in [
        None,
        Some(Value::Null),
        Some(json!(1)),
        Some(json!("invalid")),
        Some(json!(u64::MAX.to_string())),
    ] {
        let request = request();
        let mut upstream = prepare_request(&request).unwrap();
        upstream["metadata"] = json!({"synthetic":"preserve"});
        if let Some(value) = invalid {
            upstream["metadata"]["agent_iteration"] = value;
        }
        let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
        let mut reply = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
        reply["output"].as_array_mut().unwrap().insert(
            0,
            json!({
                "type":"message","role":"assistant",
                "content":[{"type":"output_text","text":"synthetic lookup"}]
            }),
        );
        assert_rejected_without_changes(
            &mut session,
            &reply,
            &mut upstream,
            "metadata.agent_iteration",
        );
    }
}

#[test]
fn successful_round_commits_original_items_and_iteration_once() {
    let request = request();
    let mut upstream = prepare_request(&request).unwrap();
    upstream["metadata"] = json!({"agent_iteration":"7","synthetic":"preserve"});
    let before_input = upstream["input"].as_array().unwrap().len();
    let mut session = DiscoverySession::new(&request, true).unwrap().unwrap();
    let mut reply = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
    reply["output"].as_array_mut().unwrap().insert(
        0,
        json!({
            "type":"reasoning","id":"rs_synthetic","encrypted_content":"synthetic-opaque"
        }),
    );
    let original = reply.clone();
    assert!(session.advance(&reply, &mut upstream).unwrap());
    assert_eq!(reply, original);
    assert_eq!(
        upstream["metadata"],
        json!({"agent_iteration":"8","synthetic":"preserve"})
    );
    assert_eq!(session.rounds, 1);
    assert_eq!(session.loaded, BTreeSet::from(["files.read".to_owned()]));
    assert_eq!(
        session.call_ids,
        BTreeSet::from(["call_synthetic".to_owned()])
    );
    assert_eq!(
        session
            .lookup_contracts
            .get("files.read")
            .map(String::as_str),
        Some("call_synthetic")
    );
    assert_eq!(session.prior_messages, vec![reply["output"][0].clone()]);
    assert!(session.continuation_bytes > 0);
    let added = &upstream["input"].as_array().unwrap()[before_input..];
    assert_eq!(&added[..2], reply["output"].as_array().unwrap());
    assert_eq!(added.len(), 3);
    assert_eq!(added[2]["call_id"], "call_synthetic");
    let result: Value = serde_json::from_str(added[2]["output"].as_str().unwrap()).unwrap();
    assert_eq!(result["tools"][0]["name"], "files.read");
}

#[test]
fn same_round_reuse_keeps_first_identity_and_counts_new_schemas_once() {
    for diagnostics_enabled in [false, true] {
        let request = request();
        let mut upstream = prepare_request(&request).unwrap();
        let mut session = DiscoverySession::new(&request, diagnostics_enabled)
            .unwrap()
            .unwrap();
        let mut reply = response(LOOKUP_TOOL, json!({"names":["files.read"]}));
        for (id, args) in [
            ("call_invalid", json!({"names":[]})),
            ("call_next", json!({"names":["files.read","files.write"]})),
        ] {
            let mut next = response(LOOKUP_TOOL, args);
            next["output"][0]["call_id"] = json!(id);
            next["output"][0]["id"] = json!(format!("fc_{id}"));
            reply["output"]
                .as_array_mut()
                .unwrap()
                .push(next["output"][0].take());
        }
        assert!(session.advance(&reply, &mut upstream).unwrap());
        let input = upstream["input"].as_array().unwrap();
        let results = input[input.len() - 3..]
            .iter()
            .map(|item| serde_json::from_str::<Value>(item["output"].as_str().unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results[0]["tools"][0]["name"], "files.read");
        assert_eq!(results[1]["error"], "invalid_catalog_lookup");
        assert_eq!(results[2]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(results[2]["tools"][0]["name"], "files.write");
        assert_eq!(
            results[2]["reused"],
            json!([{"name":"files.read","call_id":"call_synthetic"}])
        );
        assert_eq!(
            session.lookup_contracts,
            BTreeMap::from([
                ("files.read".to_owned(), "call_synthetic".to_owned()),
                ("files.write".to_owned(), "call_next".to_owned()),
            ])
        );
        assert_eq!(session.loaded.len(), 2);
        assert_eq!(session.call_ids.len(), 3);
        assert_eq!(session.rounds, 1);
        // Lookups render original full schemas, not the compact preload form.
        let expected_bytes = request["tools"][0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| crate::tool_policy::serialized_bytes(&tool["parameters"]))
            .sum::<u64>();
        assert_eq!(
            session.rendered_schema_bytes,
            diagnostics_enabled.then_some(expected_bytes)
        );
    }
}
