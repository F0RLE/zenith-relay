use super::{consume_frames, take_event};
use std::{hint::black_box, time::Instant};

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
