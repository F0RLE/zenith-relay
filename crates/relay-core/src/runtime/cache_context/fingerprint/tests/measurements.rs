use super::*;

#[test]
#[ignore = "manual synthetic cache-context fingerprint measurement"]
fn measure_cache_context_fingerprinting() {
    use std::{hint::black_box, time::Instant};

    let salt = [7; 32];
    for items in [16, 256] {
        let request = json!({
            "model":"synthetic-model",
            "instructions":"Stable synthetic instructions.",
            "tools": (0..48).map(|index| json!({
                "type":"function", "name":format!("tool_{index}"),
                "parameters":{"type":"object","properties":{
                    "path":{"type":"string"},"text":{"type":"string"}
                },"required":["path","text"],"additionalProperties":false}
            })).collect::<Vec<_>>(),
            "input": (0..items).map(|index| json!({
                "role": if index % 2 == 0 { "user" } else { "assistant" },
                "content":[{"type":"input_text","text":
                    "Synthetic text with \"quotes\", \\paths,\n\tкириллица. ".repeat(32)}]
            })).collect::<Vec<_>>()
        });
        let expected = RequestFingerprint::capture(&request, &salt).unwrap();
        let iterations = 100;
        for _ in 0..20 {
            black_box(RequestFingerprint::capture(black_box(&request), &salt).unwrap());
        }
        let mut samples = Vec::new();
        for _ in 0..9 {
            let started = Instant::now();
            for _ in 0..iterations {
                let actual = RequestFingerprint::capture(black_box(&request), &salt).unwrap();
                assert!(actual.changes_from(&expected).is_empty());
                assert_eq!(actual.input_bytes, expected.input_bytes);
                assert_eq!(
                    actual.history(Some(&expected)).shared_prefix_items,
                    Some(items)
                );
                black_box(actual);
            }
            samples.push(started.elapsed().as_nanos() / iterations);
        }
        samples.sort_unstable();
        eprintln!(
            "cache_context items={items} bytes={} warmup=20 trials=9 iterations={iterations} median_ns={} min_ns={} max_ns={}",
            serde_json::to_vec(&request).unwrap().len(),
            samples[4],
            samples[0],
            samples[8]
        );
    }
}
