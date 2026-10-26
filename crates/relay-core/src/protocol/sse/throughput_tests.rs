use super::{consume_frames, fragmentation_tests::scalar_line_end, line_end, take_event};
use std::{hint::black_box, time::Instant};

#[test]
#[ignore = "manual synthetic scalar versus vectorized SSE line scan measurement"]
fn measure_line_ending_search() {
    for payload_bytes in [0, 16, 240, 64 * 1024, 1024 * 1024] {
        let input = format!("{}\r\n", "x".repeat(payload_bytes)).into_bytes();
        assert_eq!(line_end(&input), scalar_line_end(&input));
        let iterations = if payload_bytes < 1024 { 10_000 } else { 100 };
        let mut samples = [Vec::new(), Vec::new()];
        for _ in 0..5 {
            black_box(line_end(black_box(&input)));
            black_box(scalar_line_end(black_box(&input)));
        }
        for trial in 0..7 {
            for variant in [trial % 2, 1 - trial % 2] {
                let started = Instant::now();
                for _ in 0..iterations {
                    black_box(if variant == 0 {
                        line_end(black_box(&input))
                    } else {
                        scalar_line_end(black_box(&input))
                    });
                }
                samples[variant].push(started.elapsed().as_nanos() / iterations);
            }
        }
        for (variant, mut samples) in samples.into_iter().enumerate() {
            samples.sort_unstable();
            let variant = if variant == 0 { "memchr" } else { "scalar" };
            eprintln!(
                "SSE line scan variant={variant} payload={payload_bytes} iterations={iterations} trials=7 median_ns={} min_ns={} max_ns={}",
                samples[3], samples[0], samples[6]
            );
        }
    }
}

#[test]
#[ignore = "manual synthetic complete SSE chunk throughput measurement"]
fn measure_complete_frame_consumption() {
    let frame = format!("data: {}\n\n", "x".repeat(240));
    for count in [1, 64, 1024] {
        let input = frame.repeat(count).into_bytes();
        let iterations = 1000;
        let mut samples = Vec::new();
        for _ in 0..7 {
            let mut pending = Vec::new();
            let mut delivered = 0;
            let started = Instant::now();
            for _ in 0..iterations {
                consume_frames(
                    &mut pending,
                    black_box(&input),
                    &mut delivered,
                    |_| false,
                    |total, event| *total += black_box(event).len(),
                );
            }
            samples.push(started.elapsed());
            assert_eq!(delivered, input.len() * iterations);
            assert!(pending.is_empty());
        }
        samples.sort_unstable();
        eprintln!(
            "complete frames/chunk={count}, iterations={iterations}: median={:?}",
            samples[3]
        );
    }
}

#[test]
#[ignore = "manual synthetic fragmented SSE parsing throughput measurement"]
fn measure_fragmented_frame_consumption() {
    for payload_bytes in [64 * 1024, 1024 * 1024] {
        let input = format!("data: {}\r\n\r\n", "x".repeat(payload_bytes)).into_bytes();
        for chunk_bytes in [1024, 64 * 1024] {
            let mut samples = Vec::new();
            for _ in 0..5 {
                let mut pending = Vec::new();
                let mut delivered = 0;
                let started = Instant::now();
                for chunk in input.chunks(chunk_bytes) {
                    consume_frames(
                        &mut pending,
                        black_box(chunk),
                        &mut delivered,
                        |_| false,
                        |total, frame| *total += black_box(frame).len(),
                    );
                }
                samples.push(started.elapsed());
                assert_eq!(delivered, input.len());
                assert!(pending.is_empty());
            }
            samples.sort_unstable();
            eprintln!(
                "payload={payload_bytes} bytes, chunk={chunk_bytes} bytes: median={:?}",
                samples[2]
            );
        }
    }
}

#[test]
#[ignore = "manual synthetic SSE parsing throughput measurement"]
fn measure_coalesced_frame_consumption() {
    let frame = format!("data: {}\n\n", "x".repeat(240));
    for count in [1, 64, 1024] {
        let input = frame.repeat(count).into_bytes();
        let iterations = 200;
        let started = Instant::now();
        let mut previous_bytes = 0;
        for _ in 0..iterations {
            let mut pending = black_box(input.clone());
            while let Some(frame) = take_event(&mut pending) {
                previous_bytes += black_box(frame).len();
            }
        }
        let previous = started.elapsed();
        let started = Instant::now();
        let mut current_bytes = 0;
        for _ in 0..iterations {
            consume_frames(
                &mut Vec::new(),
                black_box(&input),
                &mut current_bytes,
                |_| false,
                |total, frame| *total += black_box(frame).len(),
            );
        }
        let current = started.elapsed();
        assert_eq!(previous_bytes, input.len() * iterations);
        assert_eq!(current_bytes, previous_bytes);
        eprintln!("{count} frames/chunk: previous={previous:?}, current={current:?}");
    }
}
