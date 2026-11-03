use super::*;

#[test]
#[ignore = "synthetic upstream SSE benchmark"]
fn measure_timed_upstream_forwarding() {
    const DELTAS: usize = 4096;
    let mut wire = Vec::new();
    wire.extend_from_slice(
        b"data: {\"type\":\"response.created\",\"response\":{\"model\":\"synthetic-model\"}}\n\n",
    );
    for sequence in 0..DELTAS {
        let payload = json!({
            "type":"response.output_text.delta", "item_id":"msg_synthetic",
            "output_index":0, "content_index":0, "sequence_number":sequence,
            "delta":"Synthetic output\n"
        });
        wire.extend_from_slice(format!("data: {payload}\n\n").as_bytes());
    }
    let terminal = json!({"type":"response.completed","response":{
        "model":"synthetic-model","status":"completed","output":[],
        "usage":{"input_tokens":100,"output_tokens":DELTAS}
    }});
    wire.extend_from_slice(format!("data: {terminal}\n\n").as_bytes());
    let mut frames = Vec::new();
    let mut remaining = wire.as_slice();
    while let Some(end) = crate::protocol::sse_event_end(remaining) {
        frames.push(&remaining[..end]);
        remaining = &remaining[end..];
    }
    assert!(remaining.is_empty());
    for validate_identity in [false, true] {
        let mut samples = [Vec::new(), Vec::new()];
        for trial in 0..9 {
            // Alternate order to balance warm caches and scheduling noise.
            for path in [trial % 2, 1 - trial % 2] {
                let mut capture = UpstreamUsage::new(
                    crate::gateway::test_support::test_usage_event(),
                    validate_identity.then(|| "synthetic-model".to_string()),
                )
                .with_timing(Instant::now());
                let started = Instant::now();
                for frame in &frames {
                    if path == 0 {
                        capture.observe_full_frame(std::hint::black_box(frame));
                    } else {
                        capture.observe_frame(std::hint::black_box(frame));
                    }
                }
                let elapsed = started.elapsed();
                let mut event = crate::gateway::test_support::test_usage_event();
                capture.apply_to(&mut event);
                assert_eq!(event.input_tokens, Some(100));
                assert_eq!(event.output_tokens, Some(DELTAS as u64));
                assert!(event.ttft_ms.is_some_and(|first| first <= event.latency_ms));
                assert!(capture.rejection.is_none());
                if trial >= 2 {
                    samples[path].push(elapsed);
                }
            }
        }
        for sample in &mut samples {
            sample.sort_unstable();
        }
        eprintln!(
            "observer comparison: identity={validate_identity}, deltas={DELTAS}, full={:?}, fast={:?}, speedup={:.2}x",
            samples[0][3],
            samples[1][3],
            samples[0][3].as_secs_f64() / samples[1][3].as_secs_f64()
        );
    }
    for validate_identity in [false, true] {
        for chunk_size in [16 * 1024, wire.len()] {
            let chunks: Vec<_> = wire
                .chunks(chunk_size)
                .map(Bytes::copy_from_slice)
                .collect();
            let mut samples = Vec::new();
            for trial in 0..9 {
                let mut capture = UpstreamUsage::new(
                    crate::gateway::test_support::test_usage_event(),
                    validate_identity.then(|| "synthetic-model".to_string()),
                )
                .with_timing(Instant::now());
                let mut forwarded = Vec::with_capacity(wire.len());
                let started = Instant::now();
                for chunk in &chunks {
                    forwarded
                        .extend_from_slice(&capture.forward(std::hint::black_box(chunk.clone())));
                }
                let elapsed = started.elapsed();
                assert_eq!(forwarded, wire);
                assert!(capture.pending.is_empty());
                let mut event = crate::gateway::test_support::test_usage_event();
                capture.apply_to(&mut event);
                assert_eq!(event.input_tokens, Some(100));
                assert_eq!(event.output_tokens, Some(DELTAS as u64));
                assert!(event.ttft_ms.is_some_and(|first| first <= event.latency_ms));
                if trial >= 2 {
                    samples.push(elapsed);
                }
            }
            samples.sort_unstable();
            eprintln!(
                "timed upstream: identity={validate_identity}, deltas={DELTAS}, bytes={}, chunk={chunk_size}, median={:?}, min={:?}, max={:?}",
                wire.len(),
                samples[3],
                samples[0],
                samples[6]
            );
        }
    }
}
