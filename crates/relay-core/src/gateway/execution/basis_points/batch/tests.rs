use super::super::catalog::client_tools;
use super::super::history::{separate_tool_images, translate_input_items};
use super::super::response::translate_response_value;
use super::*;

mod throughput;

fn request() -> Value {
    json!({"model":"synthetic", "input":[], "tools":[
        {"type":"function","name":"inspect","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}},
        {"type":"namespace","name":"files","tools":[{"type":"custom","name":"patch"}]}
    ]})
}

fn native(references: Value, payloads: Value) -> Value {
    json!({"type":"function_call","id":"fc_parent","call_id":"parent","name":TRANSPORT_TOOL,"status":"completed",
        "arguments":json!({"references":references,"code":format!("{PREFIX}{payloads}")}).to_string()})
}

fn response(item: Value) -> Value {
    json!({"id":"resp_synthetic","status":"completed","output":[item]})
}

fn pair() -> Value {
    native(
        json!(["inspect", "inspect"]),
        json!([{"path":"a"},{"path":"b"}]),
    )
}

fn calls() -> Vec<Value> {
    translate_response_value(response(pair()), &request()).unwrap()["output"]
        .as_array()
        .unwrap()
        .clone()
}

fn history() -> Vec<Value> {
    let mut items = calls();
    for call in items.clone() {
        items.push(json!({"type":"function_call_output","call_id":call["call_id"],"output":"ok"}));
    }
    items
}

#[test]
fn batch_payload_accepts_crlf_without_changing_custom_input() {
    let raw = "*** Begin Patch\r\n+literal \\n and \"quotes\"\r\n*** End Patch";
    let mut item = native(json!(["inspect", "files.patch"]), json!([{"path":"a"},raw]));
    item["arguments"] = json!({
        "references":["inspect","files.patch"],
        "code":format!("relay-tool-batch-v1\r\n{}", json!([{"path":"a"},raw]))
    })
    .to_string()
    .into();
    let translated = translate_response_value(response(item), &request()).unwrap();
    assert_eq!(translated["output"][1]["input"], raw);
    assert_eq!(
        serde_json::from_str::<Value>(translated["output"][0]["arguments"].as_str().unwrap())
            .unwrap(),
        json!({"path":"a"})
    );
}

#[test]
fn batch_payload_uses_the_same_lossless_json_rules_as_single_functions() {
    let path = "C:\\synthetic\\q\n\tline";
    let raw = "*** Begin Patch\n+literal \\n and \"quotes\"\n*** End Patch";
    let code = format!("{PREFIX}[{{\"path\":\"{path}\"}},{}]", json!(raw));
    let mut item = pair();
    item["arguments"] = json!({"references":["inspect","files.patch"],"code":code})
        .to_string()
        .into();
    let translated = translate_response_value(response(item), &request()).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(translated["output"][0]["arguments"].as_str().unwrap())
            .unwrap(),
        json!({"path":path})
    );
    assert_eq!(translated["output"][1]["input"], raw);
}

#[test]
fn batch_failures_keep_safe_causes_and_never_expand_a_partial_batch() {
    for (code, suffix) in [
        ("[]", "prefix"),
        ("relay-tool-batch-v1\n{}", "array"),
        ("relay-tool-batch-v1\n[]", "count"),
        (
            "relay-tool-batch-v1\n[{\"path\":\"synthetic-private\"},null]",
            "payload",
        ),
        (
            "relay-tool-batch-v1\n[{\"path\":\"synthetic-private\\u12X4\"},{}]",
            "json",
        ),
        (
            "relay-tool-batch-v1\n[{\"path\":\"synthetic-private\nvalue\"},",
            "json",
        ),
        (
            "relay-tool-batch-v1\n[{\"path\":\"synthetic-private\"},{\"path\":\"b\"}]]",
            "json",
        ),
        (
            "relay-tool-batch-v1\n[{\"path\":\"synthetic-private\"},{\"path\":\"b\"}]{}",
            "json",
        ),
    ] {
        let mut item = pair();
        item["arguments"] = json!({"references":["inspect","inspect"],"code":code})
            .to_string()
            .into();
        let mut items = vec![item];
        let original = items.clone();
        let error =
            expand_output(&mut items, &request(), &client_tools(&request()).unwrap()).unwrap_err();
        assert_eq!(
            error.parameter().unwrap(),
            format!("output.run_officejs.batch.{suffix}")
        );
        assert_eq!(items, original);
        let message = error.to_string();
        assert!(!message.contains("synthetic-private"));
        if suffix == "json" {
            assert!(message.contains("json_reason="));
            assert!(message.contains("payload_shape=json_array"));
        }
    }
}

#[test]
fn repeated_tools_namespaces_and_raw_payloads_roundtrip_without_losing_reasoning() {
    let path = "C:\\synthetic\\a\n\"quoted\"\tline";
    let raw = "*** Begin Patch\n+literal \\n and \"quotes\"\n*** End Patch";
    let framed = format!(
        "{}{}\n{path}",
        super::super::FUNCTION_STRING_PREFIX,
        serde_json::to_string_pretty(&json!({"field":"path","args":{}})).unwrap()
    );
    let native = native(
        json!(["inspect", "inspect", "files.patch"]),
        json!([{"path":path},framed,raw]),
    );
    let reasoning = json!({"type":"reasoning","id":"rs_synthetic","summary":[],"encrypted_content":"synthetic-opaque"});
    let mut reply = response(native);
    reply["output"]
        .as_array_mut()
        .unwrap()
        .insert(0, reasoning.clone());
    let translated = translate_response_value(reply, &request()).unwrap();
    assert_eq!(translated["output"][0], reasoning);
    let mut items = translated["output"].as_array().unwrap().clone();
    let ids = items[1..]
        .iter()
        .map(|call| call["call_id"].as_str().unwrap())
        .collect::<HashSet<_>>();
    assert_eq!(ids.len(), 3);
    for item in &items[1..3] {
        assert_eq!(
            serde_json::from_str::<Value>(item["arguments"].as_str().unwrap()).unwrap(),
            json!({"path":path})
        );
        assert_eq!(
            parent_call_id(item["call_id"].as_str().unwrap()).as_deref(),
            Some("parent")
        );
    }
    assert_eq!(items[3]["name"], "patch");
    assert_eq!(items[3]["namespace"], "files");
    assert_eq!(items[3]["input"], raw);
    let image = json!({"type":"input_image","image_url":"data:image/png;base64,c3ludGhldGlj","detail":"low"});
    let file = json!({"type":"input_file","filename":"synthetic.txt","file_data":"data:text/plain;base64,b2s="});
    // Tool completion order is not the declaration order.
    items.extend([
        json!({"type":"custom_tool_call_output","call_id":items[3]["call_id"],"output":[image.clone(),file.clone()]}),
        json!({"type":"function_call_output","call_id":items[2]["call_id"],"output":"{\"literal\":true}"}),
        json!({"type":"function_call_output","call_id":items[1]["call_id"],"output":"failed: synthetic","is_error":true}),
    ]);
    let tools = client_tools(&request()).unwrap();
    let restored = translate_input_items(items.clone(), &tools).unwrap();
    assert_eq!(restored[0], reasoning);
    assert_eq!(restored[1]["call_id"], "parent");
    assert_eq!(restored[2]["call_id"], "parent");
    let replay = translate_response_value(response(restored[1].clone()), &request()).unwrap();
    assert_eq!(
        &replay["output"],
        &Value::Array(translated["output"].as_array().unwrap()[1..].to_vec())
    );
    let parts = restored[2]["output"].as_array().unwrap();
    assert_eq!(parts[1]["text"], "failed: synthetic");
    assert!(parts[0]["text"]
        .as_str()
        .unwrap()
        .contains("\"is_error\":true"));
    assert_eq!(parts[3]["text"], "{\"literal\":true}");
    assert_eq!(parts[5], image);
    assert_eq!(parts[6], file);
    let with_images = separate_tool_images(restored);
    assert_eq!(with_images.len(), 4);
    assert_eq!(with_images[3]["role"], "user");
    assert_eq!(with_images[3]["content"][2], image);
    assert_eq!(with_images[2]["output"][6], file);
    // Current schemas can change or disappear without corrupting old history.
    assert!(translate_input_items(items, &[]).is_ok());
}

#[test]
fn bounded_aliases_are_stable_canonical_and_reconstruct_before_id_limiting() {
    let parent = "x".repeat(64);
    let mut item = pair();
    item["call_id"] = json!(parent);
    let output = translate_response_value(response(item), &request()).unwrap();
    let mut history = output["output"].as_array().unwrap().clone();
    for call in history.clone() {
        assert!(call["call_id"].as_str().unwrap().len() > 64);
        history
            .push(json!({"type":"function_call_output","call_id":call["call_id"],"output":"ok"}));
    }
    let restored = translate_input_items(history, &client_tools(&request()).unwrap()).unwrap();
    assert_eq!(restored[0]["call_id"], parent);
    assert_eq!(restored[1]["call_id"], parent);
    for id in [
        "call_rb1.02.0.cGFyZW50",
        "call_rb1.2.2.cGFyZW50",
        "call_rb1.1.0.cGFyZW50",
        "call_rb1.2.0.!",
        "call_rb1.2.0.",
    ] {
        assert!(Alias::decode(id).is_err(), "{id}");
    }
}

#[test]
fn near_limit_function_objects_replay_without_double_encoding() {
    let path = "\\\"".repeat((MAX_BYTES - 128) / 8);
    let native = native(
        json!(["inspect", "inspect"]),
        json!([{"path":path},{"path":path}]),
    );
    let reply = translate_response_value(response(native), &request()).unwrap();
    let mut history = reply["output"].as_array().unwrap().clone();
    for call in history.clone() {
        history
            .push(json!({"type":"function_call_output","call_id":call["call_id"],"output":"ok"}));
    }
    let restored = collapse_history(history, &client_tools(&request()).unwrap()).unwrap();
    let replay = translate_response_value(response(restored[0].clone()), &request()).unwrap();
    assert_eq!(replay["output"], reply["output"]);
}

#[test]
fn malformed_or_disallowed_batches_release_nothing() {
    let mut cases = vec![
        native(json!(["inspect", "inspect"]), json!([{"path":"a"}])),
        native(json!(["inspect", "inspect"]), json!([{"path":"a"},null])),
        native(
            json!(["inspect", "inspect"]),
            json!([{"path":"a"},{"path":1}]),
        ),
        native(
            json!(["inspect", "files.patch"]),
            json!([{"path":"a"},{"patch":"not raw"}]),
        ),
        native(
            json!(["inspect", "relay.catalog"]),
            json!([{"path":"a"},{"names":["inspect"]}]),
        ),
        native(json!(["inspect", "run_officejs"]), json!([{"path":"a"},{}])),
        native(json!(["inspect", "unknown"]), json!([{"path":"a"},{}])),
        native(json!(["inspect", null]), json!([{"path":"a"},{}])),
        native(
            json!(vec!["inspect"; 9]),
            json!(vec![json!({"path":"a"}); 9]),
        ),
    ];
    for code in [
        "[{\"path\":\"a\"},{\"path\":\"b\"}]".to_owned(),
        format!("{PREFIX}[broken]"),
        format!("{PREFIX}{}", " ".repeat(MAX_BYTES)),
    ] {
        let mut item = pair();
        item["arguments"] = json!({"references":["inspect","inspect"],"code":code})
            .to_string()
            .into();
        cases.push(item);
    }
    for id in [
        "".to_string(),
        "x".repeat(65),
        "parent\n".to_string(),
        Alias {
            parent: "parent".into(),
            count: 2,
            index: 0,
        }
        .encode(),
    ] {
        let mut item = pair();
        item["call_id"] = json!(id);
        cases.push(item);
    }
    for (index, item) in cases.into_iter().enumerate() {
        assert!(
            translate_response_value(response(item), &request()).is_err(),
            "case {index}"
        );
    }
    for choice in [
        json!("none"),
        json!({"type":"custom","name":"patch","namespace":"files"}),
    ] {
        let mut request = request();
        request["tool_choice"] = choice;
        assert!(translate_response_value(response(pair()), &request).is_err());
    }
    let mut request = request();
    request["parallel_tool_calls"] = json!(false);
    assert_eq!(
        translate_response_value(response(pair()), &request)
            .unwrap_err()
            .parameter(),
        Some("output.run_officejs.parallel_tool_calls")
    );
}

#[test]
fn duplicate_replayed_and_overexpanded_native_calls_are_rejected_atomically() {
    let mut reply = response(pair());
    reply["output"].as_array_mut().unwrap().push(pair());
    assert!(translate_response_value(reply, &request()).is_err());
    let mut replay_request = request();
    replay_request["input"] = json!(history());
    assert!(translate_response_value(response(pair()), &replay_request).is_err());
    replay_request["input"] =
        json!([{"type":"function_call_output","call_id":"parent","output":"prior"}]);
    assert!(translate_response_value(response(pair()), &replay_request).is_err());
    let mut items = vec![json!({"type":"message"}); MAX_OUTPUT_ITEMS - 1];
    items.push(pair());
    let before = items.clone();
    assert!(expand_output(&mut items, &request(), &client_tools(&request()).unwrap()).is_err());
    assert_eq!(items, before);
}

#[test]
fn noncompleted_native_items_never_release_single_or_batched_calls() {
    for batched in [false, true] {
        for status in [
            json!("in_progress"),
            json!("incomplete"),
            json!("unknown"),
            json!(null),
            json!(1),
        ] {
            let mut item = pair();
            if !batched {
                item["arguments"] = json!({"references":["inspect"],"code":"{\"path\":\"a\"}"})
                    .to_string()
                    .into();
            }
            item["status"] = status;
            item["call_id"] = json!("unfinished");
            let mut reply = response(pair());
            reply["output"].as_array_mut().unwrap().push(item);
            assert_eq!(
                translate_response_value(reply, &request())
                    .unwrap_err()
                    .parameter(),
                Some("output.run_officejs.status")
            );
        }
    }
}

#[test]
fn native_item_status_remains_optional() {
    for batched in [false, true] {
        let mut item = pair();
        if !batched {
            item["arguments"] = json!({"references":["inspect"],"code":"{\"path\":\"a\"}"})
                .to_string()
                .into();
        }
        item.as_object_mut().unwrap().remove("status");
        let translated = translate_response_value(response(item), &request()).unwrap();
        let calls = translated["output"].as_array().unwrap();
        assert_eq!(calls.len(), if batched { 2 } else { 1 });
        assert!(calls.iter().all(|call| call["status"] == "completed"));
    }
}

#[test]
fn incomplete_response_preserves_reasoning_and_usage_but_never_a_batch() {
    let mut reply = response(pair());
    reply["status"] = json!("incomplete");
    reply["usage"] = json!({"output_tokens":7});
    reply["output"].as_array_mut().unwrap().insert(
        0,
        json!({"type":"reasoning","encrypted_content":"synthetic"}),
    );
    let output = translate_response_value(reply, &request()).unwrap();
    assert_eq!(output["status"], "incomplete");
    assert_eq!(output["usage"]["output_tokens"], 7);
    assert_eq!(output["output"].as_array().unwrap().len(), 1);
}

#[test]
fn interleaved_parallel_batches_and_single_results_preserve_identity_and_boundaries() {
    let mut other = native(
        json!(["files.patch", "inspect"]),
        json!(["synthetic patch", {"path":"c"}]),
    );
    other["call_id"] = json!("other");
    other["id"] = json!("fc_other");
    let mut reply = response(pair());
    reply["output"].as_array_mut().unwrap().extend([
        other,
        json!({"type":"function_call","name":TRANSPORT_TOOL,"call_id":"solo","arguments":json!({"references":["inspect"],"code":"{\"path\":\"d\"}"}).to_string()}),
    ]);
    let calls = translate_response_value(reply, &request()).unwrap()["output"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(calls.len(), 5);
    let tools = client_tools(&request()).unwrap();
    for (order, parents) in [
        ([3, 1, 4, 2, 0], ["other", "parent", "solo"]),
        ([0, 2, 4, 1, 3], ["parent", "other", "solo"]),
        ([4, 1, 3, 0, 2], ["solo", "parent", "other"]),
    ] {
        let mut items = calls.clone();
        for index in order {
            items.push(json!({
                "type":format!("{}_output",calls[index]["type"].as_str().unwrap()),
                "call_id":calls[index]["call_id"],"output":format!("result-{index}")
            }));
        }
        let boundary = json!({"role":"developer","content":"Must remain after the results"});
        items.push(boundary.clone());
        for catalog in [&tools[..], &[][..]] {
            let restored = collapse_history(items.clone(), catalog).unwrap();
            assert_eq!(restored.len(), 7);
            assert_eq!(restored[6], boundary);
            for (result, parent) in restored[3..6].iter().zip(parents) {
                assert_eq!(result["call_id"], parent);
                if parent == "solo" {
                    assert_eq!(result["output"], "result-4");
                } else {
                    let first = if parent == "parent" { 0 } else { 2 };
                    assert_eq!(result["output"][1]["text"], format!("result-{first}"));
                    assert_eq!(result["output"][3]["text"], format!("result-{}", first + 1));
                    assert!(result["output"][0]["text"]
                        .as_str()
                        .unwrap()
                        .contains(calls[first]["call_id"].as_str().unwrap()));
                }
            }
        }
        let mut interrupted = items.clone();
        interrupted.insert(7, boundary.clone());
        let restored = collapse_history(interrupted, &tools).unwrap();
        assert_eq!(restored.iter().filter(|item| **item == boundary).count(), 2);
        let duplicate = items[5..10]
            .iter()
            .find(|item| item["call_id"] == calls[0]["call_id"])
            .unwrap()
            .clone();
        items.insert(6, duplicate);
        assert!(collapse_history(items, &tools).is_err());
    }
}

#[test]
fn interrupted_batch_results_replay_independently_without_moving_messages() {
    let tools = client_tools(&request()).unwrap();
    for boundary in [
        json!({"type":"message","role":"developer","content":[{"type":"input_text","text":"Synthetic service instruction"}]}),
        json!({"type":"message","role":"user","content":[{"type":"input_text","text":"Synthetic user message"}]}),
        json!({"type":"reasoning","id":"rs_synthetic","summary":[],"encrypted_content":"synthetic-opaque"}),
    ] {
        let mut items = calls();
        let first = items[0]["call_id"].clone();
        let second = items[1]["call_id"].clone();
        let image = json!({"type":"input_image","image_url":"data:image/png;base64,c3ludGhldGlj"});
        items.extend([
            json!({"type":"function_call_output","call_id":first,"output":[image.clone()]}),
            boundary.clone(),
            json!({"type":"function_call_output","call_id":second,"output":"synthetic result","is_error":true}),
        ]);
        for catalog in [&tools[..], &[][..]] {
            let restored = translate_input_items(items.clone(), catalog).unwrap();
            assert_eq!(restored.len(), 5);
            assert_eq!(restored[0]["name"], TRANSPORT_TOOL);
            assert_eq!(restored[1]["name"], TRANSPORT_TOOL);
            assert_eq!(restored[0]["call_id"], first);
            assert_eq!(restored[1]["call_id"], second);
            assert_ne!(restored[0]["call_id"], restored[1]["call_id"]);
            assert_eq!(restored[2]["call_id"], restored[0]["call_id"]);
            assert_eq!(restored[2]["output"][0], image);
            assert_eq!(restored[3], boundary);
            assert_eq!(restored[4]["call_id"], restored[1]["call_id"]);
            assert_eq!(restored[4]["output"], "synthetic result");
            assert_eq!(restored[4]["is_error"], true);
            for item in &restored {
                if let Some(id) = item["id"].as_str() {
                    assert!(id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')));
                }
            }
        }
    }
}

#[test]
fn interrupted_batch_history_still_rejects_missing_duplicate_or_mismatched_children() {
    let tools = client_tools(&request()).unwrap();
    let mut baseline = history();
    baseline.insert(3, json!({"role":"developer","content":"Must remain here"}));
    let mut cases = Vec::new();
    for index in 0..baseline.len() {
        if index == 3 {
            continue;
        }
        let mut items = baseline.clone();
        items.remove(index);
        cases.push(items);
    }
    for (field, value) in [
        ("call_id", baseline[2]["call_id"].clone()),
        ("type", json!("custom_tool_call_output")),
        ("status", json!("in_progress")),
        ("output", json!(42)),
        ("output", json!([42])),
    ] {
        let mut items = baseline.clone();
        items[4][field] = value;
        cases.push(items);
    }
    let mut collision = baseline.clone();
    collision.push(json!({"type":"function_call_output","call_id":"parent","output":"collision"}));
    cases.push(collision);
    let mut interrupted_definitions = baseline.clone();
    interrupted_definitions.insert(
        1,
        json!({"role":"developer","content":"Must remain here too"}),
    );
    cases.push(interrupted_definitions);
    let mut duplicate_definition = baseline.clone();
    duplicate_definition[1] = duplicate_definition[0].clone();
    cases.push(duplicate_definition);
    for (index, items) in cases.into_iter().enumerate() {
        assert!(collapse_history(items, &tools).is_err(), "case {index}");
    }
}

#[test]
fn incomplete_or_ambiguous_history_fails_closed_without_reordering_authority() {
    let tools = client_tools(&request()).unwrap();
    let baseline = history();
    let mut cases = vec![baseline[..2].to_vec(), baseline[2..].to_vec()];
    for index in 0..4 {
        let mut items = baseline.clone();
        items.remove(index);
        cases.push(items);
    }
    let mut duplicate = baseline.clone();
    duplicate[3] = duplicate[2].clone();
    cases.push(duplicate);
    let mut reordered = baseline.clone();
    reordered.swap(0, 1);
    cases.push(reordered);
    let mut interrupted_definitions = baseline.clone();
    interrupted_definitions.insert(1, json!({"role":"developer","content":"must not move"}));
    cases.push(interrupted_definitions);
    for (field, value) in [
        ("type", json!("custom_tool_call_output")),
        ("status", json!("in_progress")),
        ("output", json!(42)),
        ("call_id", json!("call_rb1.2.0.invalid")),
    ] {
        let mut items = baseline.clone();
        items[2][field] = value;
        cases.push(items);
    }
    let mut collision = baseline.clone();
    collision.push(json!({"type":"function_call_output","call_id":"parent","output":"collision"}));
    cases.push(collision);
    let mut repeated = baseline.clone();
    repeated.extend(baseline.clone());
    cases.push(repeated);
    for (index, items) in cases.into_iter().enumerate() {
        assert!(collapse_history(items, &tools).is_err(), "case {index}");
    }
    let message = json!({"role":"developer","content":"must stay here"});
    let mut items = baseline;
    items.insert(2, message.clone());
    let output = collapse_history(items, &tools).unwrap();
    assert_eq!(output[1], message);
}

#[test]
fn messages_bridge_replays_batch_aliases_with_owned_reasoning_and_next_answer() {
    use crate::{
        AdapterRequestContext, CacheWriteTtl, MessagesReasoningMode, SourceAdapter, WireApi,
    };
    let request = json!({
        "max_tokens":4096,"messages":[{"role":"user","content":"Inspect synthetic files"}],
        "thinking":{"type":"adaptive"},"output_config":{"effort":"high"},
        "tools":[{"name":"inspect","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}]
    });
    let prepared = SourceAdapter::MessagesToResponses
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Messages,
            request: &request,
            model: "synthetic",
            stream: true,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: None,
            response_scope: "synthetic-batch",
            response_id_seed: "first",
        })
        .unwrap();
    let mut batch = pair();
    batch["call_id"] = json!("p".repeat(64));
    let mut native = response(batch);
    let reasoning = json!({"type":"reasoning","id":"rs_synthetic","summary":[{"type":"summary_text","text":"Inspect both"}],"encrypted_content":"synthetic-opaque"});
    native["output"]
        .as_array_mut()
        .unwrap()
        .insert(0, reasoning.clone());
    let translated = translate_response_value(native, prepared.upstream_body()).unwrap();
    let bytes = super::super::response::synthetic_stream(&serde_json::to_vec(&translated).unwrap())
        .unwrap();
    let mut bridge = prepared.into_stream_bridge().unwrap();
    for chunk in bytes.chunks(13) {
        bridge.push(chunk);
    }
    bridge.finish();
    let completed = bridge.completed().expect("batch reaches Messages");
    assert_eq!(completed.response_body["stop_reason"], "tool_use");
    let content = completed.response_body["content"].as_array().unwrap();
    let results: Vec<_> = content
        .iter()
        .filter(|part| part["type"] == "tool_use")
        .map(|part| json!({"type":"tool_result","tool_use_id":part["id"],"content":"ok"}))
        .collect();
    assert_eq!(results.len(), 2);
    let mut next = request;
    next["messages"].as_array_mut().unwrap().extend([
        json!({"role":"assistant","content":content}),
        json!({"role":"user","content":results}),
    ]);
    let continued = SourceAdapter::MessagesToResponses
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Messages,
            request: &next,
            model: "synthetic",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: Some(completed.continuation.clone()),
            response_scope: "synthetic-batch",
            response_id_seed: "next",
        })
        .unwrap();
    let upstream = super::super::prepare_request(continued.upstream_body()).unwrap();
    let input = upstream["input"].as_array().unwrap();
    assert!(input.contains(&reasoning));
    let calls: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "function_call")
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["call_id"], "p".repeat(64));
    assert_eq!(
        input
            .iter()
            .filter(|item| item["type"] == "function_call_output")
            .count(),
        1
    );
    let final_reply = continued.translate_response_bytes(&serde_json::to_vec(&json!({
        "id":"resp_final","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Ready"}]}]
    })).unwrap()).unwrap().unwrap();
    assert_eq!(
        final_reply.translated_response().response_body["stop_reason"],
        "end_turn"
    );
}
