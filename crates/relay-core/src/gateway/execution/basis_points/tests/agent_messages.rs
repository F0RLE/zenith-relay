use super::*;
use crate::protocol::agent_messages::AgentMessageContext;

fn encrypted_request(property: Value) -> Value {
    json!({"model":"synthetic","input":"Delegate a synthetic task","tools":[{
        "type":"namespace","name":"collaboration","tools":[{
            "type":"function","name":"spawn_agent","parameters":{
                "type":"object","properties":{
                    "message":property,"task_name":{"type":"string"}
                },"required":["task_name"],"additionalProperties":false
            }
        }]
    }]})
}

fn reply(arguments: Value) -> Value {
    json!({"id":"resp_synthetic","status":"completed","output":[{
        "type":"function_call","id":"fc_synthetic","call_id":"call_synthetic",
        "name":"run_officejs","arguments":json!({
            "references":["collaboration.spawn_agent"],"code":arguments.to_string()
        }).to_string()
    }]})
}

#[test]
fn agent_messages_roundtrip_buffered_and_parsed_responses_before_history_lowering() {
    let context = AgentMessageContext::new(Default::default(), "synthetic-key");
    let task = "Synthetic task\nпроверка: do not change unrelated files";
    let request = encrypted_request(json!({"type":"string","encrypted":true}));
    let (prepared, _) = PreparedClientRequest::prepare(request.clone(), &mut None, false).unwrap();
    let prepared = prepared.with_agent_messages(context.clone());
    for buffered in [false, true] {
        let native = reply(json!({"message":task,"task_name":"worker"}));
        let translated = if buffered {
            serde_json::from_slice(
                &prepared
                    .translate_bytes(&serde_json::to_vec(&native).unwrap())
                    .unwrap(),
            )
            .unwrap()
        } else {
            prepared.translate_value(native).unwrap()
        };
        let call = &translated["output"][0];
        assert_eq!(call["name"], "spawn_agent");
        assert_eq!(call["namespace"], "collaboration");
        let arguments: Value = serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
        assert_ne!(arguments["message"], task);
        assert!(!translated.to_string().contains("Synthetic task"));
        assert_eq!(arguments["task_name"], "worker");

        let mut child = json!({"model":"synthetic","input":[{
            "type":"agent_message","author":"/root","recipient":"/root/worker",
            "role":"developer","content":[{"type":"encrypted_content","encrypted_content":arguments["message"]}]
        }]});
        context.restore_request(&mut child).unwrap();
        let child_wire = prepare_request(&child).unwrap();
        let user = child_wire["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["role"] == "user")
            .unwrap();
        assert_eq!(user["content"][1]["text"], task);
        assert!(
            user["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("/root/worker")
        );

        let mut parent = request.clone();
        parent["input"] = json!([
            {"role":"user","content":"Delegate a synthetic task"},call,
            {"type":"function_call_output","call_id":"call_synthetic","output":"worker started"}
        ]);
        context.restore_request(&mut parent).unwrap();
        let restored: Value =
            serde_json::from_str(parent["input"][1]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(restored["message"], task);
        let wire = prepare_request(&parent).unwrap();
        assert!(!wire.to_string().contains("zenith-relay-agent-v1:"));
        assert!(wire.to_string().contains("Synthetic task"));
    }
}

#[test]
fn agent_messages_optional_null_and_regular_arguments_are_preserved() {
    let context = AgentMessageContext::new(Default::default(), "synthetic-key");
    let (prepared, _) = PreparedClientRequest::prepare(
        encrypted_request(json!({"type":["string","null"],"encrypted":true})),
        &mut None,
        false,
    )
    .unwrap();
    let prepared = prepared.with_agent_messages(context);
    for arguments in [
        json!({"task_name":"worker"}),
        json!({"task_name":"worker","message":null}),
    ] {
        let response = prepared.translate_value(reply(arguments.clone())).unwrap();
        let actual: Value =
            serde_json::from_str(response["output"][0]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(actual, arguments);
    }
    let (ordinary, _) = PreparedClientRequest::prepare(
        encrypted_request(json!({"type":"string","encrypted":false})),
        &mut None,
        false,
    )
    .unwrap();
    let response = ordinary
        .translate_value(reply(
            json!({"task_name":"worker","message":"ordinary text"}),
        ))
        .unwrap();
    assert!(
        response["output"][0]["arguments"]
            .as_str()
            .unwrap()
            .contains("ordinary text")
    );
}

#[test]
fn agent_messages_validate_plaintext_before_sealing_and_fail_without_context() {
    let (prepared, _) = PreparedClientRequest::prepare(
        encrypted_request(json!({"type":"string","encrypted":true,"minLength":10})),
        &mut None,
        false,
    )
    .unwrap();
    let error = prepared
        .translate_value(reply(json!({"task_name":"worker","message":"short"})))
        .unwrap_err();
    assert_eq!(error.parameter(), Some("output.run_officejs.args.schema"));
    let error = prepared
        .translate_value(reply(
            json!({"task_name":"worker","message":"valid synthetic message"}),
        ))
        .unwrap_err();
    assert_eq!(error.parameter(), Some("tools.parameters.encrypted"));
}

#[test]
fn agent_messages_reject_unsupported_encryption_shapes_before_dispatch() {
    for property in [
        json!({"type":"integer","encrypted":true}),
        json!({"anyOf":[{"type":"string","encrypted":true},{"type":"null"}]}),
        json!({"type":"array","items":{"type":"string","encrypted":true}}),
        json!({"type":"object","properties":{"nested":{"type":"string","encrypted":true}}}),
        json!({"type":"string","encrypted":true,"allOf":[{"encrypted":true}]}),
        json!({"type":"string","$defs":{"secret":{"type":"string","encrypted":true}}}),
    ] {
        let error = prepare_request(&encrypted_request(property)).unwrap_err();
        assert_eq!(error.parameter(), Some("tools.parameters.encrypted"));
    }
}

#[test]
fn agent_messages_literal_annotations_inside_defaults_are_not_schema_keywords() {
    let request = encrypted_request(
        json!({"type":"object","properties":{"encrypted":{"type":"boolean"}},
        "default":{"encrypted":true},"examples":[{"encrypted":true}]}),
    );
    prepare_request(&request).unwrap();
}

#[test]
fn agent_messages_reject_reference_aliases_of_encrypted_properties() {
    let mut request = encrypted_request(json!({"type":"string","encrypted":true}));
    request["tools"][0]["tools"][0]["parameters"]["properties"]["alias"] =
        json!({"$ref":"#/properties/message"});
    let error = prepare_request(&request).unwrap_err();
    assert_eq!(error.parameter(), Some("tools.parameters.encrypted"));
}
