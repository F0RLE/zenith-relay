use super::*;
use std::{hint::black_box, time::Instant};

#[test]
#[ignore = "synthetic identifier-only benchmark"]
fn measure_bps_identifier_replay() {
    for long in [false, true] {
        let mut items = (0..256).flat_map(|index| {
            let call_id = format!("call_synthetic_{index}{}", if long { "x".repeat(80) } else { String::new() });
            [json!({"type":"function_call","id":format!("fc_{call_id}"),"call_id":call_id}),
                json!({"type":"function_call_output","id":format!("fc_{call_id}"),"call_id":call_id,"output":"Synthetic result"})]
        }).collect::<Vec<_>>();
        limit_responses_identifiers(&mut items).unwrap();
        let expected = items.clone();
        let mut samples = Vec::new();
        for trial in 0..8 {
            let started = Instant::now();
            for _ in 0..200 {
                limit_responses_identifiers(black_box(&mut items)).unwrap();
            }
            if trial > 0 {
                samples.push(started.elapsed().as_micros() / 200);
            }
        }
        samples.sort_unstable();
        assert_eq!(items, expected);
        eprintln!(
            "bps-identifiers fitted_long={long} items=512 warmup=1 trials=7 iterations=200 median_us={} hash={}",
            samples[3],
            crate::gateway::execution::basis_points::codec::short_hash(&items)
        );
    }
}
