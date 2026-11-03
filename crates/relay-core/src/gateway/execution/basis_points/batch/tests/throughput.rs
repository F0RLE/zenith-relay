use super::*;

fn mixed_output(bytes: usize, batches: usize) -> Vec<Value> {
    let mut items = Vec::new();
    for index in 0..8 {
        items.push(json!({
            "type":"message","id":format!("msg_{index}"),"role":"assistant",
            "content":[{"type":"output_text","text":"x".repeat(bytes)}]
        }));
        if index < batches {
            let mut batch = pair();
            batch["id"] = json!(format!("fc_{index}"));
            batch["call_id"] = json!(format!("parent_{index}"));
            items.push(batch);
        }
    }
    items
}

#[test]
fn passthrough_keeps_output_storage_and_batch_order() {
    let request = request();
    let tools = client_tools(&request).unwrap();
    for batches in [0, 1, 8] {
        let mut items = mixed_output(4096, batches);
        let before = items.clone();
        let original_buffer = items.as_ptr();
        let text_pointers: Vec<_> = items
            .iter()
            .filter_map(|item| item.pointer("/content/0/text").and_then(Value::as_str))
            .map(str::as_ptr)
            .collect();
        expand_output(&mut items, &request, &tools).unwrap();
        assert_eq!(items.len(), 8 + batches * 2);
        if batches == 0 {
            assert_eq!(items.as_ptr(), original_buffer);
            assert_eq!(items, before);
        }
        let mut position = 0;
        for (index, pointer) in text_pointers.iter().enumerate() {
            let item = &items[position];
            assert_eq!(item["id"], format!("msg_{index}"));
            assert_eq!(
                item["content"][0]["text"].as_str().unwrap().as_ptr(),
                *pointer
            );
            position += 1;
            if index < batches {
                for child in 0..2 {
                    let alias = Alias::decode(items[position]["call_id"].as_str().unwrap())
                        .unwrap()
                        .unwrap();
                    assert_eq!(alias.parent, format!("parent_{index}"));
                    assert_eq!((alias.index, alias.count), (child, 2));
                    position += 1;
                }
            }
        }
    }
}

#[test]
fn failed_late_batch_leaves_values_and_allocations_unchanged() {
    let request = request();
    let tools = client_tools(&request).unwrap();
    let mut items = mixed_output(4096, 1);
    let mut invalid = pair();
    invalid["call_id"] = json!("invalid_parent");
    invalid["arguments"] = json!({
        "references":["inspect","missing_tool"],
        "code":format!("{PREFIX}[{{\"path\":\"a\"}},{{\"path\":\"b\"}}]")
    })
    .to_string()
    .into();
    items.push(invalid);
    let before = items.clone();
    let original_buffer = items.as_ptr();
    let text_pointer = items[0]["content"][0]["text"].as_str().unwrap().as_ptr();
    assert!(expand_output(&mut items, &request, &tools).is_err());
    assert_eq!(items, before);
    assert_eq!(items.as_ptr(), original_buffer);
    assert_eq!(
        items[0]["content"][0]["text"].as_str().unwrap().as_ptr(),
        text_pointer
    );
}

fn single_call(bytes: usize, stringified: bool) -> Value {
    let unit = "synthetic \"quoted\"\\path\r\nстрока\t";
    let code = json!({"path":unit.repeat(bytes / unit.len() + 1)}).to_string();
    let mut arguments = json!({"references":["inspect"],"code":code});
    if stringified {
        arguments = Value::String(arguments.to_string());
    }
    json!({"type":"function_call","name":TRANSPORT_TOOL,"call_id":"single",
        "arguments":arguments,"status":"completed"})
}

#[test]
fn batch_probe_preserves_single_call_storage_and_json_reference_semantics() {
    let request = request();
    let tools = client_tools(&request).unwrap();
    for stringified in [false, true] {
        let mut items = vec![single_call(4096, stringified)];
        let before = items.clone();
        let pointer = items.as_ptr();
        expand_output(&mut items, &request, &tools).unwrap();
        assert_eq!(items, before);
        assert_eq!(items.as_ptr(), pointer);
    }

    let code =
        serde_json::to_string(&format!("{PREFIX}[{{\"path\":\"a\"}},{{\"path\":\"b\"}}]")).unwrap();
    // Escape the marker so these cases exercise metadata screening rather than
    // its conservative literal-marker shortcut.
    let code = code.replacen("relay-tool-batch-v1", r"\u0072elay-tool-batch-v1", 1);
    for (fields, count) in [
        (
            r#""references":["inspect"],"references":["inspect","inspect"]"#,
            2,
        ),
        (
            r#""references":["inspect","inspect"],"references":["inspect"]"#,
            1,
        ),
        (r#""refere\u006eces":["inspect","inspect"]"#, 2),
        (
            r#""metadata":{"references":["inspect","inspect"]},"references":["inspect"]"#,
            1,
        ),
        (r#""references":null"#, 1),
        (r#""references":{"nested":["inspect","inspect"]}"#, 1),
        (r#""references":[]"#, 1),
    ] {
        let text = format!("{{{fields},\"code\":{code}}}");
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let mut expected = vec![pair()];
        expected[0]["arguments"] = parsed;
        expand_output(&mut expected, &request, &tools).unwrap();
        let mut items = vec![pair()];
        items[0]["arguments"] = Value::String(text);
        let before = items.clone();
        expand_output(&mut items, &request, &tools).unwrap();
        assert_eq!(items.len(), count, "fields={fields}");
        if count == 1 {
            assert_eq!(items, before);
        } else {
            assert_eq!(items, expected);
        }
    }
}

#[test]
fn batch_probe_never_releases_malformed_single_call_arguments() {
    for arguments in [
        r#"{"references":["inspect"],"code":"\uD800"}"#,
        r#"{"references":["inspect"],"code":"\q"}"#,
        r#"{"references":["inspect"],"code":"{}"} trailing"#,
        r#"{"references":["inspect"],"code":"{}""#,
        r#"{"references":["inspect","inspect"],"code":"\uD800"}"#,
        r#"{"references":["inspect"],"code":"relay-tool-batch-v1\uD800"}"#,
    ] {
        let mut item = pair();
        item["arguments"] = json!(arguments);
        let error = translate_response_value(response(item), &request()).unwrap_err();
        assert_eq!(error.parameter(), Some("output.run_officejs.arguments"));
    }
}

#[test]
#[ignore = "local BPS single-call batch inspection benchmark; no provider requests"]
fn measure_bps_single_call_batch_probe() {
    use std::{hint::black_box, time::Instant};

    let request = request();
    let tools = client_tools(&request).unwrap();
    for bytes in [4096, 1024 * 1024] {
        for stringified in [false, true] {
            for batch in [false, true] {
                let mut source = vec![single_call(bytes, stringified)];
                if batch {
                    source.push(pair());
                }
                let wire_bytes = serde_json::to_vec(&source).unwrap().len();
                let mut expected = source.clone();
                expand_output(&mut expected, &request, &tools).unwrap();
                let mut samples = Vec::new();
                for trial in 0..16 {
                    let mut items = source.clone();
                    let started = Instant::now();
                    expand_output(black_box(&mut items), &request, &tools).unwrap();
                    let elapsed = started.elapsed().as_nanos();
                    assert_eq!(items, expected);
                    if trial > 0 {
                        samples.push(elapsed);
                    }
                }
                samples.sort_unstable();
                eprintln!(
                    "bps-batch-probe stringified={stringified} batch={batch} wire_bytes={wire_bytes} trials=15 median_ns={} p90_ns={}",
                    samples[7], samples[13]
                );
            }
        }
    }
}

#[test]
#[ignore = "local BPS batch expansion benchmark; no provider requests"]
fn measure_bps_batch_passthrough() {
    use std::{hint::black_box, time::Instant};

    let request = request();
    let tools = client_tools(&request).unwrap();
    for bytes in [1024, 1024 * 1024] {
        for batches in [0, 1, 8] {
            let source = mixed_output(bytes, batches);
            let mut expected = source.clone();
            expand_output(&mut expected, &request, &tools).unwrap();
            let mut timings = Vec::new();
            for _ in 0..9 {
                let mut items = source.clone();
                let started = Instant::now();
                expand_output(black_box(&mut items), &request, &tools).unwrap();
                timings.push(started.elapsed());
                assert_eq!(items, expected);
            }
            timings.sort();
            eprintln!(
                "BPS batch passthrough: 8 x {bytes} bytes, batches={batches}, median={:?}",
                timings[4]
            );
        }
    }
}
