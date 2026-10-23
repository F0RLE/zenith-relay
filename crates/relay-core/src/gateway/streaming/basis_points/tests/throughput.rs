use super::*;

fn parallel_wire(fragments: usize) -> Vec<u8> {
    let calls: Vec<_> = (0..8)
        .map(|index| {
            let mut item = call("inspect");
            item["id"] = json!(format!("fc_parallel_{index}"));
            item["call_id"] = json!(format!("call_parallel_{index}"));
            item["arguments"] = json!({
                "references":["inspect"],
                "code":json!({"path":"x".repeat(fragments * 64)}).to_string()
            })
            .to_string()
            .into();
            item
        })
        .collect();
    let mut wire = Vec::new();
    let mut arguments: Vec<_> = calls
        .iter()
        .map(|call| call["arguments"].as_str().unwrap().as_bytes().chunks(64))
        .collect();
    loop {
        let mut pending = false;
        for (index, chunks) in arguments.iter_mut().enumerate() {
            if let Some(chunk) = chunks.next() {
                pending = true;
                wire.extend_from_slice(&frame(json!({
                    "type":"response.function_call_arguments.delta",
                    "output_index":index,"item_id":calls[index]["id"],
                    "delta":std::str::from_utf8(chunk).unwrap()
                })));
            }
        }
        if !pending {
            break;
        }
    }
    wire.extend_from_slice(&frame(completed(json!(calls))));
    wire
}

#[test]
fn parallel_tool_fragments_are_independent_of_network_chunking() {
    let wire = parallel_wire(8);
    let mut expected = Translator::new(request());
    expected.push(&wire);
    let expected = output(&mut expected);
    assert!(!expected.contains("event: error"));
    assert_eq!(
        expected.matches("event: response.output_item.done").count(),
        8
    );
    assert_eq!(expected.matches("event: response.completed").count(), 1);
    for chunk_size in [1, 17, 4096] {
        let mut translator = Translator::new(request());
        for chunk in wire.chunks(chunk_size) {
            translator.push(chunk);
        }
        assert_eq!(output(&mut translator), expected);
        assert!(translator.terminal);
        assert!(translator.pending.is_empty());
    }
}

#[test]
#[ignore = "local parallel BPS SSE normalization benchmark; no provider requests"]
fn parallel_stream_normalization_latency() {
    use std::{hint::black_box, time::Instant};
    for fragments in [128, 1024] {
        let wire = parallel_wire(fragments);
        let mut expected = Translator::new(request());
        expected.push(&wire);
        let expected = output(&mut expected);
        assert!(!expected.contains("event: error"));
        for chunk_size in [64 * 1024, wire.len()] {
            let mut timings = Vec::new();
            for _ in 0..5 {
                let mut translator = Translator::new(request());
                let started = Instant::now();
                for chunk in wire.chunks(chunk_size) {
                    translator.push(black_box(chunk));
                }
                timings.push(started.elapsed());
                assert_eq!(output(&mut translator), expected);
            }
            timings.sort();
            eprintln!(
                "8 parallel calls, {fragments} fragments/call, {} bytes, chunk={chunk_size}: median={:?}",
                wire.len(), timings[2]
            );
        }
    }
}
