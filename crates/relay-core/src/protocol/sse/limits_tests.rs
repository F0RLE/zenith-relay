use super::consume_bounded_frames;

#[test]
fn oversized_fragmented_event_is_rejected_without_retaining_its_bytes() {
    let mut pending = Vec::new();
    let mut delivered = 0;
    for chunk in [b"data: ".as_slice(), b"1234567890"] {
        consume_bounded_frames(
            &mut pending,
            chunk,
            16,
            &mut delivered,
            |_| false,
            |n, _| *n += 1,
        )
        .unwrap();
    }
    assert_eq!(pending.len(), 16);
    assert!(
        consume_bounded_frames(
            &mut pending,
            b"x",
            16,
            &mut delivered,
            |_| false,
            |n, _| *n += 1
        )
        .is_err()
    );
    assert!(pending.is_empty());
    assert_eq!(delivered, 0);
}

#[test]
fn coalesced_events_may_exceed_the_individual_event_budget() {
    let frame = b"data: x\r\n\r\n";
    for chunk_size in 1..=frame.len() * 3 {
        let input = frame.repeat(3);
        let mut pending = Vec::new();
        let mut frames = Vec::new();
        for chunk in input.chunks(chunk_size) {
            consume_bounded_frames(
                &mut pending,
                chunk,
                frame.len(),
                &mut frames,
                |_| false,
                |frames, frame| frames.push(frame.to_vec()),
            )
            .unwrap();
        }
        assert!(pending.is_empty());
        assert_eq!(frames.concat(), input);
    }
}

#[test]
fn bytes_after_the_terminal_are_not_subject_to_the_event_limit() {
    let mut pending = Vec::new();
    let mut delivered = 0;
    consume_bounded_frames(
        &mut pending,
        b"data: x\n\nxxxxxxxxxxxxxxxxxxxxxxxx",
        10,
        &mut delivered,
        |n| *n != 0,
        |n, _| *n += 1,
    )
    .unwrap();
    assert_eq!(delivered, 1);
    assert!(pending.is_empty());
}
