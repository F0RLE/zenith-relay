use super::*;

fn replay_items(request: &Value) -> Vec<Value> {
    prepare_request(request).unwrap()["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            matches!(
                item["type"].as_str(),
                Some("function_call" | "function_call_output")
            )
        })
        .cloned()
        .collect()
}

fn request(call: Value) -> Value {
    json!({"model":"synthetic","tools":[],"input":[
        call,
        {"type":format!("{}_output",call["type"].as_str().unwrap()),
            "call_id":call["call_id"],"output":"Synthetic historical result"},
        {"role":"user","content":"Synthetic continuation"}]})
}

#[test]
fn history_kind_changes_preserve_recorded_payloads() {
    for custom in [false, true] {
        let mut call = json!({"type":if custom {"custom_tool_call"} else {"function_call"},
            "name":"execute","namespace":"synthetic","call_id":"call_previous"});
        if custom {
            call["input"] = json!("raw synthetic input\n\"quoted\" C:\\synthetic\\file");
        } else {
            call["arguments"] = json!({"script":"synthetic\n\"quoted\" C:\\synthetic\\file"})
                .to_string()
                .into();
        }
        let mut request = request(call);
        let expected = replay_items(&request);
        request["tools"] = json!([{"type":"namespace","name":"synthetic","tools":[{
            "type":if custom {"function"} else {"custom"},"name":"execute"}]}]);
        assert_eq!(replay_items(&request), expected);
    }
}

#[test]
fn history_names_do_not_follow_the_current_catalog() {
    let mut request = request(json!({"type":"function_call","name":"inspect",
        "call_id":"call_previous","arguments":"{}"}));
    let expected = replay_items(&request);
    request["tools"] = json!([{"type":"namespace","name":"different","tools":[{
        "type":"function","name":"inspect"}]}]);
    assert_eq!(replay_items(&request), expected);
}

fn historical_call(id: &str, custom: bool) -> Value {
    if custom {
        json!({"type":"custom_tool_call","name":"synthetic","call_id":id,"input":"Synthetic input"})
    } else {
        json!({"type":"function_call","name":"synthetic","call_id":id,"arguments":"{}"})
    }
}

#[test]
fn incoming_history_rejects_distinct_ids_that_collapse_to_one() {
    let long = format!("call_{}", "synthetic".repeat(16));
    let fitted = super::super::history::fit_item_id(&long);
    for custom in [false, true] {
        for ids in [[&long, &fitted], [&fitted, &long]] {
            let mut first = request(historical_call(ids[0], custom));
            let second = request(historical_call(ids[1], custom));
            first["input"]
                .as_array_mut()
                .unwrap()
                .extend(second["input"].as_array().unwrap().clone());
            let original = first.clone();
            let error = prepare_request(&first).unwrap_err();
            assert_eq!(error.parameter(), Some("input.call_id"));
            assert_eq!(first, original);
        }
    }
}

#[test]
fn incoming_history_rejects_duplicate_call_or_result_identity() {
    for custom in [false, true] {
        for index in [0, 1] {
            let mut request = request(historical_call("call_synthetic", custom));
            let duplicate = request["input"][index].clone();
            request["input"].as_array_mut().unwrap().push(duplicate);
            let error = prepare_request(&request).unwrap_err();
            assert_eq!(error.parameter(), Some("input.call_id"));
        }
    }
}

#[test]
fn incoming_history_rejects_mismatched_client_result_kinds() {
    for custom in [false, true] {
        let mut request = request(historical_call("call_synthetic", custom));
        request["input"][1]["type"] = json!(if custom {
            "function_call_output"
        } else {
            "custom_tool_call_output"
        });
        let error = prepare_request(&request).unwrap_err();
        assert_eq!(error.parameter(), Some("input.type"));
    }
}

#[test]
fn incoming_history_preserves_pending_calls_and_native_outputs() {
    for custom in [false, true] {
        let mut request = request(historical_call("call_synthetic", custom));
        request["input"].as_array_mut().unwrap().remove(1);
        let external = json!({"type":"function_call_output","call_id":"call_external",
            "name":"native_lookup","output":"Synthetic native result"});
        request["input"]
            .as_array_mut()
            .unwrap()
            .push(external.clone());
        let replay = replay_items(&request);
        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0]["call_id"], "call_synthetic");
        assert_eq!(replay[1], external);
    }
}

#[test]
#[ignore = "synthetic history-only benchmark; no provider requests"]
fn measure_bps_history_replay() {
    use std::{hint::black_box, time::Instant};

    for count in [0, 16, 1024] {
        let tools = super::super::catalog::client_tools(&json!({"tools":(0..count).map(|index|
            json!({"type":"function","name":format!("synthetic_{index}")})).collect::<Vec<_>>()
        }))
        .unwrap();
        let name = format!("synthetic_{}", count.max(1) - 1);
        let history: Vec<_> = (0..128).flat_map(|index| {
            let id = format!("call_synthetic_{index}");
            [json!({"type":"function_call","name":name,"call_id":id,"arguments":"{\"value\":1}"}),
                json!({"type":"function_call_output","call_id":id,"output":"Synthetic result"})]
        }).collect();
        let expected = super::super::history::translate_input_items(history.clone()).unwrap();
        let mut samples = Vec::new();
        for trial in 0..8 {
            let started = Instant::now();
            for _ in 0..20 {
                black_box(
                    super::super::history::translate_input_items(black_box(history.clone()))
                        .unwrap(),
                );
            }
            if trial > 0 {
                samples.push(started.elapsed().as_micros() / 20);
            }
        }
        samples.sort_unstable();
        assert_eq!(
            super::super::history::translate_input_items(history.clone()).unwrap(),
            expected
        );
        eprintln!(
            "bps-history catalog={} calls=128 warmup=1 trials=7 iterations=20 median_us={} output_bytes={} hash={}",
            tools.len(),
            samples[3],
            serde_json::to_vec(&expected).unwrap().len(),
            super::super::codec::short_hash(&expected)
        );
    }
}
