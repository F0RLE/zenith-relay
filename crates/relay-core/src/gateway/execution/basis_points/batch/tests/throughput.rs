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
