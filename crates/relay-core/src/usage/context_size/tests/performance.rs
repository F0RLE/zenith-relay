use super::*;
use std::{hint::black_box, time::Instant};

fn measured(items: &[Value], detailed: bool) -> u64 {
    if detailed {
        let mut parts = ConversationSizeBreakdown::default();
        items
            .iter()
            .map(|item| parts.capture(black_box(item)))
            .sum()
    } else {
        // Prior per-item byte accounting, without the new classification.
        items.iter().map(crate::tool_policy::serialized_bytes).sum()
    }
}

#[test]
#[ignore = "paired synthetic context accounting; no provider or end-to-end latency"]
fn measure_context_composition() {
    for size in [256 * 1024, 1024 * 1024, 2 * 1024 * 1024] {
        for fragmented in [false, true] {
            let count = if fragmented { 256 } else { 1 };
            let text = "Synthetic quoted \"history\"\n/path. ".repeat(size / count / 35 + 1);
            let items: Vec<_> = (0..count)
                .map(|index| match index % 4 {
                    0 => json!({"role":"user","content":[{"type":"input_text","text":text}]}),
                    1 => json!({"type":"function_call_output","output":text}),
                    2 => json!({"role":"user","content":[{"type":"input_file","file_data":text}]}),
                    _ => json!({"type":"reasoning","encrypted_content":text}),
                })
                .collect();
            let expected = measured(&items, false);
            assert_eq!(measured(&items, true), expected);
            let mut samples = [Vec::new(), Vec::new()];
            for trial in 0..11 {
                // Alternate order; fixture construction and assertions are untimed.
                for index in [trial % 2, 1 - trial % 2] {
                    let start = Instant::now();
                    for _ in 0..10 {
                        black_box(measured(black_box(&items), index == 1));
                    }
                    if trial > 1 {
                        samples[index].push(start.elapsed().as_nanos() / 10);
                    }
                }
            }
            for values in &mut samples {
                values.sort_unstable();
            }
            eprintln!(
                "context-accounting bytes={expected} fragmented={fragmented} baseline_ns={} detailed_ns={} baseline_p90_ns={} detailed_p90_ns={}",
                samples[0][4], samples[1][4], samples[0][8], samples[1][8],
            );
        }
    }
}
